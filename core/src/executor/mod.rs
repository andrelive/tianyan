#[allow(clippy::module_inception)]
mod executor;

pub mod approval;
pub mod types;

pub use approval::{
    ApprovalDecision, ApprovalRecord, ApprovalRequest, ApprovalResponse, ApprovalWorkflow,
    ApprovalWorkflowConfig, AutoApprovalRule, RiskLevel,
};
pub use executor::{
    execute_command_action, execute_read_file, execute_run_tests, execute_search_code,
    execute_verify_build, execute_write_file, SecurityPolicy,
};
pub use types::{Action, ExecutorError};
