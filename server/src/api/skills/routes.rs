use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::api::skills::handlers::{
    execute_skill, get_skill_detail, get_skills_stats, list_skills,
};
use crate::state::AppState;

/// 创建技能路由
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/skills", get(list_skills))
        .route("/skills/stats", get(get_skills_stats))
        .route("/skills/{id}", get(get_skill_detail))
        .route("/skills/{id}/execute", post(execute_skill))
}
