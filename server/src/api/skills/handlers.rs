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

/// 技能使用统计（调用次数/平均耗时/执行异常——哪些技能被用得多）。
///
/// 语义说明：技能是方法论文档，调用本身无成败语义；`success_calls` 仅表示
/// handler 执行未抛异常（异常 = 存储/模型故障的健康信号），**不是**技能成功率。
/// 数据源为 UsageStats 持久化统计（`skill:` 前缀 = 技能级，工具级忽略）。
pub async fn get_skills_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stats = state.usage_stats().query_top_skills(100).await;
    // 技能级统计：call_skill 以 `skill:<id>` 为键记录
    let skills: Vec<tianyan::observability::usage_stats::SkillStats> = stats
        .into_iter()
        .filter(|s| s.skill_id.starts_with("skill:"))
        .map(|mut s| {
            s.skill_id = s.skill_id.trim_start_matches("skill:").to_string();
            s
        })
        .collect();
    let total_calls: u64 = skills.iter().map(|s| s.total_calls).sum();
    let total_success: u64 = skills.iter().map(|s| s.success_calls).sum();
    let success_rate = if total_calls == 0 {
        0.0
    } else {
        total_success as f64 / total_calls as f64
    };
    Ok(Json(serde_json::json!({
        "skills": skills,
        "total_calls": total_calls,
        "total_success": total_success,
        "success_rate": success_rate,
    })))
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
