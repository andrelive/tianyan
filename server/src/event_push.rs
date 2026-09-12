//! 统一事件推送（ADR-028/031）：会话管理器推送 wrapper。
//!
//! - [`BroadcastingSessionManager`]：`SessionManager` trait 的包装——
//!   ADR-031 后消息**不再广播**（落库即广播移除）：主会话消息经流式增量 +
//!   完成事件（user_message_id 确认 / pump 边界）到前端；后台任务完成
//!   通知只落库（LLM 上下文），唤醒轮流式化后输出经 chat_stream 推送；
//!   子智能体消息经 chat_stream 事件（session_id=task_id）路由到面板。
//!   **System 消息统一推送**：后台任务/命令完成通知、定时提醒、**压缩摘要**
//!   等 System 消息落库后补发 chat_stream 边界事件（chunk_type=message）——
//!   与用户消息边界事件同构，前端 applyServerMessage 按 id 查重追加，实时
//!   可见（历史加载与实时流同一份数据、同一条渲染路径）。压缩点是会话
//!   时序链上的普通节点（统一结构，不做区分）：推送与快照/历史按 id 幂等
//!   收敛，不产生重复。
//!   快照恢复（订阅端点）与断点对齐（fetch）是权威兜底。
//! - 通道：`task_event_tx`（broadcast）仍承载任务状态/命令输出事件
//!   （core 已 emit）与流式事件（chat_stream），前端按 `type` 区分。

use std::sync::Arc;

use async_trait::async_trait;

use tianyan::common::error::Result;
use tianyan::common::types::{Message, StructuredMessage};
use tianyan::session::{Session, SessionManager};

/// 会话管理器推送 wrapper（ADR-031：纯委托——消息不再广播）。
///
/// 只包装 `add_structured_message`（所有消息的唯一入口：用户消息/工具结果/
/// 后台通知/唤醒轮输出/压缩摘要均经此落库）；其余方法原样委托。
/// System 消息（后台任务/命令完成、定时提醒、压缩摘要）落库后补发
/// chat_stream 边界事件——输入类消息"落库 → 推送"统一契约（用户消息有
/// send_message_boundary，System 消息补上等价推送；压缩点与普通消息同构，
/// 不做区分）。
#[derive(Clone)]
pub struct BroadcastingSessionManager {
    inner: Arc<dyn SessionManager>,
    /// 统一事件通道（GET /events 广播源）：System 通知边界事件经此下发。
    event_tx: tokio::sync::broadcast::Sender<String>,
}

impl BroadcastingSessionManager {
    /// 创建推送 wrapper（`event_tx` 为统一事件通道：System 通知边界事件经此下发）。
    pub fn new(
        inner: Arc<dyn SessionManager>,
        event_tx: tokio::sync::broadcast::Sender<String>,
    ) -> Self {
        Self { inner, event_tx }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tianyan::common::types::StructuredMessage;
    use tianyan::session::{Session, SessionManager};

    use super::*;

    /// 记录落库调用的 mock 会话管理器。
    struct RecordingSessionManager {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl RecordingSessionManager {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SessionManager for RecordingSessionManager {
        async fn create_session(&self, id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(id))
        }

        async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
            Ok(None)
        }

        async fn update_session(&self, _session: &Session) -> Result<()> {
            Ok(())
        }

        async fn add_structured_message(
            &self,
            session_id: &str,
            msg: StructuredMessage,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((session_id.to_string(), msg.id));
            Ok(())
        }

        async fn add_structured_message_no_fts(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> Result<()> {
            Ok(())
        }

        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> Result<()> {
            Ok(())
        }

        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(Vec::new())
        }

        async fn delete_session(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    /// 从广播通道接收一条事件 JSON（超时兜底）。
    async fn recv_event(
        mut rx: tokio::sync::broadcast::Receiver<String>,
    ) -> Option<serde_json::Value> {
        let json = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .ok()?
            .ok()?;
        serde_json::from_str(&json).ok()
    }

    #[tokio::test]
    async fn system_notification_pushes_chat_stream_event() {
        let (tx, rx) = tokio::sync::broadcast::channel::<String>(16);
        let inner = Arc::new(RecordingSessionManager::new());
        let wrapper = BroadcastingSessionManager::new(inner.clone(), tx);

        let sm = StructuredMessage::system("session-1", "[后台任务完成] test（cmd_1）");
        wrapper
            .add_structured_message("session-1", sm.clone())
            .await
            .unwrap();

        // 落库调用发生
        assert_eq!(inner.calls.lock().unwrap().len(), 1);
        // 事件推送：chat_stream + chunk_type=message + 完整消息结构
        let ev = recv_event(rx).await.expect("应收到 chat_stream 事件");
        assert_eq!(ev["type"], "chat_stream");
        assert_eq!(ev["chunk_type"], "message");
        assert_eq!(ev["session_id"], "session-1");
        assert_eq!(ev["message"]["id"], sm.id);
        assert_eq!(ev["message"]["role"], "system");
        // segments 含通知正文（前端时间线渲染）
        let segments = ev["message"]["segments"].as_array().expect("segments");
        assert_eq!(segments[0]["type"], "text");
        assert!(segments[0]["text"]
            .as_str()
            .unwrap()
            .contains("后台任务完成"));
    }

    #[tokio::test]
    async fn non_system_message_does_not_push() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<String>(16);
        let inner = Arc::new(RecordingSessionManager::new());
        let wrapper = BroadcastingSessionManager::new(inner.clone(), tx);

        let sm = StructuredMessage::user("session-1", "hello");
        wrapper
            .add_structured_message("session-1", sm)
            .await
            .unwrap();

        // 用户消息落库但不推送（主会话流式路径负责推送）
        assert_eq!(inner.calls.lock().unwrap().len(), 1);
        let timeout = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(timeout.is_err(), "非 System 消息不应推送事件");
    }

    #[tokio::test]
    async fn compression_marker_system_pushes_chat_stream_event() {
        // 压缩摘要与普通 System 消息同构推送（统一结构，不做区分）：落库
        // 后补发 chat_stream 边界事件——前端实时可见（压缩点不再需要重载
        // 会话才能发现）；`compression_marker` 字段随消息下发，前端据此
        // 渲染"压缩点"标识。
        let (tx, rx) = tokio::sync::broadcast::channel::<String>(16);
        let inner = Arc::new(RecordingSessionManager::new());
        let wrapper = BroadcastingSessionManager::new(inner.clone(), tx);

        let mut sm = StructuredMessage::system("session-1", "[对话摘要] 早期对话摘要");
        sm.compression_marker = true;
        wrapper
            .add_structured_message("session-1", sm.clone())
            .await
            .unwrap();

        // 落库调用发生
        assert_eq!(inner.calls.lock().unwrap().len(), 1);
        // 事件推送：chat_stream + chunk_type=message + compression_marker 透传
        let ev = recv_event(rx)
            .await
            .expect("压缩摘要应推送 chat_stream 事件");
        assert_eq!(ev["type"], "chat_stream");
        assert_eq!(ev["chunk_type"], "message");
        assert_eq!(ev["session_id"], "session-1");
        assert_eq!(ev["message"]["id"], sm.id);
        assert_eq!(ev["message"]["role"], "system");
        assert_eq!(ev["message"]["compression_marker"], true);
        let segments = ev["message"]["segments"].as_array().expect("segments");
        assert!(segments[0]["text"].as_str().unwrap().contains("对话摘要"));
    }

    #[tokio::test]
    async fn assistant_message_does_not_push() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<String>(16);
        let inner = Arc::new(RecordingSessionManager::new());
        let wrapper = BroadcastingSessionManager::new(inner.clone(), tx);

        let sm = StructuredMessage::assistant("session-1", "回答");
        wrapper
            .add_structured_message("session-1", sm)
            .await
            .unwrap();

        assert_eq!(inner.calls.lock().unwrap().len(), 1);
        let timeout = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(timeout.is_err(), "assistant 消息不应推送事件");
    }
}

#[async_trait]
impl SessionManager for BroadcastingSessionManager {
    async fn create_session(&self, id: &str, message: Message) -> Result<Session> {
        self.inner.create_session(id, message).await
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.inner.get_session(id).await
    }

