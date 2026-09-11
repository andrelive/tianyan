//! 流式事件统一转发（ADR-032）。
//!
//! 一轮流式输出（用户轮 / 唤醒轮 / 子代理）的"最后一公里"接线单点：
//! 建通道 → **立即启动消费任务** → 返回 sender 交给轮。
//!
//! # 不变量（防死锁）
//!
//! 通道必须先于轮启动创建并立即开始消费——调用方先调用
//! [`spawn_stream_forwarder`] 拿到 sender，再把它交给轮
//! （`process_message_stream` / `process_wake` / `run_stream`）。若反过来
//! （先跑轮、后消费），输出积压到轮结束才转发，超过通道缓冲还会死锁
//! （发送端等消费、消费方等轮结束）——历史 bug（1151680）即此形态；
//! 三处接线收敛到本模块后，该形态不可能再写出来。
//!
//! # 职责边界
//!
//! - 本模块负责：通道创建（统一容量）、消费循环、`type=chat_stream` /
//!   `session_id` 字段注入（[`inject_stream_event_fields`]）、错误 chunk
//!   防御性跳过、rx 关闭即消费任务结束（调用方 await 句柄 = 等待流排空）。
//! - 调用方负责：映射函数（[`StreamEventMapper`]：chunk → 事件 JSON；
//!   协议差异在此可插拔）、送达目标（[`StreamEventDeliver`]）、消费结束
//!   后的收尾（如用户轮的 assistant 消息边界事件）。
//!
//! # 映射器为何可插拔
//!
//! 用户轮 / 唤醒轮共用 server 的 `map_chunk_to_event`（依赖 server 展示
//! 类型 `ChatMessage` 的转换，无法下沉 core）；子代理使用 core 侧精简
//! 映射（`chunk_to_stream_json`）。映射器差异是**有意保留**的 seam，
//! 转发骨架（本模块）则是三处共用的唯一实现。

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::agent::background::TaskEventSink;
use crate::agent::types::{AgentStreamChunk, StreamEventSender};
use crate::common::error::TianyanError;

/// 流式转发通道缓冲（统一容量；历史各处 64/100 混用）。
pub const STREAM_FORWARD_BUFFER: usize = 100;

/// 流式事件映射函数：`(session_id, chunk) → 事件 JSON`（`None` = 不转发该 chunk）。
///
/// session_id 传入供映射器填写（如 `map_chunk_to_event` 需要）；转发器随后
/// 仍会统一注入 `type` / `session_id`（幂等，见 [`inject_stream_event_fields`]）。
pub type StreamEventMapper =
    Arc<dyn Fn(&str, AgentStreamChunk) -> Option<serde_json::Value> + Send + Sync>;

/// 流式事件送达目标（统一转发器与宿主之间的 seam）。
///
/// 实现方决定事件去向：主会话 / 唤醒轮 → [`BroadcastJsonDeliver`]（统一
/// 事件通道，SSE 广播源）；子代理 → [`TaskSinkDeliver`]（任务面板）。
#[async_trait::async_trait]
pub trait StreamEventDeliver: Send + Sync {
    /// 送达一个事件 JSON。
    async fn deliver(&self, session_id: &str, event: serde_json::Value);
}

/// 统一事件通道适配器（用户轮 / 唤醒轮：broadcast JSON 下发）。
///
/// 无订阅者时静默丢弃（broadcast 语义）；事件 JSON 已含 `type` /
/// `session_id`（转发器注入），此处直接序列化下发。
pub struct BroadcastJsonDeliver {
    tx: broadcast::Sender<String>,
}

