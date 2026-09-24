//! 统一事件推送（ADR-028/031/039）：边界消息推送单点。
//!
//! - **`push_boundary_event`**：把边界消息（System 通知 / 压缩点——user 锚定、
//!   按 `compression_marker` 判定）转成 chat_stream 事件推入统一通道
//!   （`chunk_type=message`）——与用户消息边界事件同构，前端 `applyServerMessage`
//!   按 id 查重追加，实时可见（历史加载与实时流同一份数据、同一条渲染路径）。
//!   压缩点是会话时序链上的普通节点（统一结构，不做区分）：推送与快照/历史
//!   按 id 幂等收敛，不产生重复。快照恢复（订阅端点）与断点对齐（fetch）兜底。
//! - **唯一调用方 = 工作集注入的边界推送回调**（ADR-039：所有写入经
//!   `ws.append`，门控与映射同源；core 不感知通道）。ADR-039 已删除
//!   `BroadcastingSessionManager` 装饰器（写方法从 `SessionManager` trait 移除
//!   后，wrapper 退化为空壳）。
//! - 通道：`task_event_tx`（broadcast）承载任务状态/命令输出事件与流式事件，
//!   前端按 `type` 区分。

use tianyan::common::types::StructuredMessage;

/// 把边界消息（System 通知 / 压缩点）转成 chat_stream 事件推入统一通道。
///
/// 复用 `map_chunk_to_event`（Message chunk → ChatStreamEvent 同构转换），
/// 与主会话流式路径同一映射；无订阅者时静默丢弃（broadcast 语义）。
///
/// **唯一调用方**：工作集注入的边界推送回调（ADR-039：所有写入经 `ws.append`，
/// 门控与映射同源；core 不感知通道，回调由装配层注入）。
pub fn push_boundary_event(
    event_tx: &tokio::sync::broadcast::Sender<String>,
    session_id: &str,
    msg: &StructuredMessage,
) {
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
            let _ = event_tx.send(json);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tianyan::common::types::StructuredMessage;

    use super::*;

    /// ADR-035 §3 补记：经**工作集注入回调**的推送路径——`ws.append` 落库
    /// System 通知后，统一事件通道收到 `chat_stream` 事件（前端
    /// `applyServerMessage` 按 id 查重追加显示）；普通消息不推送。
    #[tokio::test]
    async fn working_set_boundary_push_reaches_event_channel() {
        use tianyan::agent::working_set::WorkingSetRegistry;
        use tianyan::session::store::SessionStore;
        use tianyan::session::SessionHeader;

        let db = tianyan::db::Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let store = SessionStore::new(db).unwrap();
        store.create("s1", &SessionHeader::default()).await.unwrap();

        let (tx, mut rx) = tokio::sync::broadcast::channel::<String>(16);
        let reg = WorkingSetRegistry::new(Some(store));
        let tx2 = tx.clone();
        reg.set_boundary_push(Arc::new(move |sid: &str, msg: &StructuredMessage| {
            push_boundary_event(&tx2, sid, msg);
        }));
        let ws = reg.ensure("s1").await.unwrap();
        ws.append(
            &StructuredMessage::system("s1", "[后台命令完成] build"),
            true,
        )
        .await
        .unwrap();

        let payload = rx.try_recv().expect("应收到边界推送事件");
        assert!(payload.contains("\"type\":\"chat_stream\""), "{payload}");
        assert!(payload.contains("s1"), "事件带 session_id: {payload}");
        assert!(
            payload.contains("后台命令完成"),
            "事件带通知正文: {payload}"
        );

        ws.append(&StructuredMessage::user("s1", "hi"), true)
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "普通消息不应推送");
    }
}