    async fn update_session(&self, session: &Session) -> Result<()> {
        self.inner.update_session(session).await
    }

    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()> {
        // ADR-031：消息不再广播（落库即广播移除）——主会话消息经流式增量 +
        // 完成事件（user_message_id 确认 / pump 边界）到前端；后台任务完成
        // 通知只落库（LLM 上下文），唤醒轮流式化后输出经 chat_stream 推送。
        // 快照恢复（订阅端点）与断点对齐（fetch）仍是权威兜底。
        self.inner
            .add_structured_message(session_id, msg.clone())
            .await?;
        // System 消息（含压缩摘要）：落库后补发 chat_stream 边界事件（与
        // 用户消息边界事件同构）——前端 applyServerMessage 按 id 查重追加，
        // 实时可见。压缩点是会话时序链上的普通节点（统一结构，不做区分）；
        // 推送与快照/历史按 id 幂等收敛，不产生重复。
        if msg.role == tianyan::common::types::MessageRole::System {
            self.push_system_notification(session_id, &msg);
        }
        Ok(())
    }

    async fn add_structured_message_no_fts(
        &self,
        session_id: &str,
        msg: StructuredMessage,
    ) -> Result<()> {
        // ADR-030/031：子智能体消息落库（不索引 FTS，ADR-026）——不再广播；
        // 任务面板展开 = 订阅快照 + 活跃流 reducer（chat_stream 事件带
        // session_id=task_id 路由）。
        self.inner
            .add_structured_message_no_fts(session_id, msg)
            .await
    }

    async fn rewrite_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()> {
        self.inner.rewrite_messages(session_id, messages).await
    }

    async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.inner.list_sessions().await
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        self.inner.delete_session(id).await
    }
}

impl BroadcastingSessionManager {
    /// 把 System 通知转成 chat_stream 边界事件推入统一事件通道。
    ///
    /// 复用 `map_chunk_to_event`（Message chunk → ChatStreamEvent 同构转换），
    /// 与主会话流式路径同一映射；无订阅者时静默丢弃（broadcast 语义）。
    fn push_system_notification(&self, session_id: &str, msg: &StructuredMessage) {
        let chunk = tianyan::agent::AgentStreamChunk {
            delta: String::new(),
            chunk_type: tianyan::agent::StreamChunkType::Message,
            message: Some(msg.clone()),
            ..Default::default()
        };
        let event =
            crate::api::chat::services::map_chunk_to_event(chunk, "system-notify", session_id, 0);
        if let Ok(mut payload) = serde_json::to_value(&event) {
            payload["type"] = serde_json::json!("chat_stream");
            if let Ok(json) = serde_json::to_string(&payload) {
                let _ = self.event_tx.send(json);
            }
        }
    }
}
