use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use tracing::info;

use crate::api::shared::error::ApiError;
use crate::api::skills::services::SkillService;
use crate::api::skills::types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, SkillDetail,
};
use crate::state::AppState;

/// 列出所有可用技能
pub async fn list_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListSkillsResponse>, ApiError> {
    info!("列出所有技能");

    let service = SkillService::new(state.skill_registry(), state.skill_executor(), state.vfs());

    service
        .list_skills()
        .await
        .inspect_err(|e| tracing::error!("列出技能失败: {}", e))
        .map(Json)
}

/// 获取技能详情（完整内容 + 创建/更新时间）
pub async fn get_skill_detail(
    State(state): State<Arc<AppState>>,
    Path(skill_id): Path<String>,
) -> Result<Json<SkillDetail>, ApiError> {
    if skill_id.trim().is_empty() {
        return Err(ApiError::BadRequest("技能ID不能为空".to_string()));
    }

    info!("获取技能详情: {}", skill_id);

    let service = SkillService::new(state.skill_registry(), state.skill_executor(), state.vfs());

    service
        .get_skill_detail(&skill_id)
        .await
        .inspect_err(|e| tracing::error!("获取技能详情失败: {}", e))
        .map(Json)
}

/// 执行技能
pub async fn execute_skill(
    State(state): State<Arc<AppState>>,
    Path(skill_id): Path<String>,
    Json(request): Json<ExecuteSkillRequest>,
) -> Result<Json<ExecuteSkillResponse>, ApiError> {
    if skill_id.trim().is_empty() {
        return Err(ApiError::BadRequest("技能ID不能为空".to_string()));
    }
    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    info!("执行技能: {}", skill_id);

    let service = SkillService::new(state.skill_registry(), state.skill_executor(), state.vfs());

    service
        .execute_skill(&skill_id, request)
        .await
        .inspect_err(|e| tracing::error!("执行技能失败: {}", e))
        .map(Json)
}
