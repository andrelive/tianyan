//! 统一事件推送（ADR-028）：落库即推送 + 全局单连接 SSE。
//!
//! - [`BroadcastingSessionManager`]：`SessionManager` trait 的推送 wrapper——
//!   落库成功后把同一消息广播到统一事件通道（core 不感知，SessionStore 纯库）。
//! - 事件契约：`{type: "message", session_id, seq, message}`——seq 即
//!   `SessionStore` 原子取号的持久化序号（每会话独立、单调递增），
//!   是前端完整性检测锚点（断点对齐）。
//! - 通道：复用 `task_event_tx`（broadcast）——任务状态/命令输出事件
//!   （core 已 emit）与消息事件共用同一通道，前端按 `type` 区分。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use tianyan::common::error::Result;
use tianyan::common::types::{Message, MessageRole, StructuredMessage};
use tianyan::session::store::SessionStore;
use tianyan::session::{Session, SessionManager};

use crate::api::shared::types::ChatMessage;

// 跳过 role=Tool 的独立消息：工具结果在历史加载中合并进前一条 assistant
// 调用卡片（跨消息合并），单独广播会产生空气泡。空 assistant 消息保留
// （前端唤醒轮以"最后一条是 assistant"为停止信号，与历史加载语义一致）。
fn broadcastable(msg: &StructuredMessage) -> bool {
    msg.role != MessageRole::Tool
}

/// 会话管理器推送 wrapper：落库成功后广播消息事件。
///
/// 只包装 `add_structured_message`（所有消息的唯一入口：用户消息/工具结果/
/// 后台通知/唤醒轮输出/压缩摘要均经此落库）；其余方法原样委托。
/// 广播失败静默（通道是尽力而为的通知，可靠性由数据库保证）。
#[derive(Clone)]
pub struct BroadcastingSessionManager {
    inner: Arc<dyn SessionManager>,
    store: Arc<SessionStore>,
    tx: tokio::sync::broadcast::Sender<String>,
}

impl BroadcastingSessionManager {
    /// 创建推送 wrapper。
    pub fn new(
        inner: Arc<dyn SessionManager>,
        store: Arc<SessionStore>,
        tx: tokio::sync::broadcast::Sender<String>,
    ) -> Self {
        Self { inner, store, tx }
    }

    /// 落库成功后广播消息事件（尽力而为：失败静默，前端断点对齐兜底）。
    async fn broadcast_message(&self, session_id: &str, msg: &StructuredMessage) {
        // 取持久化 seq（断点对齐锚点）；失败则跳过广播（消息已落库，前端可 fetch）
        let Ok(seq) = self.store.last_seq(session_id).await else {
            return;
        };
        let payload = json!({
            "type": "message",
            "session_id": session_id,
            "seq": seq,
            // 广播 ChatMessage（API 展示格式，与历史加载/流式边界同构）：
            // StructuredMessage 是存储格式（parts 数组），前端渲染访问
            // content/segments/tool_calls 会 undefined 崩溃。
            "message": ChatMessage::from_structured_light(msg),
        });
        if let Ok(json) = serde_json::to_string(&payload) {
            let _ = self.tx.send(json);
        }
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
        self.inner.add_structured_message(session_id, msg.clone()).await?;
        if broadcastable(&msg) {
            self.broadcast_message(session_id, &msg).await;
        }
        Ok(())
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