impl BroadcastJsonDeliver {
    /// 创建适配器（`tx` 为统一事件通道发送端）。
    pub fn new(tx: broadcast::Sender<String>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl StreamEventDeliver for BroadcastJsonDeliver {
    async fn deliver(&self, _session_id: &str, event: serde_json::Value) {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = self.tx.send(json);
        }
    }
}

/// 任务事件通道适配器（子代理路径：`TaskEventSink` → [`StreamEventDeliver`]）。
pub struct TaskSinkDeliver(pub Arc<dyn TaskEventSink>);

#[async_trait::async_trait]
impl StreamEventDeliver for TaskSinkDeliver {
    async fn deliver(&self, session_id: &str, event: serde_json::Value) {
        self.0.emit(session_id, event).await;
    }
}

/// 空送达（未装配事件通道的路径：消费但不送达，防发送端阻塞）。
pub struct NullDeliver;

#[async_trait::async_trait]
impl StreamEventDeliver for NullDeliver {
    async fn deliver(&self, _session_id: &str, _event: serde_json::Value) {}
}

/// 注入流式事件公共字段（`type=chat_stream` + `session_id`）——单点。
///
/// 所有经统一事件通道下发的流式事件必须携带这两个字段（前端按
/// `session_id` 路由、按 `type` 分发）；转发器内部与边界事件等手工事件
/// 共用本函数，避免各处手写 `payload["type"] = ...` 漂移。
pub fn inject_stream_event_fields(payload: &mut serde_json::Value, session_id: &str) {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("type".to_string(), serde_json::json!("chat_stream"));
        obj.insert("session_id".to_string(), serde_json::json!(session_id));
    }
}

/// 创建一轮流式转发：建通道 + 立即启动消费任务，返回 `(sender, 消费任务句柄)`。
///
/// - `session_id`：事件路由标识（`type` / `session_id` 字段由转发器单点注入）；
/// - `mapper`：chunk → 事件 JSON（返回 `None` 表示该 chunk 不转发）；
/// - `deliver`：事件 JSON 的送达目标。
///
/// 调用方必须在启动轮**之前**调用本函数（见模块文档"不变量"）；轮结束后
/// `await` 返回的句柄可确保所有事件已送达（rx 关闭 = 轮内所有 sender 已
/// 释放 = 轮结束）。
pub fn spawn_stream_forwarder(
    session_id: impl Into<String>,
    mapper: StreamEventMapper,
    deliver: Arc<dyn StreamEventDeliver>,
) -> (StreamEventSender, JoinHandle<()>) {
    let session_id = session_id.into();
    let (tx, mut rx) =
        mpsc::channel::<Result<AgentStreamChunk, TianyanError>>(STREAM_FORWARD_BUFFER);
    let handle = tokio::spawn(async move {
        while let Some(chunk_result) = rx.recv().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(e) => {
                    // 防御性分支：StreamEventSender 只发 Ok（错误经 Error chunk
                    // 表达），此处兜底跳过——不中断消费，避免个别异常卡死整条流。
                    tracing::debug!(error = %e, "流式事件通道异常（防御性跳过）");
                    continue;
                }
            };
            if let Some(mut payload) = mapper(&session_id, chunk) {
                inject_stream_event_fields(&mut payload, &session_id);
                deliver.deliver(&session_id, payload).await;
            }
        }
    });
    (StreamEventSender::new(tx), handle)
}

