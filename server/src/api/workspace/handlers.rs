//! 工作区请求处理函数

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde_json::Value;

use crate::api::shared::error::ApiError;
use crate::api::workspace::services::WorkspaceService;
use crate::api::workspace::types::{
    ApplyEditRequest, ApplyEditResponse, ApplyPatchRequest, ApplyPatchResponse, DiffQuery,
    DirsQuery, DirsResponse, ReadQuery, TreeQuery, TreeResponse,
};
use crate::state::AppState;

/// 从应用状态构建工作区服务（会话管理器 + 全局默认目录 + 快照管理器）。
async fn build_service(state: Arc<AppState>) -> WorkspaceService {
    let default_working_dir = state
        .config()
        .read()
        .await
        .agent
        .working_directory
        .clone()
        .map(PathBuf::from);
    WorkspaceService::new(
        state.session_manager(),
        default_working_dir,
        state.snapshot_manager(),
    )
}

/// 处理目录树请求。
pub async fn tree_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<TreeResponse>, ApiError> {
    let service = build_service(state).await;
    let rel = query.path.unwrap_or_default();
    Ok(Json(service.tree(&rel, query.session_id.as_deref()).await?))
}

/// 处理目录选择器请求（工作区选择：逐级浏览任意目录，不限于当前工作目录）。
pub async fn dirs_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirsQuery>,
) -> Result<Json<DirsResponse>, ApiError> {
    let service = build_service(state).await;
    Ok(Json(service.list_dirs(query.path.as_deref()).await?))
}

/// 处理文件读取请求（委托 core executor）。
pub async fn read_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<Value>, ApiError> {
    let service = build_service(state).await;
    let rel = query.path.unwrap_or_default();
    Ok(Json(
        service
            .read(&rel, query.offset, query.limit, query.session_id.as_deref())
            .await?,
    ))
}

/// 处理差异对比请求（快照模式 / 文件间模式）。
pub async fn diff_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<Value>, ApiError> {
    let service = build_service(state).await;
    let value = if query.base.as_deref() == Some("snapshot") {
        let session_id = query
            .session_id
            .ok_or_else(|| ApiError::BadRequest("快照模式缺少 session_id".to_string()))?;
        let index = query
            .index
            .ok_or_else(|| ApiError::BadRequest("快照模式缺少 index".to_string()))?;
        service
            .diff_snapshot(&session_id, index, query.path.as_deref())
            .await?
    } else if let (Some(path_a), Some(path_b)) = (query.path_a.as_deref(), query.path_b.as_deref())
    {
        service
            .diff_files(path_a, path_b, query.session_id.as_deref())
            .await?
    } else {
        return Err(ApiError::BadRequest(
            "diff 需要 base=snapshot&session_id&index 或 path_a&path_b".to_string(),
        ));
    };
    Ok(Json(value))
}

/// 处理补丁应用请求（前端保存文件的主通道）。
pub async fn apply_patch_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ApplyPatchRequest>,
) -> Result<Json<ApplyPatchResponse>, ApiError> {
    let service = build_service(state).await;
    Ok(Json(
        service
            .apply_patch(&payload.patch, payload.session_id.as_deref())
            .await?,
    ))
}

/// 处理内容匹配编辑请求（供 LLM/外部工作流）。
pub async fn apply_edit_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ApplyEditRequest>,
) -> Result<Json<ApplyEditResponse>, ApiError> {
    let service = build_service(state).await;
    Ok(Json(
        service
            .apply_edit(&payload.path, payload.edits, payload.session_id.as_deref())
            .await?,
    ))
}
