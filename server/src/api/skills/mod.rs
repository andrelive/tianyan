//! 技能领域模块
//!
//! 本模块提供技能管理功能，包括：
//! - 列出可用技能
//! - 执行技能
//! - 跟踪技能执行状态

/// 技能请求处理函数
pub mod handlers;
/// 技能路由定义
pub mod routes;
/// 技能业务逻辑
pub mod services;
/// 技能类型定义
pub mod types;

pub use routes::routes;
pub use types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, Skill, SkillParameter,
};
