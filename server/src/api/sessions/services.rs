use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;

use tianyan::session::SessionManager;

use crate::api::sessions::types::{
    DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse, RedoRequest, Session,
    SessionDetail, SessionMessagesResponse, UpdateTitleRequest,
};
use crate::api::shared::error::ApiError;
use crate::api::shared::types::ChatMessage;
use tianyan::common::types::StructuredMessage;
use tianyan::snapshot::SnapshotManager;
/// 分段加载默认单页条数（ADR-035 §8）。
const DEFAULT_MESSAGE_PAGE: usize = 50;
/// 分段加载单页条数上限（防超大页拖垮前端渲染）。
const MAX_MESSAGE_PAGE: usize = 200;

/// 会话服务，管理对话会话
pub struct SessionService {
    session_manager: Arc<dyn SessionManager>,
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory；会话级绑定缺省时的快照根）。
    default_working_dir: Option<PathBuf>,
    /// 会话工作集注册表（ADR-035 §3：写侧收口唯一通道；测试桩为 None 时回退直写）。
    working_sets: Option<Arc<tianyan::agent::working_set::WorkingSetRegistry>>,
}

impl SessionService {
    /// 创建新的会话服务
    pub fn new(
        session_manager: Arc<dyn SessionManager>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
        default_working_dir: Option<PathBuf>,
        working_sets: Option<Arc<tianyan::agent::working_set::WorkingSetRegistry>>,
    ) -> Self {
        Self {
            session_manager,
            snapshot_manager,
            default_working_dir,
            working_sets,
        }
    }

