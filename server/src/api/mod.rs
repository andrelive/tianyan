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

use axum::Router;
use std::sync::Arc;

// 领域模块
pub mod chat;
pub mod config;
pub mod insights;
pub mod knowledge;
pub mod sessions;
pub mod shared;
pub mod skills;
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
        .nest("/api/v1", create_routes())
        .with_state(state)
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
