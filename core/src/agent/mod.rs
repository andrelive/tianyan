//! 天演智能体系统的智能体模块。
//!
//! 本模块提供核心智能体协调器，集成所有组件：
//! 检索、模型服务、技能和记忆。

/// 默认核心提示词内容。
pub const DEFAULT_SOUL: &str = include_str!("default_soul.md");

/// 会话状态管理。
pub mod session_state;

mod agent_core;
mod builder;
mod coordinator;
mod r#loop;
mod tool_params;
mod tool_registry;
mod tools;
mod types;

pub use agent_core::Agent;
pub use builder::AgentBuilder;
pub use coordinator::AgentCoordinator;
pub use r#loop::{AgentLoop, AgentLoopConfig, AgentLoopError, AgentLoopResult};
pub use session_state::{SessionState, SessionStateManager};
pub use tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams, ReadFileParams,
    RunTestsParams, SearchCodeParams, SearchKnowledgeParams, SelfCheckParams, VerifyBuildParams,
    VfsListParams, VfsReadParams, WriteFileParams,
};
pub use tool_registry::{ToolExecutionError, ToolRegistry};
pub use tools::{AgentTool, ToolResult};
pub use types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    SkillCallInfo, StreamChunkType, StreamEventSender,
};
