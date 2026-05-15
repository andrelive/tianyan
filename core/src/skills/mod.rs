//! 技能模块。
//!
//! 本模块提供技能管理和执行能力。
//!
//! # 架构
//!
//! 技能系统由以下组件组成：
//!
//! - **定义**：技能结构、参数模式和注册表
//! - **执行**：具有验证、安全和监控功能的技能执行
//! - **类型**：技能的通用类型
//!
//! # 示例
//!
//! ```no_run
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//! use tianyan::skills::{Skill, SkillRegistry, SkillExecutor, ExecutorConfig, register_builtin_skills};
//!
//! async fn setup_skills() {
//!     let mut registry = SkillRegistry::new();
//!     let config = ExecutorConfig::new();
//!     
//!     register_builtin_skills(&mut registry, &config);
//!     
//!     let registry = Arc::new(RwLock::new(registry));
//!     let executor = SkillExecutor::new(registry.clone(), config);
//! }
//! ```

mod definition;
mod executor;
pub mod handlers;
pub(crate) mod learning;
mod manager;
mod registry;
mod types;

pub use definition::{
    ParameterDefinition, ParameterSchema, ParameterType, Skill, SkillHandler, SkillRegistry,
};
pub use executor::{ExecutionLogEntry, ExecutorConfig, SkillExecutor};
pub use handlers::{
    FileDeleteHandler, FileListHandler, FileReadHandler, FileWriteHandler, HttpRequestHandler,
    SystemCommandHandler,
};
pub use learning::{
    ExecutionHistory, ExecutionStep, GeneratedSkill, SkillAction, SkillEvaluation,
    SkillLearningConfig, SkillLearningEngine, SkillParameter,
};
pub use manager::{SkillManager, SkillSummary};
pub use registry::{create_builtin_skills, register_builtin_skills};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        let _registry = SkillRegistry::new();
        let _config = ExecutorConfig::new();
        let _skill = Skill::new("test", "Test", "A test skill");
    }
}
