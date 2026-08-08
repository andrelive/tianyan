//! 后台任务支持（fire-and-forget 委托）。
//!
//! 业界对标（opencode task(background=true) / oh-my-openagent
//! BackgroundManager / Claude Code background subagents）：
//! - 任务注册表：id → 状态机（Pending/Running/Completed/Failed/Cancelled）
//! - 并发上限：信号量限制同时运行的后台委托数（防失控扇出）
//! - 完成通知：任务完成时通过 [`TaskNotifier`] 通知父会话（默认实现把
//!   通知作为 System 消息持久化到会话——主 LLM 下一轮自然看到并继续，
//!   对应"noReply 注入 + 轮次边界投递"模式；天演为请求驱动架构，
//!   通知在用户下次交互时进入上下文）
//! - join 信号：通知携带"该会话剩余任务数"，全部完成时明确提示汇总
//!   （代码层只做计数与批量提示，最终聚合由主 LLM 完成——opencode 共识）
//!
//! 与 [`crate::scheduler`]（cron 定时任务）无关：本模块是 LLM 驱动的
//! agent 后台任务，不是定时调度。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
};
use crate::session::SessionManager;

/// 默认最大并发后台任务数。
pub const DEFAULT_MAX_BACKGROUND_TASKS: usize = 4;

/// 通知中结果摘要的最大字符数。
const RESULT_SUMMARY_MAX_CHARS: usize = 2000;

/// 后台任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已注册，等待并发许可。
    Pending,
    /// 运行中。
    Running,
    /// 已完成（含结果摘要）。
    Completed,
    /// 执行失败（含错误信息）。
    Failed,
    /// 已取消。
    Cancelled,
}

impl TaskStatus {
    /// 是否为终态。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }
}

/// 后台任务（注册表条目）。
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTask {
    /// 任务 ID（`bt_` 前缀）。
    pub id: String,
    /// 任务描述（原始委托任务文本，截断）。
    pub description: String,
    /// 状态。
    pub status: TaskStatus,
    /// 发起任务的父会话 ID。
    pub parent_session_id: String,
    /// 结果摘要（Completed 时）。
    pub result: Option<String>,
    /// 错误信息（Failed 时）。
    pub error: Option<String>,
    /// 创建时间（epoch 毫秒）。
    pub created_at: i64,
    /// 完成时间（epoch 毫秒）。
    pub completed_at: Option<i64>,
}

/// 任务完成通知器。
///
/// 由 Agent 装配层注入（默认实现 [`SessionTaskNotifier`] 把通知持久化
/// 到父会话）；`remaining` 为父会话中仍进行中的任务数（join 信号）。
#[async_trait]
pub trait TaskNotifier: Send + Sync {
    /// 任务进入终态时调用（Completed / Failed / Cancelled）。
    async fn on_task_terminal(&self, session_id: &str, task: &BackgroundTask, remaining: usize);
}

/// 后台任务管理器：注册表 + 并发限制 + 完成通知分发。
#[derive(Clone)]
pub struct BackgroundTaskManager {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    semaphore: Arc<Semaphore>,
    notifier: Option<Arc<dyn TaskNotifier>>,
}

impl BackgroundTaskManager {
    /// 创建管理器（默认并发上限）。
    pub fn new() -> Self {
        Self::with_max_concurrent(DEFAULT_MAX_BACKGROUND_TASKS)
    }

