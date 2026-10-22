//! 天演服务器 API 路由
//!
//! 本模块提供天演服务器的所有 HTTP API 端点。
//!
//! # 架构
//!
//! API 采用领域驱动设计（DDD）原则组织：
//!
//! - `shared/` - 共享组件（类型、错误、响应）
//! - `chat/` - 对话领域（非流式和流式）
//! - `sessions/` - 会话管理领域
//! - `knowledge/` - 知识管理领域（摄入 + 检索）
//! - `skills/` - 技能执行领域
//! - `config/` - 配置管理领域

use axum::extract::OriginalUri;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::Router;
use std::sync::Arc;

// 领域模块
pub mod chat;
pub mod clipboard;
pub mod config;
pub mod events;
pub mod goals;
pub mod insights;
pub mod knowledge;
pub mod roles;
pub mod sessions;
pub mod shared;
pub mod skills;
pub mod tasks;
pub mod todos;
pub mod tools;
pub mod traces;
pub mod workspace;

// 重新导出应用状态和共享类型
pub use crate::state::AppState;
pub use shared::{ApiError, ChatMessage, MessageRole, TokenUsage};

/// 创建包含所有路由的 API 路由器
///
/// 本函数将所有 API 模块路由组合成一个路由器，
/// 挂载在 `/api/v1` 前缀下。
///
/// # 示例
///
/// ```
/// use tianyan_server::api::create_api_router;
/// use tianyan_server::api::AppState;
/// use std::sync::Arc;
///
/// // 注意：此示例假设 AppState 可以同步创建
/// // 实际使用中应使用 AppState::new(config).await
/// ```
pub fn create_api_router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api/v1", create_routes().fallback(api_not_found))
        .with_state(state)
}

/// 未匹配的 `/api/v1/**` → **404 JSON**。
///
/// 为什么必须显式挂：主 router 的 `fallback_service` 是 GUI 静态文件服务，而 SPA 需要
/// 「未知路径回 `index.html`」（前端路由 `/chat` 刷新不能 404）。若不给 API 子树兜底，
/// `/api/v1/typo` 会被 SPA fallback 吞成 `index.html` —— 前端拿到 HTML 去 `json()`
/// 会报「Unexpected token <」，把「路径写错」伪装成解析错误。
async fn api_not_found(OriginalUri(uri): OriginalUri) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "not_found",
            "message": format!("未知 API 端点：{}", uri.path()),
        })),
    )
        .into_response()
}

/// 创建无状态的路由（用于测试/组合）
pub fn create_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(chat::routes())
        .merge(sessions::routes())
        .merge(knowledge::routes())
        .merge(skills::routes())
        .merge(config::routes())
        .merge(insights::routes())
        .merge(workspace::routes())
        .merge(tasks::routes())
        .merge(todos::routes())
        .merge(goals::routes())
        .merge(tools::routes())
        .merge(roles::routes())
        .merge(traces::routes())
        .merge(clipboard::routes())
        .merge(events::routes())
        .merge(crate::scheduled_tasks::routes::routes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_routes() {
        let _routes = create_routes();
        // 仅验证编译通过并创建路由器
        // 完整路由测试需要集成测试
    }
}
