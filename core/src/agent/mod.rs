//! 天演智能体系统的智能体模块。
//!
//! 本模块提供核心智能体协调器，集成所有组件：
//! 检索、模型服务、技能和记忆。

/// 默认核心提示词内容。
pub const DEFAULT_SOUL: &str = include_str!("default_soul.md");

/// 会话状态管理。
pub mod session_state;
pub mod working_set;

mod agent_core;
/// 后台任务支持（fire-and-forget 委托 + 完成通知 + join 信号）。
pub mod background;
mod builder;
mod coordinator;
mod r#loop;
/// 角色学习引擎（ADR-016：自演化分工的角色侧）。
pub mod role_learning;
/// 角色向量路由（ADR-016 P3：任务描述 → 角色摘要语义匹配建议）。
pub mod role_router;
/// 角色化子 Agent 委托（delegate_to_agent role 参数）。
pub mod roles;
/// 流式事件统一转发（ADR-032：三处接线收敛——建通道即消费 + 字段注入单点）。
pub mod stream_forward;
mod tool_params;
mod tool_registry;
mod types;
/// 用户问题服务（ask_user 同步等待用户回答；对齐 DSH user-questions seam）。
pub mod user_questions;

pub use agent_core::Agent;
pub use agent_core::AgentWakeForwarder;
pub use builder::AgentBuilder;
pub use coordinator::AgentCoordinator;
pub use r#loop::{AgentLoop, AgentLoopConfig, AgentLoopResult};
pub use role_learning::{
    categorize_task_by_keyword, LearnedRole, RoleLearningConfig, RoleLearningEngine,
};
pub use role_router::{RoleMatch, RoleRouter};
pub use roles::{AgentRole, RoleRegistry, RoleSource, RoleStatus};
pub use session_state::SessionState;
pub use stream_forward::{
    inject_stream_event_fields, spawn_null_forwarder, spawn_stream_forwarder, BroadcastJsonDeliver,
    NullDeliver, StreamEventDeliver, StreamEventMapper, TaskSinkDeliver, STREAM_FORWARD_BUFFER,
};
pub use tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams, ReadFileParams,
    RunTestsParams, SearchCodeParams, SearchVfsParams, SelfCheckParams, VerifyBuildParams,
    VfsListParams, VfsReadParams, WriteFileParams,
};
pub use tool_registry::{DynamicToolExecutor, ToolRegistry};
pub use types::{
    AgentResponse, AgentState, AgentStreamChunk, SkillCallInfo, StreamChunkType, StreamEventSender,
    ToolCallEvent, ToolResultEvent, TurnStateEvent,
};
pub use working_set::{SessionWorkingSet, WorkingSetRegistry, WORKING_SET_IDLE_TTL};