    /// 创建管理器并指定并发上限。
    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            notifier: None,
        }
    }

    /// 设置完成通知器（Agent 装配层注入）。
    pub fn with_notifier(mut self, notifier: Arc<dyn TaskNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// 尝试获取并发许可；达到上限时立即报错（不阻塞）。
    pub async fn try_acquire(&self) -> std::result::Result<OwnedSemaphorePermit, TianyanError> {
        self.semaphore.clone().try_acquire_owned().map_err(|_| {
            TianyanError::Custom(
                "tool: 后台任务并发已达上限，请等待现有任务完成，或取消后重试".to_string(),
            )
        })
    }

    /// 注册新任务（Pending）并返回任务 ID。
    pub async fn register(&self, description: String, parent_session_id: String) -> String {
        let id = format!("bt_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let task = BackgroundTask {
            id: id.clone(),
            description: truncate(&description, 200),
            status: TaskStatus::Pending,
            parent_session_id,
            result: None,
            error: None,
            created_at: now_ms(),
            completed_at: None,
        };
        self.tasks.lock().await.insert(id.clone(), task);
        id
    }

    /// 标记任务为运行中。
    pub async fn mark_running(&self, id: &str) {
        if let Some(t) = self.tasks.lock().await.get_mut(id) {
            t.status = TaskStatus::Running;
        }
    }

    /// 标记完成并触发通知。
    pub async fn complete(&self, id: &str, result: String) {
        self.finish(
            id,
            TaskStatus::Completed,
            Some(truncate(&result, RESULT_SUMMARY_MAX_CHARS)),
            None,
        )
        .await;
    }

    /// 标记失败并触发通知。
    pub async fn fail(&self, id: &str, error: String) {
        self.finish(id, TaskStatus::Failed, None, Some(truncate(&error, 500)))
            .await;
    }

    /// 取消任务并触发通知。
    pub async fn cancel(&self, id: &str) -> Result<()> {
        let exists = {
            let tasks = self.tasks.lock().await;
            tasks
                .get(id)
                .map(|t| t.status.is_terminal())
                .unwrap_or(false)
        };
        if exists {
            return Ok(());
        }
        self.finish(id, TaskStatus::Cancelled, None, None).await;
        Ok(())
    }

    /// 获取任务快照。
    pub async fn get(&self, id: &str) -> Option<BackgroundTask> {
        self.tasks.lock().await.get(id).cloned()
    }

    /// 获取全部任务快照（按创建时间排序）。
    pub async fn snapshot(&self) -> Vec<BackgroundTask> {
        let mut tasks: Vec<BackgroundTask> = self.tasks.lock().await.values().cloned().collect();
        tasks.sort_by_key(|t| t.created_at);
        tasks
    }

    /// 统一终态收尾：更新状态 → 计算剩余计数 → 通知。
    async fn finish(
        &self,
        id: &str,
        status: TaskStatus,
        result: Option<String>,
        error: Option<String>,
    ) {
        let (session_id, remaining, task) = {
            let mut tasks = self.tasks.lock().await;
            let Some(t) = tasks.get_mut(id) else {
                return;
            };
            if t.status.is_terminal() {
                return; // 幂等：终态不重复通知
            }
            t.status = status;
            t.result = result;
            t.error = error;
            t.completed_at = Some(now_ms());
            let session = t.parent_session_id.clone();
            let task = t.clone();
            // 释放可变借用后再统计剩余计数
            let remaining = tasks
                .values()
                .filter(|x| x.parent_session_id == session && !x.status.is_terminal())
                .count();
            (session, remaining, task)
        };

        if let Some(notifier) = &self.notifier {
            notifier
                .on_task_terminal(&session_id, &task, remaining)
                .await;
        }
    }
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认通知器：把完成通知作为 System 消息持久化到父会话。
///
/// 持久化后，通知随会话历史进入下一次上下文组装（主 LLM 下一轮看到）；
/// 前端会话历史同样可见。对应业界"完成时注入消息 + 轮次边界投递"。
pub struct SessionTaskNotifier {
    session_manager: Arc<dyn SessionManager>,
}

impl SessionTaskNotifier {
    /// 创建通知器。
    pub fn new(session_manager: Arc<dyn SessionManager>) -> Self {
        Self { session_manager }
    }
}

#[async_trait]
impl TaskNotifier for SessionTaskNotifier {
    async fn on_task_terminal(&self, session_id: &str, task: &BackgroundTask, remaining: usize) {
        let text = build_notification_text(task, remaining);
        let now = now_ms();
        let sm = StructuredMessage {
            id: format!("msg_{now}"),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text,
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now,
                completed: now,
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: false,
        };
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, sm)
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "后台任务通知持久化失败");
        }
    }
}

