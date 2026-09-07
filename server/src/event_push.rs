//! 统一事件推送（ADR-028/031）：会话管理器推送 wrapper。
//!
//! - [`BroadcastingSessionManager`]：`SessionManager` trait 的包装——
//!   ADR-031 后消息**不再广播**（落库即广播移除）：主会话消息经流式增量 +
//!   完成事件（user_message_id 确认 / pump 边界）到前端；后台任务完成
//!   通知只落库（LLM 上下文），唤醒轮流式化后输出经 chat_stream 推送；
//!   子智能体消息经 chat_stream 事件（session_id=task_id）路由到面板。
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
#[derive(Clone)]
pub struct BroadcastingSessionManager {
    inner: Arc<dyn SessionManager>,
}

impl BroadcastingSessionManager {
    /// 创建推送 wrapper。
    pub fn new(inner: Arc<dyn SessionManager>) -> Self {
        Self { inner }
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
        self.inner.add_structured_message(session_id, msg).await
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
