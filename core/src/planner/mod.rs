//! Planner 模块（已废弃）。
//!
//! 功能已迁移至 `agent::loop::AgentLoop`。
//! 保留此模块仅用于向后兼容的 `ClarificationQuestion` 类型导出。

pub mod types;

pub use types::{ClarificationQuestion, QuestionType};
