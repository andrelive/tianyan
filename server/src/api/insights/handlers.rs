//! 内部状态视图处理器。
//!
//! 记忆浏览直接读取 VFS memory 命名空间（不经 SummaryEngine——内容已由
//! MemoryExtractor 写入，此处仅提供读路径）；统计摘要读取 UsageStats。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::json;
use tracing::info;

use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use tianyan::vfs::{ContentStore, VfsCore};

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 列出记忆命名空间全部条目（含 L0/L1/L2 内容）。
///
/// 记忆由 MemoryTask（每 10 分钟 cron）经 MemoryExtractor 写入
/// `tianyan://memory/*`；此前无任何读路径，本端点补齐浏览能力。
pub async fn list_memories_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let vfs = state.vfs();
    let memory_root = TianyanUri::new(ContextNamespace::Memory, vec![]);

    let entries = vfs
        .list(&memory_root)
        .await
        .map_err(|e| ApiError::Internal(format!("记忆浏览错误：列出记忆失败：{e}")))?;

    let mut memories = Vec::with_capacity(entries.len());
    for entry in entries {
        let uri = &entry.metadata.uri;
        // 内容读取失败不阻塞整体浏览：缺失层级返回 null
        let abstract_content = vfs.read(uri, ContentLevel::Abstract).await.ok();
        let overview_content = vfs.read(uri, ContentLevel::Overview).await.ok();
        let detail_content = vfs.read(uri, ContentLevel::Detail).await.ok();
        memories.push(json!({
            "uri": uri.to_string(),
            "is_directory": entry.is_directory(),
            "name": uri.path().last().map(|s| s.to_string()),
            "metadata": entry.metadata,
            "abstract": abstract_content,
            "overview": overview_content,
            "detail": detail_content,
        }));
    }

    info!(total = memories.len(), "记忆列表已返回");
    Ok(Json(
        json!({ "memories": memories, "total": memories.len() }),
    ))
}

/// 返回使用统计摘要（技能调用、文档访问、搜索热度）。
pub async fn get_stats_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stats = state.usage_stats();
    Ok(Json(stats.query_summary().await))
}
