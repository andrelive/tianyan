//! 后台任务视图：GET /api/v1/tasks + POST /api/v1/tasks/{id}/cancel +
//! GET /api/v1/tasks/stream（子智能体消息流 SSE）。
//!
//! 提供 delegate_to_agent(background) 后台任务的只读列表与取消
//! （状态/结果/所属会话），供前端任务面板使用；任务生命周期由 core
//! 后台任务管理器驱动。

use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::Json;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// SSE 流式通道缓冲区大小（与 chat 流一致）。
const SSE_CHANNEL_BUFFER: usize = 100;

/// 后台任务列表。
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<tianyan::agent::background::BackgroundTask>>, ApiError> {
    let agent = state.agent().await;
    let tasks = agent.background_tasks().await;
    Ok(Json(tasks))
}

/// 子智能体消息流 SSE（ADR-026）：订阅全部委托任务的事件，
/// 事件为 ChatStreamEvent 同构 JSON（带 task_id 归集），前端面板按
/// task_id 渲染到对应任务卡片。客户端断开时静默结束。
pub async fn stream_tasks(
    State(state): State<Arc<AppState>>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(SSE_CHANNEL_BUFFER);
    let mut event_rx = state.task_event_tx.subscribe();
    let shutdown_flag = state.shutdown_flag();
    tokio::spawn(async move {
        // 服务关停时退出（与 chat 流 forwarder 同模式）：本流是前端常驻订阅
        // （EventSource 不关闭），若死等 broadcast 则 axum 优雅关停
        // （with_graceful_shutdown 等待所有连接结束）永不完成，托盘退出卡死。
        let mut shutdown_poll = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Ok(json) => {
                            if tx.send(Ok(Event::default().data(json))).await.is_err() {
                                break; // 客户端断开
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                _ = shutdown_poll.tick() => {
                    if shutdown_flag.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        }
    });
    Sse::new(ReceiverStream::new(rx))
}

/// 取消后台任务（终态任务幂等；任务不存在返回 404）。
pub async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent = state.agent().await;
    // ? 传播：保留 core 错误语义（ApiError 变体保真，不吞成 Internal）
    let cancelled = agent.cancel_background_task(&task_id).await?;
    if !cancelled {
        return Err(ApiError::NotFound(format!("任务不存在：{}", task_id)));
    }
    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "status": "cancelled",
    })))
}