/// 创建"只消费不送达"的转发器（无事件通道的路径：唤醒轮本身正常执行，
/// 事件静默丢弃——但消费端必须存活，防发送端填满缓冲后阻塞）。
pub fn spawn_null_forwarder(session_id: impl Into<String>) -> (StreamEventSender, JoinHandle<()>) {
    spawn_stream_forwarder(session_id, Arc::new(|_, _| None), Arc::new(NullDeliver))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::StreamChunkType;
    use std::sync::Mutex;
    use std::time::Duration;

    /// 记录送达事件的 mock 送达目标。
    struct RecordingDeliver {
        events: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl RecordingDeliver {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn count(&self) -> usize {
            self.events.lock().unwrap().len()
        }

        fn take(&self) -> Vec<(String, serde_json::Value)> {
            self.events.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait::async_trait]
    impl StreamEventDeliver for RecordingDeliver {
        async fn deliver(&self, session_id: &str, event: serde_json::Value) {
            self.events
                .lock()
                .unwrap()
                .push((session_id.to_string(), event));
        }
    }

    /// 简单映射器：answer chunk → `{"delta": ...}`；其余类型返回 None。
    fn answer_mapper() -> StreamEventMapper {
        Arc::new(|session_id, chunk| {
            if chunk.chunk_type == StreamChunkType::Answer {
                Some(serde_json::json!({ "delta": chunk.delta, "session_id": session_id }))
            } else {
                None
            }
        })
    }

    #[tokio::test]
    async fn test_forwarder_maps_delivers_and_injects_fields() {
        let deliver = Arc::new(RecordingDeliver::new());
        let (sender, handle) =
            spawn_stream_forwarder("session-1", answer_mapper(), deliver.clone());

        sender.send_answer_delta("你好").await;
        sender.send_answer_delta("世界").await;
        // 非 Answer 类型（Thought）→ mapper 返回 None → 不送达
        sender.send_thought("思考").await;
        drop(sender);
        let _ = handle.await;

        let events = deliver.take();
        assert_eq!(events.len(), 2, "Answer 事件应送达；Thought 被映射器过滤");
        for (sid, ev) in &events {
            assert_eq!(sid, "session-1");
            // 字段注入单点：type + session_id
            assert_eq!(ev["type"], "chat_stream");
            assert_eq!(ev["session_id"], "session-1");
        }
        assert_eq!(events[0].1["delta"], "你好");
        assert_eq!(events[1].1["delta"], "世界");
    }

    #[tokio::test]
    async fn test_forwarder_delivers_while_turn_running() {
        // 防死锁回归：事件必须在**轮进行中**送达（而非积压到轮结束）。
        // 发送首 chunk 后 sender 不释放（轮仍在跑），消费端应立刻收到。
        let deliver = Arc::new(RecordingDeliver::new());
        let (sender, handle) =
            spawn_stream_forwarder("session-1", answer_mapper(), deliver.clone());

        sender.send_answer_delta("首块").await;
        // 轮仍在进行（sender 未 drop）：消费端 1s 内必须收到
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while deliver.count() == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(deliver.count(), 1, "事件应在轮进行中实时送达（1s 内）");
        assert!(!handle.is_finished(), "此时消费任务应仍在运行（轮未结束）");

        drop(sender);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_forwarder_handle_completes_after_sender_drop() {
        let deliver = Arc::new(RecordingDeliver::new());
        let (sender, handle) =
            spawn_stream_forwarder("session-1", answer_mapper(), deliver.clone());

        assert!(!handle.is_finished(), "sender 存活时消费任务不应结束");
        drop(sender);
        // 句柄在 1s 内完成（rx 关闭 → 循环退出）
        let completed = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(completed.is_ok(), "sender 释放后消费任务应结束");
    }

    #[tokio::test]
    async fn test_null_forwarder_drains_without_delivery() {
        // 无事件通道路径：消费端存活（不阻塞发送端），无送达副作用。
        let (sender, handle) = spawn_null_forwarder("session-1");
        for _ in 0..(STREAM_FORWARD_BUFFER + 10) {
            sender.send_answer_delta("x").await;
        }
        drop(sender);
        let completed = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(completed.is_ok(), "NullDeliver 应持续消费直到 sender 释放");
    }

    #[test]
    fn test_inject_stream_event_fields() {
        let mut v = serde_json::json!({"delta": "x", "session_id": "old"});
        inject_stream_event_fields(&mut v, "session-9");
        assert_eq!(v["type"], "chat_stream");
        assert_eq!(v["session_id"], "session-9");
        // 非对象载荷：原样返回（不 panic）
        let mut plain = serde_json::json!("plain");
        inject_stream_event_fields(&mut plain, "session-9");
        assert_eq!(plain, serde_json::json!("plain"));
    }

    #[tokio::test]
    async fn test_broadcast_json_deliver_sends_json() {
        let (tx, mut rx) = broadcast::channel::<String>(4);
        let deliver = BroadcastJsonDeliver::new(tx);
        deliver.deliver("s1", serde_json::json!({"a": 1})).await;
        let got = rx.recv().await.expect("应收到广播 JSON");
        let v: serde_json::Value = serde_json::from_str(&got).expect("应为合法 JSON");
        assert_eq!(v["a"], 1);
    }
}
