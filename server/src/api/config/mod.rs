//! 配置领域模块
//!
//! 本模块提供配置管理功能，包括：
//! - 获取应用配置
//! - 更新配置
//! - 获取特定配置节
//! - 获取配置状态
//! - 测试模型连接

/// Provider 发现与引导处理函数
pub mod discovery_handlers;
/// 配置请求处理函数
pub mod handlers;
/// MCP 服务器管理处理函数
pub mod mcp_handlers;
/// 配置路由定义
pub mod routes;
/// 配置业务逻辑
pub mod services;
/// Soul（智能体人格）管理处理函数
pub mod soul_handlers;
/// 配置类型定义
pub mod types;

pub use routes::routes;
pub use types::{
    ConfigResponse, ModelsResponse, SwitchModelRequest, UpdateConfigRequest, UpdateConfigResponse,
};
