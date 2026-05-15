//! 天演智能体系统的智能体模块。
//!
//! 本模块提供核心智能体协调器，集成所有组件：
//! 检索、模型服务、技能和记忆。

pub mod session_state;

mod builder;
mod coordinator;
pub mod harness;
pub mod skill_subsystem;
mod tool_params;
mod r#loop;
mod tool_registry;
mod tools;
mod types;

pub use crate::config::AgentConfig;
pub use builder::AgentBuilder;
pub use coordinator::{Agent, AgentCoordinator};
pub use r#loop::{AgentLoop, AgentLoopConfig, AgentLoopError, AgentLoopResult};
pub use session_state::{SessionState, SessionStateManager};
pub use tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    ReadFileParams, RunTestsParams, SearchCodeParams, VerifyBuildParams, WriteFileParams,
};
pub use tool_registry::{ToolExecutionError, ToolRegistry};
pub use tools::{AgentTool, ToolResult};
pub use types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    SkillCallInfo, StreamChunkType, StreamEventSender,
};