    /// 写入收口 ②（ADR-035 §3）：消息全量重写经工作集（缓存与库同步推进）；
    /// 未装配工作集（测试桩）时回退 `session_manager` 直写。
    async fn persist_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<(), ApiError> {
        if let Some(reg) = self.working_sets.as_ref() {
            match reg.ensure(session_id).await {
                Ok(ws) => {
                    ws.rewrite(messages).await?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, session = %session_id, "工作集加载失败，回退直写重写");
                }
            }
        }
        self.session_manager
            .rewrite_messages(session_id, messages)
            .await?;
        Ok(())
    }

    /// 写入收口 ③（ADR-035 §3）：会话头部元数据经工作集读改镜像后落库。
    ///
    /// 相对旧的 `update_session(&session)` 的改进：作用于**库中最新 header
    /// 镜像**，不会用 server 侧读到的旧 header 覆盖并发写入的字段
    /// （如 Agent 刚固化的 `injectable_snapshot`）。
    async fn persist_meta(&self, session: &tianyan::session::Session) -> Result<(), ApiError> {
        if let Some(reg) = self.working_sets.as_ref() {
            match reg.ensure(&session.session_id).await {
                Ok(ws) => {
                    let title = session.title.clone();
                    let ended_at = session.ended_at;
                    let created_at = session.created_at;
                    ws.update_header(|h| {
                        h.title = title;
                        h.ended_at = ended_at;
                        h.created_at = Some(created_at);
                    })
                    .await?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, session = %session.session_id, "工作集加载失败，回退直写头部");
                }
            }
        }
        self.session_manager.update_session(session).await?;
        Ok(())
    }

    /// 写入收口 ③（`working_directory` 变体，ADR-035 §3）。
    async fn persist_working_directory(
        &self,
        session_id: &str,
        working_directory: Option<String>,
    ) -> Result<(), ApiError> {
        if let Some(reg) = self.working_sets.as_ref() {
            match reg.ensure(session_id).await {
                Ok(ws) => {
                    ws.update_header(|h| h.working_directory = working_directory.clone())
                        .await?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = %session_id,
                        "工作集加载失败，回退直写工作目录"
                    );
                }
            }
        }
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {session_id}")))?;
        session.header.working_directory = working_directory;
        self.session_manager.update_session(&session).await?;
        Ok(())
    }

    /// 解析会话生效的工作目录（会话级绑定优先，缺省回退全局配置）。
    fn resolve_workdir(&self, session: &tianyan::session::Session) -> Option<PathBuf> {
        session.working_directory(self.default_working_dir.as_deref())
    }

    /// 列出所有会话
    pub async fn list_sessions(&self) -> Result<ListSessionsResponse, ApiError> {
        // 从会话管理器获取所有会话
        let core_sessions = self.session_manager.list_sessions().await?;

        // 将核心 Session 转换为 API Session
        let sessions: Vec<Session> = core_sessions.iter().map(Session::from_core).collect();

        let total = sessions.len();

        Ok(ListSessionsResponse { sessions, total })
    }

    /// 获取会话详情
    pub async fn get_session_detail(&self, session_id: &str) -> Result<SessionDetail, ApiError> {
        info!("获取会话详情: {}", session_id);

        let core_session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        // 完整转换（与事件订阅快照 `subscribe_events` 同源）：跨消息合并
        // 工具结果（result / duration_ms / error）、Tool→Assistant 映射、
        // 无展示内容的 tool 消息跳过。两条路径必须同源——否则订阅快照
        // 的 replace 会把富消息覆盖成贫消息（工具卡片永久"运行中"）。
        //
        // 携带链上位置（ADR-035 §4）：全量链按 seq 升序，位置即 seq——
        // 前端按 seq 对齐（rewrite 后的重排由 `MessagesRewritten` 事件
        // 整体替换窗口处理）。
        let messages = ChatMessage::messages_from_structured_with_seq(
            core_session
                .messages
                .into_iter()
                .enumerate()
                .map(|(i, m)| (i as i64, m))
                .collect(),
        );

        Ok(SessionDetail {
            id: core_session.session_id,
            title: core_session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: core_session.created_at.to_rfc3339(),
            updated_at: core_session
                .last_message_at
                .or(core_session.ended_at)
                .unwrap_or(core_session.created_at)
                .to_rfc3339(),
            messages,
        })
    }

    /// 获取会话消息
    pub async fn get_messages(
        &self,
        session_id: &str,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!("获取会话消息: {}", session_id);

        let detail = self.get_session_detail(session_id).await?;

        let last_seq = detail.messages.len() as i64 - 1;
        Ok(SessionMessagesResponse {
            session_id: detail.id,
            messages: detail.messages,
            next_before_seq: None,
            has_more: false,
            last_seq,
        })
    }

    /// 分段加载（ADR-035 §8）：只查目标区间，不全量加载——长会话打开/上滚
    /// 不付全链成本。
    ///
    /// `before_seq` 省略 = 最近一页；给出 = 该位置之前一页（上滚游标取上一页的
    /// `next_before_seq`）。每条消息携带 `seq`（链上位置，前端按 seq 对齐）。
    pub async fn get_messages_page(
        &self,
        session_id: &str,
        before_seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<SessionMessagesResponse, ApiError> {
        if !self.session_manager.session_exists(session_id).await? {
            return Err(ApiError::NotFound(format!("会话未找到: {session_id}")));
        }
        let page_limit = limit
            .unwrap_or(DEFAULT_MESSAGE_PAGE)
            .clamp(1, MAX_MESSAGE_PAGE);
        let last_seq = self.session_manager.last_seq(session_id).await?;
        let before = before_seq.unwrap_or(last_seq + 1);
        let rows = self
            .session_manager
            .load_before(session_id, before, page_limit)
            .await?;
        // 上滚游标 = 本页最早一条的 seq（=0 表示已到链首，无更早）
        let next_before_seq = rows.first().map(|(seq, _)| *seq).filter(|seq| *seq > 0);
        Ok(SessionMessagesResponse {
            session_id: session_id.to_string(),
            messages: ChatMessage::messages_from_structured_with_seq(rows),
            next_before_seq,
            has_more: next_before_seq.is_some(),
            last_seq,
        })
    }

    /// 删除消息及其后的所有消息（与编辑/重新生成一致的截断语义）。
    ///
    /// 按消息 ID 定位原始索引后截断——前端展示列表经合并/过滤（tool/system
    /// 消息）后索引与服务端消息列表错位，数字索引会删过头；ID 是稳定定位键。
    pub async fn delete_message(
        &self,
        session_id: &str,
        request: DeleteMessageRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        // 消息 ID → 服务端原始列表索引（快照/截断仍按索引，恢复语义不变）
        let message_index = session
            .messages
            .iter()
            .position(|m| m.id == request.message_id)
            .ok_or_else(|| ApiError::NotFound(format!("消息不存在: {}", request.message_id)))?;
        info!(
            "删除消息: 会话={}, 消息={}, 索引={}",
            session_id, request.message_id, message_index
        );

        // 保存重做状态：当前工作区 + 被截断的消息（按消息 ID 组织；
        // 配置了 working_directory 时快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let truncated: Vec<StructuredMessage> = session.messages[message_index..].to_vec();
            if let Some(workdir) = self.resolve_workdir(&session) {
                if let Err(e) = sm
                    .with_workdir(workdir)
                    .save_redo(session_id, &request.message_id, &truncated)
                    .await
                {
                    tracing::warn!(error = %e, "保存重做状态失败");
                }
            }
        }

        // 截断到该消息之前（不含）
        session.messages.truncate(message_index);

        // 写侧收口（ADR-035 §3）：经工作集（②整表重写 + ③头部）——
        // 单一写入口，缓存与库同步推进
        self.persist_meta(&session).await?;
        self.persist_messages(session_id, &session.messages).await?;

        // 回退工作区文件到该消息处理前的快照（配置了 working_directory 时生效；
        // 快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let Some(workdir) = self.resolve_workdir(&session) else {
                return self.get_messages(session_id).await;
            };
            match sm
                .with_workdir(workdir)
                .restore(session_id, &request.message_id)
                .await
            {
                Ok(n) => {
                    info!(
                        "回退工作区: 会话={}, 锚点={}, 恢复 {} 个文件",
                        session_id, request.message_id, n
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = %session_id,
                        anchor = %request.message_id,
                        "工作区快照恢复失败（会话已回退，文件未回退）"
                    );
                }
            }
        }

        // 返回剩余消息，前端可直接替换本地状态
        self.get_messages(session_id).await
    }

    /// 重做：恢复被回退的消息与工作区文件。
    pub async fn redo_message(
        &self,
        session_id: &str,
        request: RedoRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!("重做消息: 会话={}, 消息={}", session_id, request.message_id);

        // 0. 存在性检查：不存在的会话 → 404（优先于重做状态检查）；
        //    同时解析当前会话生效的工作目录（快照根，会话级绑定优先）
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;
        let workdir = self
            .resolve_workdir(&session)
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?;

        // 1. 恢复工作区文件 + 取出被截断的消息（快照管理器一次性语义，
        //    重做数据按被回退消息的 ID 组织）
        let Some((messages, restored)) = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?
            .with_workdir(workdir)
            .load_redo(session_id, &request.message_id)
            .await?
        else {
            return Err(ApiError::BadRequest("没有可重做的状态".to_string()));
        };

        // 2. 恢复会话消息
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        for msg in messages {
            session.add_structured_message(msg);
        }
        // 写侧收口（ADR-035 §3）：经工作集（②整表重写 + ③头部）
        self.persist_meta(&session).await?;
        self.persist_messages(session_id, &session.messages).await?;

        tracing::info!(session = %session_id, restored, "重做完成");
        self.get_messages(session_id).await
    }

    /// 删除会话
    pub async fn delete_session(
        &self,
        session_id: &str,
    ) -> Result<DeleteSessionResponse, ApiError> {
        info!("删除会话: {}", session_id);

        // 存在性检查：不存在的会话删除 → 404（幂等语义，避免 500）
        let exists = self
            .session_manager
            .get_session(session_id)
            .await?
            .is_some();
        if !exists {
            return Err(ApiError::NotFound(format!("会话未找到: {}", session_id)));
        }

        // 写侧收口 ④（ADR-035 §3）：经工作集删除 + 注册表移除（缓存不残留）。
        let mut deleted_via_ws = false;
        if let Some(reg) = self.working_sets.as_ref() {
            if let Ok(ws) = reg.ensure(session_id).await {
                ws.delete().await?;
                reg.remove(session_id).await;
                deleted_via_ws = true;
            }
        }
        if !deleted_via_ws {
            self.session_manager.delete_session(session_id).await?;
        }

        Ok(DeleteSessionResponse {
            success: true,
            message: format!("会话 {} 删除成功", session_id),
        })
    }

    /// 更新会话标题
    pub async fn update_title(
        &self,
        session_id: &str,
        request: UpdateTitleRequest,
    ) -> Result<Session, ApiError> {
        info!("更新会话标题: {} -> {}", session_id, request.title);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        session.title = Some(request.title);

        // 写侧收口 ③（ADR-035 §3）：经工作集头部镜像
        self.persist_meta(&session).await?;

        Ok(Session::from_core(&session))
    }

    /// 更新会话绑定的工作目录（工作区归属；空串清除绑定）。
    ///
    /// 目录必须存在，否则 400。
    pub async fn update_workspace(
        &self,
        session_id: &str,
        working_directory: &str,
    ) -> Result<Session, ApiError> {
        info!("更新会话工作目录: {} -> {}", session_id, working_directory);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        let trimmed = working_directory.trim();
        if trimmed.is_empty() {
            session.header.working_directory = None;
        } else {
            if !std::path::Path::new(trimmed).is_dir() {
                return Err(ApiError::BadRequest(format!("目录不存在：{trimmed}")));
            }
            session.header.working_directory = Some(trimmed.to_string());
        }
        // 写侧收口 ③（ADR-035 §3）：working_directory 属头部字段（经工作集镜像）
        self.persist_working_directory(
            &session.session_id,
            session.header.working_directory.clone(),
        )
        .await?;

        Ok(Session::from_core(&session))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tianyan::common::types::{MessageRole, Part, PartTime, StructuredMessage};
    use tianyan::session::{Session, SessionManager};

    use crate::api::sessions::services::SessionService;

    /// 内存会话管理器 mock（get_session_detail 回归测试用）。
    struct MockSessionManager {
        sessions: Mutex<HashMap<String, Session>>,
    }

    impl MockSessionManager {
        fn with_sessions(sessions: Vec<Session>) -> Self {
            let map = sessions
                .into_iter()
                .map(|s| (s.session_id.clone(), s))
                .collect();
            Self {
                sessions: Mutex::new(map),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionManager for MockSessionManager {
        async fn create_session(
            &self,
            id: &str,
            _message: tianyan::Message,
        ) -> tianyan::Result<Session> {
            let session = Session::new(id);
            self.sessions
                .lock()
                .unwrap()
                .insert(id.to_string(), session.clone());
            Ok(session)
        }

        async fn get_session(&self, id: &str) -> tianyan::Result<Option<Session>> {
            Ok(self.sessions.lock().unwrap().get(id).cloned())
        }

        async fn update_session(&self, session: &Session) -> tianyan::Result<()> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id.clone(), session.clone());
            Ok(())
        }

        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> tianyan::Result<()> {
            Ok(())
        }

        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> tianyan::Result<()> {
            Ok(())
        }

        async fn list_sessions(&self) -> tianyan::Result<Vec<Session>> {
            Ok(self.sessions.lock().unwrap().values().cloned().collect())
        }

        async fn delete_session(&self, id: &str) -> tianyan::Result<()> {
            self.sessions.lock().unwrap().remove(id);
            Ok(())
        }
    }

    #[test]
    fn test_session_service_creation() {
        // 编译时测试
    }

    /// 回归：历史消息结构化转换 —— 工具调用/结果独立字段输出，
    /// 工具结果保持完整内容不截断（多字节 UTF-8 中文内容完整保留，
    /// 且不再有按字节切片导致的 panic，此前会导致 GET /sessions/{id}/messages 500）。
    #[tokio::test]
    async fn test_get_session_detail_structured_tool_parts_with_multibyte_content() {
        let chinese_result = "中文结果".repeat(300); // 900 字符 ≈ 2700 字节
        let chinese_args = "中文参数".repeat(150); // 450 字符 ≈ 1350 字节
        let msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: chinese_args,
                    time: PartTime::default(),
                },
                Part::ToolResult {
                    tool_call_id: "call_1".to_string(),
                    content: chinese_result,
                    error: None,
                    time: PartTime::default(),
                },
            ],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
            None,
        );

        // 不 panic；结构化输出：正文不含工具文本，工具调用与结果按 id 合并
        // 进同一张卡片，工具结果保持完整内容（不做展示层截断）
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");
        let msg = &detail.messages[0];
        assert!(
            msg.content.is_none(),
            "响应方向无纯文本字段（正文在 segments）"
        );
        assert!(msg.thinking.is_none(), "无思考 parts 时 thinking 为空");

        let calls = msg.tool_calls.as_ref().expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1, "调用与结果合并为一张卡片");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments.len(), 1800, "参数保持完整不截断");
        assert_eq!(calls[0].presentation, "read", "展示意图来自 core 默认映射");
        let result = calls[0].result.as_ref().expect("结果应合并进调用卡片");
        assert_eq!(result.len(), 3600, "工具结果保持完整内容（不截断）");
        assert!(result.starts_with("中文结果"), "中文内容完整保留");
    }

    /// 孤立工具结果（无对应 ToolCall part）仍以结果-only 卡片保留。
    #[tokio::test]
    async fn test_get_session_detail_keeps_orphan_tool_result() {
        let msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_x".to_string(),
                content: "孤立结果".to_string(),
                error: None,
                time: PartTime::default(),
            }],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");
        let calls = detail.messages[0]
            .tool_calls
            .as_ref()
            .expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_x");
        assert_eq!(calls[0].name, "", "孤立结果无工具名");
        assert_eq!(calls[0].result.as_deref(), Some("孤立结果"));
    }

    /// 跨消息合并：工具结果位于独立的 role=tool 消息（工具执行后单独
    /// 持久化），应上挂到前一条 assistant 消息的调用卡片，且原 tool 消息
    /// 不再产生空气泡。
    #[tokio::test]
    async fn test_get_session_detail_merges_result_across_messages() {
        let call_msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Text {
                    text: "先看看目录".to_string(),
                    time: PartTime::default(),
                },
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "list_dir".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let result_msg = StructuredMessage {
            id: "msg_2".to_string(),
            parent_id: Some("msg_1".to_string()),
            role: MessageRole::Tool,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: r#"{"count":2}"#.to_string(),
                error: None,
                time: PartTime::default(),
            }],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(call_msg);
        session.messages.push(result_msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");

        // 只有 1 条消息（role=tool 消息被合并消费后跳过）
        assert_eq!(detail.messages.len(), 1, "tool 结果消息应被合并后跳过");
        let msg = &detail.messages[0];
        // 正文在 segments 的 Text 段（响应方向无纯文本字段）
        let text: Vec<&str> = msg
            .segments
            .as_ref()
            .map(|s| {
                s.iter()
                    .filter_map(|x| match x {
                        crate::api::shared::types::MessageSegment::Text { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(text, vec!["先看看目录"]);
        let calls = msg.tool_calls.as_ref().expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].presentation, "search");
        assert_eq!(
            calls[0].result.as_deref(),
            Some(r#"{"count":2}"#),
            "结果应跨消息合并进调用卡片"
        );
    }

    /// 回归：assistant 空消息（唤醒轮空输出）必须保留——前端唤醒轮询以
    /// "出现新的 assistant 消息"为停止信号，过滤掉会导致轮询永不停止
    /// （指示器卡死）。渲染层 MessageBubble 已跳过空消息，保留不影响展示。
    #[tokio::test]
    async fn test_get_session_detail_keeps_empty_assistant() {
        let empty_assistant = StructuredMessage {
            id: "msg_empty".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(empty_assistant);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");

        assert_eq!(
            detail.messages.len(),
            1,
            "assistant 空消息应保留（前端轮询停止信号）"
        );
        assert_eq!(detail.messages[0].role, MessageRole::Assistant);
        assert!(
            detail.messages[0].content.is_none(),
            "空 assistant 消息：无纯文本字段"
        );
    }

    /// 分段加载（ADR-035 §8）：游标逐页走完整链 → 拼接与全量一致（无缺、无重、
    /// 顺序正确），且每页消息携带各自链上 seq。
    #[tokio::test]
    async fn test_get_messages_page_cursor_walks_full_chain() {
        let mut session = Session::new("s1");
        for i in 0..5 {
            session.add_structured_message(StructuredMessage::user("s1", format!("m{i}")));
        }
        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
            None,
        );

        // 第一页（无游标 = 最近一页）：seq 3,4
        let p1 = service
            .get_messages_page("s1", None, Some(2))
            .await
            .expect("第一页");
        assert_eq!(p1.last_seq, 4);
        assert_eq!(p1.messages.len(), 2);
        assert_eq!(p1.messages[0].seq, Some(3), "每条消息携带链上位置");
        assert_eq!(p1.messages[1].seq, Some(4));
        assert!(p1.has_more);
        assert_eq!(p1.next_before_seq, Some(3), "游标 = 本页最早一条的 seq");

        // 第二页：seq 1,2
        let p2 = service
            .get_messages_page("s1", p1.next_before_seq, Some(2))
            .await
            .expect("第二页");
        assert_eq!(
            p2.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
        assert_eq!(p2.next_before_seq, Some(1));

        // 第三页：seq 0（到链首 → 无更早）
        let p3 = service
            .get_messages_page("s1", p2.next_before_seq, Some(2))
            .await
            .expect("第三页");
        assert_eq!(p3.messages.len(), 1);
        assert_eq!(p3.messages[0].seq, Some(0));
        assert!(!p3.has_more, "到链首后无更早历史");
        assert_eq!(p3.next_before_seq, None);

        // 拼接（由早到晚）== 全量
        let full = service.get_messages("s1").await.expect("全量");
        let mut walked: Vec<Option<i64>> = Vec::new();
        walked.extend(p3.messages.iter().map(|m| m.seq));
        walked.extend(p2.messages.iter().map(|m| m.seq));
        walked.extend(p1.messages.iter().map(|m| m.seq));
        let full_seqs: Vec<Option<i64>> = full.messages.iter().map(|m| m.seq).collect();
        assert_eq!(walked, full_seqs, "分页拼接应与全量一致（无缺无重）");
    }

    /// 分段加载：不存在的会话 → 404（与全量路径一致）。
    #[tokio::test]
    async fn test_get_messages_page_missing_session_not_found() {
        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![])),
            None,
            None,
            None,
        );
        let err = service
            .get_messages_page("ghost", None, Some(10))
            .await
            .unwrap_err();
        assert!(
            matches!(err, crate::api::shared::error::ApiError::NotFound(_)),
            "不存在的会话应 404: {err:?}"
        );
    }

    /// 写侧收口端到端（ADR-035 §3）：API 写路径经工作集后，
    /// **缓存与库同步推进**（不再依赖 ensure 校验兜底）。
    #[tokio::test]
    async fn test_delete_message_via_working_set_keeps_cache_consistent() {
        use tianyan::agent::working_set::WorkingSetRegistry;
        use tianyan::session::store::SessionStore;
        use tianyan::session::SessionHeader;

        let db = tianyan::db::Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let store = SessionStore::new(db).unwrap();
        store.create("s1", &SessionHeader::default()).await.unwrap();
        for i in 0..3 {
            store
                .append_message("s1", &StructuredMessage::user("s1", format!("m{i}")))
                .await
                .unwrap();
        }
        let manager: Arc<dyn SessionManager> = Arc::new(
            tianyan::session::PersistentSessionManager::new(store.clone()),
        );
        let registry = WorkingSetRegistry::new(Some(store.clone()));
        // 预加载（模拟会话正在被使用）；先缓存一条"脏"状态以验证收口会同步推进
        let ws = registry.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 2);

        let service = SessionService::new(manager, None, None, Some(registry.clone()));
        // 删除第 2 条消息（seq=1）→ 截断到它之前，剩 1 条
        let (_, before) = store.load("s1").await.unwrap().unwrap();
        let target_id = before[1].id.clone();
        service
            .delete_message(
                "s1",
                crate::api::sessions::types::DeleteMessageRequest {
                    message_id: target_id,
                },
            )
            .await
            .expect("删除消息");

        // 缓存与库一致（同一工作集实例，未经重建路径）
        assert_eq!(ws.last_seq(), 0, "工作集库尾同步前移");
        assert_eq!(ws.message_count().await, 1, "工作集缓存不残留被截断消息");
        let (_, stored) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(stored.len(), 1, "库侧一致");
    }
}
