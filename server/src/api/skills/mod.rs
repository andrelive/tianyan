//! 技能领域模块
//!
//! 本模块提供技能管理功能，包括：
//! - 列出可用技能
//! - 执行技能
//! - 跟踪技能执行状态

pub mod handlers;
pub mod routes;
pub mod services;
pub mod types;

pub use routes::routes;
pub use types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, Skill, SkillExecutionStatus,
    SkillParameter,
};
