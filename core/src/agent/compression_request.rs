//! 上下文压缩请求：模型主动「压一下再继续」的**交接条**。
//!
//! 为什么需要这个中间层（而不是工具直接调压缩）：压缩会改写会话链（插压缩点 +
//! 收缩工作集段），必须发生在**轮末的安全点**——正在跑的 `AgentLoop` 手里是
//! `Vec<Message>` 快照，轮内压缩对它不生效（下一轮请求仍用旧快照，等于白压）。
//! 所以工具只登记「我要压缩 + 交接说明」，由 AgentLoop 在工具执行后**收尾本轮**，
//! 再由 Agent 在轮末真正压缩、决定是否唤醒继续。
//!
//! 队列按会话分条（`ToolContext` 全局共享一份）；同会话重复登记**以后者为准**
//! ——模型可能改主意，取最新意图即可。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// 一次主动压缩请求。
#[derive(Debug, Clone)]
pub struct CompressionRequest {
    /// 交接说明（"接下来要做什么 / 已完成什么 / 注意什么"）。
    ///
    /// 由模型在 `compress_context(handoff: ...)` 里给；压缩时作为**必须保留项**
    /// 拼进摘要提示，避免压缩后"忘了自己在干什么"。
    pub handoff: Option<String>,
    /// 登记时刻（轮末消费时用于判断请求是否过期）。
    pub requested_at: Instant,
}

/// 按会话的压缩请求队列（`ToolContext` 与 Agent 共享同一份 Arc）。
///
/// 用 `std::sync::Mutex`：临界区只有一次 HashMap 读写，且 `AgentLoop` 的
/// 「本轮要不要收尾」是同步判据（在循环里），异步锁反而要额外 await。
#[derive(Debug, Default)]
pub struct CompressionRequests {
    inner: Mutex<HashMap<String, CompressionRequest>>,
}

impl CompressionRequests {
    /// 建空队列。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一次请求（同会话覆盖旧请求，取最新意图）。
    pub fn request(&self, session_id: &str, handoff: Option<String>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(
            session_id.to_string(),
            CompressionRequest {
                handoff,
                requested_at: Instant::now(),
            },
        );
    }

    /// 该会话是否有待处理的压缩请求（`AgentLoop` 的收尾判据）。
    pub fn is_pending(&self, session_id: &str) -> bool {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.contains_key(session_id)
    }

    /// 取出并清除（轮末消费；`None` = 没有待处理请求）。
    pub fn take(&self, session_id: &str) -> Option<CompressionRequest> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(session_id)
    }

    /// 丢弃请求（压缩失败 / 保险丝触发时用，避免下一轮又捡到旧请求）。
    pub fn discard(&self, session_id: &str) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_then_take_clears() {
        let q = CompressionRequests::new();
        assert!(!q.is_pending("s1"));
        q.request("s1", Some("继续做 X".to_string()));
        assert!(q.is_pending("s1"));
        let req = q.take("s1").expect("应取到请求");
        assert_eq!(req.handoff.as_deref(), Some("继续做 X"));
        assert!(!q.is_pending("s1"), "取走即清空，避免下一轮重复压缩");
    }

    #[test]
    fn later_request_replaces_earlier() {
        let q = CompressionRequests::new();
        q.request("s1", Some("旧的".to_string()));
        q.request("s1", Some("新的".to_string()));
        let req = q.take("s1").expect("应取到请求");
        assert_eq!(req.handoff.as_deref(), Some("新的"), "同会话取最新意图");
    }

    #[test]
    fn sessions_do_not_interfere() {
        let q = CompressionRequests::new();
        q.request("s1", None);
        assert!(q.is_pending("s1"));
        assert!(!q.is_pending("s2"), "别的会话不该看到这条请求");
        q.discard("s1");
        assert!(!q.is_pending("s1"));
    }

    #[test]
    fn take_missing_is_none() {
        let q = CompressionRequests::new();
        assert!(q.take("nope").is_none());
    }
}
