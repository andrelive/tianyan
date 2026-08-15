//! 天演智能体系统的智能体模块。
//!
//! 本模块提供核心智能体协调器，集成所有组件：
//! 检索、模型服务、技能和记忆。

/// 默认核心提示词内容。
pub const DEFAULT_SOUL: &str = include_str!("default_soul.md");

/// 会话状态管理。
pub mod session_state;

mod agent_core;
/// 后台任务支持（fire-and-forget 委托 + 完成通知 + join 信号）。
pub mod background;
mod builder;
mod coordinator;
mod r#loop;
/// 角色学习引擎（ADR-016：自演化分工的角色侧）。
pub mod role_learning;
/// 角色 VFS 存储（ADR-016：注册表持久化）。
pub mod role_store;
/// 角色化子 Agent 委托（delegate_to_agent role 参数）。
pub mod roles;
mod tool_params;
mod tool_registry;
mod types;

pub use agent_core::Agent;
pub use agent_core::AgentWakeForwarder;
pub use builder::AgentBuilder;
pub use coordinator::AgentCoordinator;
pub use r#loop::{AgentLoop, AgentLoopConfig, AgentLoopResult};
pub use role_learning::{LearnedRole, RoleLearningConfig, RoleLearningEngine};
pub use role_store::RoleStore;
pub use roles::{AgentRole, RoleRegistry, RoleSource, RoleStatus};
pub use session_state::SessionState;
pub use tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams, ReadFileParams,
    RunTestsParams, SearchCodeParams, SearchKnowledgeParams, SelfCheckParams, VerifyBuildParams,
    VfsListParams, VfsReadParams, WriteFileParams,
};
pub use tool_registry::{DynamicToolExecutor, ToolRegistry};
pub use types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    SkillCallInfo, StreamChunkType, StreamEventSender, ToolCallEvent,
};
