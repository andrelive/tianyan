#[allow(clippy::module_inception)]
mod executor;

pub mod approval;
pub mod judge;
pub mod types;
pub mod verification;

pub use approval::{
    ApprovalDecision, ApprovalRecord, ApprovalRequest, ApprovalResponse, ApprovalWorkflow,
    ApprovalWorkflowConfig, AutoApprovalRule, RiskLevel,
};
#[allow(deprecated)]
pub use executor::{
    execute_command_action, execute_read_file, execute_run_tests, execute_search_code,
    execute_verify_build, execute_write_file, Executor, SecurityPolicy,
};
pub use judge::{JudgeVerdict, LlmJudge};
pub use types::{Action, ExecutorError};
pub use verification::{VerificationGate, VerificationIssue, VerificationReport};