/// 构建通知文本（join 信号：携带剩余任务计数）。
pub fn build_notification_text(task: &BackgroundTask, remaining: usize) -> String {
    let status_label = match task.status {
        TaskStatus::Completed => "完成",
        TaskStatus::Failed => "失败",
        TaskStatus::Cancelled => "已取消",
        _ => "结束",
    };
    let detail = match (&task.result, &task.error) {
        (Some(r), _) => format!("结果：{r}"),
        (_, Some(e)) => format!("错误：{e}"),
        _ => String::new(),
    };

    let mut text = format!(
        "[后台任务{status_label}] {}（{}）",
        task.description, task.id
    );
    if !detail.is_empty() {
        text.push_str(&format!("\n{detail}"));
    }
    if remaining > 0 {
        text.push_str(&format!(
            "\n该会话还有 {remaining} 个后台任务进行中，全部完成后请汇总各任务结果。"
        ));
    } else {
        text.push_str("\n该会话的所有后台任务均已完成，请汇总各任务结果并继续原有工作。");
    }
    text
}

/// 截断文本（保留头尾）。
fn truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head = &text[..max_chars * 2 / 3];
    let tail_start = text.len().saturating_sub(max_chars / 3);
    let tail = &text[tail_start..];
    format!(
        "{}...\n[内容被截断，省略 {} 字符]...\n{}",
        head,
        text.len() - head.len() - tail.len(),
        tail
    )
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 供取消逻辑追踪运行句柄（保留字段位，防止未来扩展时遗漏）。
#[allow(dead_code)]
struct TaskHandle {
    _started: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(3000);
        let t = truncate(&long, 2000);
        assert!(t.contains("[内容被截断"));
    }

    #[test]
    fn test_notification_text_with_remaining() {
        let task = BackgroundTask {
            id: "bt_test".to_string(),
            description: "搜索资料".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("找到 3 篇".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
        };
        let text = build_notification_text(&task, 2);
        assert!(text.contains("[后台任务完成]"));
        assert!(text.contains("bt_test"));
        assert!(text.contains("找到 3 篇"));
        assert!(text.contains("还有 2 个后台任务进行中"));
    }

    #[test]
    fn test_notification_text_all_complete() {
        let task = BackgroundTask {
            id: "bt_x".to_string(),
            description: "分析日志".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("无异常".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
        };
        let text = build_notification_text(&task, 0);
        assert!(text.contains("所有后台任务均已完成"));
        assert!(text.contains("汇总各任务结果并继续"));
    }

    #[tokio::test]
    async fn test_register_and_complete() {
        let manager = BackgroundTaskManager::new();
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        assert!(id.starts_with("bt_"));
        assert_eq!(manager.get(&id).await.unwrap().status, TaskStatus::Pending);

        manager.mark_running(&id).await;
        assert_eq!(manager.get(&id).await.unwrap().status, TaskStatus::Running);

        manager.complete(&id, "done".to_string()).await;
        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result.as_deref(), Some("done"));
        assert!(task.completed_at.is_some());

        // 幂等：重复 complete 不覆盖
        manager.complete(&id, "again".to_string()).await;
        assert_eq!(
            manager.get(&id).await.unwrap().result.as_deref(),
            Some("done")
        );
    }

    #[tokio::test]
    async fn test_fail_and_cancel() {
        let manager = BackgroundTaskManager::new();
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.fail(&id, "boom".to_string()).await;
        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.error.as_deref(), Some("boom"));

        let id2 = manager.register("t2".to_string(), "s1".to_string()).await;
        manager.cancel(&id2).await.unwrap();
        assert_eq!(
            manager.get(&id2).await.unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn test_concurrency_limit() {
        let manager = BackgroundTaskManager::with_max_concurrent(2);
        let p1 = manager.try_acquire().await.unwrap();
        let p2 = manager.try_acquire().await.unwrap();
        let p3 = manager.try_acquire().await;
        assert!(p3.is_err(), "第 3 个许可应被拒绝");
        drop(p1);
        let p3 = manager.try_acquire().await;
        assert!(p3.is_ok(), "释放后应可获取");
        drop(p2);
        drop(p3.unwrap());
    }

    #[tokio::test]
    async fn test_notifier_called_with_remaining() {
        struct CountingNotifier(Arc<std::sync::Mutex<Vec<(String, usize)>>>);
        #[async_trait]
        impl TaskNotifier for CountingNotifier {
            async fn on_task_terminal(
                &self,
                _session_id: &str,
                task: &BackgroundTask,
                remaining: usize,
            ) {
                self.0.lock().unwrap().push((task.id.clone(), remaining));
            }
        }

        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let manager =
            BackgroundTaskManager::new().with_notifier(Arc::new(CountingNotifier(calls.clone())));

        let id1 = manager.register("a".to_string(), "s1".to_string()).await;
        let id2 = manager.register("b".to_string(), "s1".to_string()).await;
        manager.complete(&id1, "r1".to_string()).await;
        manager.complete(&id2, "r2".to_string()).await;

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2);
        // 先完成 id1 时剩余 1 个；再完成 id2 时剩余 0
        let first = recorded.iter().find(|(id, _)| *id == id1).unwrap();
        let second = recorded.iter().find(|(id, _)| *id == id2).unwrap();
        assert_eq!(first.1, 1, "id1 完成时 id2 仍在进行");
        assert_eq!(second.1, 0, "id2 完成时无剩余任务（join 信号）");
    }

    #[tokio::test]
    async fn test_snapshot_sorted() {
        let manager = BackgroundTaskManager::new();
        let id1 = manager.register("a".to_string(), "s1".to_string()).await;
        let id2 = manager.register("b".to_string(), "s1".to_string()).await;
        let snap = manager.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].id, id1);
        assert_eq!(snap[1].id, id2);
    }

    #[tokio::test]
    async fn test_session_notifier_persists_system_message() {
        use crate::session::SessionManager;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        // 手写 mock：记录 add_structured_message 调用
        struct RecordingSessionManager {
            calls: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl SessionManager for RecordingSessionManager {
            async fn create_session(
                &self,
                _id: &str,
                _message: crate::common::types::Message,
            ) -> Result<crate::session::Session> {
                unreachable!()
            }
            async fn get_session(&self, _id: &str) -> Result<Option<crate::session::Session>> {
                Ok(None)
            }
            async fn update_session(&self, _session: &crate::session::Session) -> Result<()> {
                Ok(())
            }
            async fn add_message(
                &self,
                _session_id: &str,
                _message: crate::common::types::Message,
            ) -> Result<()> {
                Ok(())
            }
            async fn add_structured_message(
                &self,
                _session_id: &str,
                msg: StructuredMessage,
            ) -> Result<()> {
                assert_eq!(msg.role, MessageRole::System, "通知应为 System 消息");
                assert!(
                    msg.parts.iter().any(|p| matches!(p, Part::Text { .. })),
                    "通知应含文本 part"
                );
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(())
            }
            async fn rewrite_messages(
                &self,
                _session_id: &str,
                _messages: &[StructuredMessage],
            ) -> Result<()> {
                Ok(())
            }
            async fn list_sessions(&self) -> Result<Vec<crate::session::Session>> {
                Ok(Vec::new())
            }
            async fn delete_session(&self, _id: &str) -> Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let notifier = SessionTaskNotifier::new(Arc::new(RecordingSessionManager {
            calls: calls.clone(),
        }));
        let task = BackgroundTask {
            id: "bt_t".to_string(),
            description: "任务".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("结果".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
        };
        notifier.on_task_terminal("s1", &task, 0).await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "应持久化一条通知");
    }
}
