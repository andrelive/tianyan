use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 系统命令处理器。
pub struct SystemCommandHandler {
    blocked_commands: Vec<String>,
}

impl SystemCommandHandler {
    /// 创建新的系统命令处理器。
    pub fn new(blocked_commands: Vec<String>) -> Self {
        Self { blocked_commands }
    }
}

#[async_trait]
impl SkillHandler for SystemCommandHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::InvalidSkillParameters {
                skill: "system_command".to_string(),
                message: "缺少 'command' 参数".to_string(),
            })?;

        let cmd_parts: Vec<&str> = command.split_whitespace().collect();
        if let Some(first) = cmd_parts.first() {
            if self
                .blocked_commands
                .iter()
                .any(|b| b.to_lowercase() == first.to_lowercase())
            {
                return Err(TianyanError::OperationNotAllowed(format!(
                    "命令 '{}' 已被阻止",
                    first
                )));
            }
        }

        let working_dir = context.working_directory.as_ref().map(PathBuf::from);

        let output = if cfg!(target_os = "windows") {
            tokio::process::Command::new("cmd")
                .args(["/C", command])
                .current_dir(working_dir.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }))
                .output()
                .await
        } else {
            tokio::process::Command::new("sh")
                .args(["-c", command])
                .current_dir(working_dir.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }))
                .output()
                .await
        };

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                let combined_output = if stderr.is_empty() {
                    stdout.clone()
                } else if stdout.is_empty() {
                    stderr.clone()
                } else {
                    format!("{}\n{}", stdout, stderr)
                };

                Ok(SkillExecutionResult {
                    success: output.status.success(),
                    output: Some(combined_output),
                    error: if output.status.success() {
                        None
                    } else {
                        Some(stderr)
                    },
                    exit_code: output.status.code(),
                    execution_time_ms: start.elapsed().as_millis() as u64,
                    data: HashMap::new(),
                })
            }
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("执行命令失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "system_command"
    }
}
