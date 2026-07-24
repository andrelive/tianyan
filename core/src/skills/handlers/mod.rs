mod file_delete;
mod file_list;
mod file_read;
mod file_write;
mod http_request;
mod system_command;

pub use file_delete::FileDeleteHandler;
pub use file_list::FileListHandler;
pub use file_read::FileReadHandler;
pub use file_write::FileWriteHandler;
pub use http_request::HttpRequestHandler;
pub use system_command::SystemCommandHandler;

use std::collections::HashMap;
use std::time::Instant;

use crate::skills::types::SkillExecutionResult;

/// Constructs a successful SkillExecutionResult.
fn result_success(output: impl Into<String>, start: Instant) -> SkillExecutionResult {
    SkillExecutionResult {
        success: true,
        output: Some(output.into()),
        error: None,
        exit_code: None,
        execution_time_ms: start.elapsed().as_millis() as u64,
        data: HashMap::new(),
    }
}

/// Constructs a failed SkillExecutionResult.
fn result_failure(error: impl Into<String>, start: Instant) -> SkillExecutionResult {
    SkillExecutionResult {
        success: false,
        output: None,
        error: Some(error.into()),
        exit_code: None,
        execution_time_ms: start.elapsed().as_millis() as u64,
        data: HashMap::new(),
    }
}

/// Constructs a timeout SkillExecutionResult.
fn result_timeout(secs: u64, start: Instant) -> SkillExecutionResult {
    SkillExecutionResult {
        success: false,
        output: None,
        error: Some(format!("操作超时（超过 {} 秒）", secs)),
        exit_code: None,
        execution_time_ms: start.elapsed().as_millis() as u64,
        data: HashMap::new(),
    }
}
