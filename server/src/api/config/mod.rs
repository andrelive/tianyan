//! 配置领域模块
//!
//! 本模块提供配置管理功能，包括：
//! - 获取应用配置
//! - 更新配置
//! - 获取特定配置节
//! - 配置向导（初始化配置）

pub mod handlers;
pub mod routes;
pub mod services;
pub mod types;

// 配置向导子模块
pub mod wizard_handlers;
pub mod wizard_routes;
pub mod wizard_services;
pub mod wizard_types;

pub use routes::routes;
pub use types::{
    ConfigResponse, ModelsResponse, SwitchModelRequest, UpdateConfigRequest, UpdateConfigResponse,
};
