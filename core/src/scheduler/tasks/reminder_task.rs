//! 主动提醒任务（T1 路线）。
//!
//! 定时评估记忆与规则：命中"当前值得告知用户"的内容时，经
//! [`NotificationSink`] 推送系统通知，并可注入最新会话（ADR-013 消息通路）。
//! 评估失败静默（下一次调度自然重试，不重试、不堆积）。
use crate::common::types::{ContentLevel, ContextNamespace, StructuredMessage, TianyanUri};
use crate::model::ChatService;
use crate::notification::SharedNotificationSink;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};
use crate::session::SessionManager;
use async_trait::async_trait;
use std::sync::Arc;
/// 每次评估采样的记忆条目数上限。
const MAX_MEMORY_SAMPLES: usize = 10;
/// 单条记忆文本截断字符数。
const MEMORY_SAMPLE_CHARS: usize = 200;
/// 提醒文本注入会话时的角色消息构建（复用 SessionTaskNotifier 模式）。
const REMINDER_TITLE: &str = "天演提醒";
/// LLM 评估 prompt：输出 EMPTY 表示无值得提醒的内容。
const EVAL_SYSTEM_PROMPT: &str = "\
你是一个本地智能体助手的主动提醒评估器。以下是近期记忆与规则：
{context}
判断其中是否有当前值得主动告知用户的事项（例如：用户明确要求提醒的事项、到期任务、需要用户注意的规则触发）。
若没有值得提醒的内容，只输出 EMPTY 四个字母。
若有，输出最多 {max_per_run} 条提醒，每条单独一行，以「提醒：」开头，简明扼要（每行不超过 100 字）。";
/// 主动提醒任务。
///
/// 依赖注入（与 RuleTask 同模式）：model_service 做评估、notification 推送、
/// session_manager 注入会话；vfs 从 [`TaskContext`] 读取记忆。
pub struct ReminderTask {
    /// 评估用聊天模型服务。
    model_service: Arc<dyn ChatService>,
    /// 评估模型名称。
    model_name: String,
    /// 系统通知通道（未装配时静默）。
    notification: SharedNotificationSink,
    /// 会话管理器（提醒注入最新会话）。
    session_manager: Arc<dyn SessionManager>,
    /// 每次运行最多提醒条数。
    max_per_run: usize,
    /// 是否注入到最新会话。
    inject_to_session: bool,
}
impl ReminderTask {
    /// 创建主动提醒任务。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        model_name: impl Into<String>,
        notification: SharedNotificationSink,
        session_manager: Arc<dyn SessionManager>,
        max_per_run: usize,
        inject_to_session: bool,
    ) -> Self {
        Self {
            model_service,
            model_name: model_name.into(),
            notification,
            session_manager,
            max_per_run: max_per_run.max(1),
            inject_to_session,
        }
    }
    /// 采样最近记忆（memory 命名空间，深度 2：类别目录 → 条目）。
    async fn sample_memories(&self, ctx: &TaskContext) -> Vec<String> {
        let root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let mut samples = Vec::new();
        let Ok(categories) = ctx.vfs.list(&root).await else {
            return samples;
        };
        for category in categories.iter().take(MAX_MEMORY_SAMPLES) {
            let Ok(entries) = ctx.vfs.list(category.uri()).await else {
                continue;
            };
            // 每个类别取最新一条（list 返回顺序与写入顺序一致，取末尾）
            let mut latest: Option<TianyanUri> = None;
            for entry in entries {
                if !entry.is_directory() {
                    latest = Some(entry.uri().clone());
                }
            }
            if let Some(uri) = latest {
                if let Ok(content) = ctx.vfs.read_content(&uri, ContentLevel::Detail).await {
                    let truncated: String = content.chars().take(MEMORY_SAMPLE_CHARS).collect();
                    samples.push(format!("- {}: {}", uri, truncated));
                }
            }
            if samples.len() >= MAX_MEMORY_SAMPLES {
                break;
            }
        }
        samples
    }
    /// LLM 评估：返回提醒文本列表（无提醒时为空）。
    async fn evaluate(
        &self,
        memories: &[String],
    ) -> Result<Vec<String>, crate::common::error::TianyanError> {
        if memories.is_empty() {
            return Ok(Vec::new());
        }
        let context = memories.join("\n");
        let system_prompt = EVAL_SYSTEM_PROMPT
            .replace("{context}", &context)
            .replace("{max_per_run}", &self.max_per_run.to_string());
        let request = crate::model::types::ChatCompletionRequest::new(
            &self.model_name,
            vec![
                crate::common::types::Message::system(system_prompt),
                crate::common::types::Message::user("请评估上述记忆，输出提醒或 EMPTY。"),
            ],
        );
        let response = self
            .model_service
            .chat_completion(request)
            .await
            .map_err(|e| {
                crate::common::error::TianyanError::Custom(format!("reminder: LLM 评估失败：{}", e))
            })?;
        let content = response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        let content = content.trim();
        if content.is_empty() || content == "EMPTY" || content.starts_with("EMPTY") {
            return Ok(Vec::new());
        }
        Ok(content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && l.starts_with("提醒："))
            .take(self.max_per_run)
            .collect())
    }
    /// 注入提醒到最新会话（System 消息，随历史进入下一轮上下文组装）。
    async fn inject_to_latest_session(&self, text: &str) {
        let Ok(sessions) = self.session_manager.list_sessions().await else {
            return;
        };
        let Some(latest) = sessions.into_iter().max_by_key(|s| s.created_at) else {
            return;
        };
        let sm =
            StructuredMessage::system(latest.session_id.clone(), format!("[主动提醒]\n{}", text));
        if let Err(e) = self
            .session_manager
            .add_structured_message(&latest.session_id, sm)
            .await
        {
            tracing::warn!(error = %e, "主动提醒注入会话失败");
        }
    }
}
#[async_trait]
impl TaskHandler for ReminderTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        // 配置门控：未启用时跳过（scheduler 仍可能注册，执行时判定）
        if !ctx.config.reminder.enabled {
            return TaskResult::success(0);
        }
        tracing::info!("开始执行主动提醒评估...");
        let memories = self.sample_memories(ctx).await;
        if memories.is_empty() {
            tracing::debug!("没有可评估的记忆，跳过提醒");
            return TaskResult::success(0);
        }
        let reminders = match self.evaluate(&memories).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "主动提醒评估失败（静默，下次调度重试）");
                return TaskResult::failed(e);
            }
        };
        if reminders.is_empty() {
            tracing::debug!("没有值得提醒的内容");
            return TaskResult::success(0);
        }
        let text = reminders.join("\n");
        // 系统通知（桌面通知通道）
        self.notification.notify(REMINDER_TITLE, &text);
        // 注入最新会话（ADR-013 消息通路：主 LLM 下一轮可见）
        if self.inject_to_session {
            self.inject_to_latest_session(&text).await;
        }
        tracing::info!(count = reminders.len(), "主动提醒已推送");
        TaskResult::success(reminders.len())
    }
    fn name(&self) -> &str {
        "reminder"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{Message, MessageRole, Part};
    use crate::memory::MemoryExtractor;
    use crate::model::MockChatService;
    use crate::session::Session;
    use crate::test_utils::MockVfs;
    use crate::vfs::{ContentStore, VfsCore, VirtualFileSystem};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    /// 记录通知次数的测试通知器。
    struct RecordingSink {
        calls: Arc<AtomicUsize>,
    }
    impl crate::notification::NotificationSink for RecordingSink {
        fn notify(&self, _title: &str, _body: &str) {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }
    /// 记录注入次数的会话管理器 mock。
    struct RecordingSessionManager {
        injected: Arc<AtomicUsize>,
        sessions: Vec<Session>,
    }
    #[async_trait]
    impl SessionManager for RecordingSessionManager {
        async fn add_structured_message(
            &self,
            _session_id: &str,
            msg: StructuredMessage,
        ) -> Result<(), crate::common::error::TianyanError> {
            assert_eq!(msg.role, MessageRole::System, "提醒应为 System 消息");
            assert!(msg.parts.iter().any(|p| matches!(p, Part::Text { .. })));
            self.injected.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _id: &str,
            _msgs: &[StructuredMessage],
        ) -> Result<(), crate::common::error::TianyanError> {
            Ok(())
        }
        async fn create_session(
            &self,
            _id: &str,
            _m: Message,
        ) -> Result<Session, crate::common::error::TianyanError> {
            Ok(Session::new(_id))
        }
        async fn get_session(
            &self,
            _id: &str,
        ) -> Result<Option<Session>, crate::common::error::TianyanError> {
            Ok(self.sessions.iter().find(|s| s.session_id == _id).cloned())
        }
        async fn update_session(
            &self,
            _s: &Session,
        ) -> Result<(), crate::common::error::TianyanError> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<Session>, crate::common::error::TianyanError> {
            Ok(self.sessions.clone())
        }
        async fn delete_session(
            &self,
            _id: &str,
        ) -> Result<(), crate::common::error::TianyanError> {
            Ok(())
        }
    }
    fn make_ctx(vfs: Arc<dyn VirtualFileSystem>) -> TaskContext {
        let mut config = crate::config::TianyanConfig::default();
        config.reminder.enabled = true;
        let model_mock = Arc::new(MockChatService::new());
        let engine = Arc::new(crate::vfs::SummaryEngine::new(model_mock.clone(), "test"));
        let extractor = Arc::new(MemoryExtractor::new(
            model_mock,
            crate::memory::ExtractionConfig::default(),
        ));
        TaskContext::new(vfs, engine, extractor, Arc::new(config))
    }
    async fn seed_memory(vfs: &MockVfs) {
        let root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let dir = TianyanUri::new(ContextNamespace::Memory, vec!["clipboard".into()]);
        let uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["clipboard".into(), "1".into()],
        );
        // MockVfs 不自动维护父目录关系：手动建立 根→目录→条目 层级
        let _ = vfs.create_directory(&dir).await;
        let _ = vfs.create_file(&uri).await;
        let _ = vfs
            .write_content(&uri, "用户要求每天 9 点提醒检查项目进度")
            .await;
        let _ = vfs.write_abstract(&uri, "提醒").await;
        vfs.add_directory(&root, &dir);
        vfs.add_entry(&dir, &uri);
    }
    fn task(notification: SharedNotificationSink, sessions: Arc<AtomicUsize>) -> ReminderTask {
        ReminderTask::new(
            Arc::new(MockChatService::new()),
            "test".to_string(),
            notification,
            Arc::new(RecordingSessionManager {
                injected: sessions.clone(),
                sessions: vec![],
            }),
            3,
            true,
        )
    }
    #[tokio::test]
    async fn test_empty_no_notification() {
        let vfs = Arc::new(MockVfs::new());
        seed_memory(&vfs).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let injected = Arc::new(AtomicUsize::new(0));
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(1).returning(|_| {
            Ok(crate::model::types::ChatCompletionResponse {
                id: "r".into(),
                object: "chat.completion".into(),
                created: 0,
                model: "test".into(),
                choices: vec![crate::model::types::ChatChoice {
                    index: 0,
                    message: Message::assistant("EMPTY"),
                    finish_reason: Some("stop".into()),
                }],
                usage: crate::common::types::TokenUsage::default(),
            })
        });
        let t = ReminderTask::new(
            Arc::new(mock),
            "test".to_string(),
            Arc::new(RecordingSink {
                calls: calls.clone(),
            }),
            Arc::new(RecordingSessionManager {
                injected: injected.clone(),
                sessions: vec![],
            }),
            3,
            true,
        );
        let result = t.execute(&make_ctx(vfs)).await;
        assert!(result.success, "EMPTY 应正常结束");
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0, "无提醒不通知");
        assert_eq!(injected.load(AtomicOrdering::SeqCst), 0, "无提醒不注入");
    }
    #[tokio::test]
    async fn test_reminder_notifies_and_injects() {
        let vfs = Arc::new(MockVfs::new());
        seed_memory(&vfs).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let injected = Arc::new(AtomicUsize::new(0));
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(1).returning(|_| {
            Ok(crate::model::types::ChatCompletionResponse {
                id: "r".into(),
                object: "chat.completion".into(),
                created: 0,
                model: "test".into(),
                choices: vec![crate::model::types::ChatChoice {
                    index: 0,
                    message: Message::assistant("提醒：检查项目进度"),
                    finish_reason: Some("stop".into()),
                }],
                usage: crate::common::types::TokenUsage::default(),
            })
        });
        let t = ReminderTask::new(
            Arc::new(mock),
            "test".to_string(),
            Arc::new(RecordingSink {
                calls: calls.clone(),
            }),
            Arc::new(RecordingSessionManager {
                injected: injected.clone(),
                sessions: vec![Session::new("s1")],
            }),
            3,
            true,
        );
        let result = t.execute(&make_ctx(vfs)).await;
        assert!(result.success);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "命中应通知一次");
        assert_eq!(injected.load(AtomicOrdering::SeqCst), 1, "命中应注入会话");
    }
    #[tokio::test]
    async fn test_evaluation_failure_silent() {
        let vfs = Arc::new(MockVfs::new());
        seed_memory(&vfs).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let injected = Arc::new(AtomicUsize::new(0));
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(1).returning(|_| {
            Err(crate::common::error::TianyanError::Custom(
                "mock 失败".into(),
            ))
        });
        let t = ReminderTask::new(
            Arc::new(mock),
            "test".to_string(),
            Arc::new(RecordingSink {
                calls: calls.clone(),
            }),
            Arc::new(RecordingSessionManager {
                injected: injected.clone(),
                sessions: vec![],
            }),
            3,
            true,
        );
        let result = t.execute(&make_ctx(vfs)).await;
        assert!(!result.success, "评估失败返回失败（调度器记录，不重试）");
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0, "失败不通知");
        assert_eq!(injected.load(AtomicOrdering::SeqCst), 0, "失败不注入");
    }
    #[tokio::test]
    async fn test_disabled_config_skips() {
        let vfs = Arc::new(MockVfs::new());
        seed_memory(&vfs).await;
        let mut ctx = make_ctx(vfs);
        Arc::get_mut(&mut ctx.config).unwrap().reminder.enabled = false;
        let calls = Arc::new(AtomicUsize::new(0));
        let t = task(
            Arc::new(RecordingSink {
                calls: calls.clone(),
            }),
            Arc::new(AtomicUsize::new(0)),
        );
        let result = t.execute(&ctx).await;
        assert!(result.success);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0, "禁用时跳过");
    }
}
