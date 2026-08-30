//! 结构化 Trace 视图：`GET /api/v1/traces`。
//!
//! 按会话 / 后台任务过滤查询执行 span（轮次/工具/任务），时间正序返回，
//! 供前端回放面板与坏例分析使用。查询前自动 flush 内存缓冲。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 查询参数：`session_id` / `task_id` 二选一或全空（全局最近记录）。
#[derive(Debug, Default, Deserialize)]
pub struct TraceQuery {
    /// 会话 ID 过滤。
    pub session_id: Option<String>,
    /// 后台任务 ID 过滤（bt_ 前缀）。
    pub task_id: Option<String>,
    /// 返回条数上限（默认 200，最大 1000）。
    pub limit: Option<usize>,
}

/// 查询执行 span 列表。
pub async fn query_traces(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let collector = state.trace_collector();

    let spans = collector
        .query(query.session_id.as_deref(), query.task_id.as_deref(), limit)
        .await?;

    // 回放视图：按 (session_id, task_id) 分组，组内时间正序
    let mut groups: Vec<serde_json::Value> = Vec::new();
    let mut current_key: Option<(String, Option<String>)> = None;
    let mut current_spans: Vec<&tianyan::db::trace::TraceSpan> = Vec::new();

    for span in &spans {
        let key = (span.session_id.clone(), span.task_id.clone());
        match &current_key {
            Some(k) if k == &key => {}
            _ => {
                if let Some(k) = current_key.take() {
                    groups.push(serde_json::json!({
                        "session_id": k.0,
                        "task_id": k.1,
                        "spans": current_spans
                            .iter()
                            .map(|s| serde_json::to_value(s).unwrap_or_default())
                            .collect::<Vec<_>>(),
                    }));
                }
                current_key = Some(key);
                current_spans.clear();
            }
        }
        current_spans.push(span);
    }
    if let Some(k) = current_key {
        groups.push(serde_json::json!({
            "session_id": k.0,
            "task_id": k.1,
            "spans": current_spans
                .iter()
                .map(|s| serde_json::to_value(s).unwrap_or_default())
                .collect::<Vec<_>>(),
        }));
    }

    Ok(Json(serde_json::json!({
        "total_spans": spans.len(),
        "groups": groups,
    })))
}
