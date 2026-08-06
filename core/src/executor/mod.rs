mod actions;
/// 命令执行（ExecuteCommand 动作，无安全策略依赖）。
mod command;
/// 命令输出解析工具（构建错误提取 / 测试输出解析）。
mod output_parse;
/// Executor 安全策略。
mod security;

/// 审批工作流模块。
pub mod approval;
/// LLM-as-Judge 语义验证模块。
pub mod judge;
/// 执行器类型定义。
pub mod types;
/// 验证门控模块。
pub mod verification;

pub use actions::{
    execute_command_action, execute_read_file, execute_run_tests, execute_search_code,
    execute_verify_build, execute_write_file, SecurityPolicy,
};
pub use judge::LlmJudge;
pub use types::Action;
pub use verification::{VerificationGate, VerificationResult};
