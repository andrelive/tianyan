use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// 系统命令处理器。
pub struct SystemCommandHandler {
    blocked_commands: Vec<String>,
    /// 命令执行超时（秒），默认 300。
    timeout_secs: u64,
}

impl SystemCommandHandler {
    /// 创建新的系统命令处理器。
    pub fn new(blocked_commands: Vec<String>) -> Self {
        Self {
            blocked_commands,
            timeout_secs: 300,
        }
    }

    /// 设置命令执行超时。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
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
            .ok_or_else(|| {
                TianyanError::Custom("[system_command] 缺少 'command' 参数".to_string())
            })?;

        let cmd_parts: Vec<&str> = command.split_whitespace().collect();
        if let Some(first) = cmd_parts.first() {
            if self
                .blocked_commands
                .iter()
                .any(|b| b.to_lowercase() == first.to_lowercase())
            {
                return Err(TianyanError::Custom(format!(
                    "操作不被允许：命令 '{}' 已被阻止",
                    first
                )));
            }
        }

        let working_dir = context.working_directory.as_ref().map(PathBuf::from);

        let timeout = Duration::from_secs(self.timeout_secs);
        let cmd_op = async {
            if cfg!(target_os = "windows") {
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
            }
        };

        match tokio::time::timeout(timeout, cmd_op).await {
            Ok(Ok(output)) => {
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
            Ok(Err(e)) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("执行命令失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
            Err(_) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("命令执行超时（超过 {} 秒）", self.timeout_secs)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn params(command: &str) -> HashMap<String, Value> {
        HashMap::from([("command".to_string(), Value::String(command.to_string()))])
    }

    #[tokio::test]
    async fn test_command_success_output_captured() {
        // `echo` 在 cmd.exe（Windows）和 sh（Unix）下行为一致
        let handler = SystemCommandHandler::new(Vec::new());
        let result = handler
            .execute(params("echo hello-ty"), ExecutionContext::default())
            .await
            .unwrap();

        assert!(result.success, "命令应执行成功: {:?}", result.error);
        let output = result.output.unwrap();
        assert!(output.contains("hello-ty"), "stdout 应包含输出: {output}");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_command_failure_exit_code_nonzero() {
        // `exit 3` 在 cmd /C 和 sh -c 下都以 3 退出
        let handler = SystemCommandHandler::new(Vec::new());
        let result = handler
            .execute(params("exit 3"), ExecutionContext::default())
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, Some(3));
    }

    #[tokio::test]
    async fn test_command_blocked_rejected() {
        let handler = SystemCommandHandler::new(vec!["rm".to_string()]);
        let err = handler
            .execute(params("rm -rf /"), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("已被阻止"),
            "被阻止的命令必须拒绝: {err}"
        );
    }

    #[tokio::test]
    async fn test_command_blocked_case_insensitive() {
        let handler = SystemCommandHandler::new(vec!["rm".to_string()]);
        let err = handler
            .execute(params("RM -rf /tmp/x"), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("已被阻止"),
            "大小写不应绕过拦截: {err}"
        );
    }

    #[tokio::test]
    async fn test_command_missing_param() {
        let handler = SystemCommandHandler::new(Vec::new());
        let err = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'command'"));
    }

    #[tokio::test]
    async fn test_command_working_directory_respected() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();

        // 在工作目录内列出文件，验证 current_dir 生效（跨平台：Windows 用 dir，Unix 用 ls）
        let cmd = if cfg!(target_os = "windows") {
            "dir /b"
        } else {
            "ls -A"
        };
        let handler = SystemCommandHandler::new(Vec::new());
        let context = ExecutionContext {
            working_directory: Some(dir.path().to_str().unwrap().to_string()),
            ..ExecutionContext::default()
        };
        let result = handler.execute(params(cmd), context).await.unwrap();

        assert!(result.success);
        assert!(
            result.output.unwrap().contains("marker.txt"),
            "输出应包含工作目录中的文件"
        );
    }

    #[tokio::test]
    async fn test_command_nonexistent_returns_failure() {
        // 不存在的命令：cmd 会报“不是内部或外部命令”，sh 报 command not found —— 均以非零退出
        let handler = SystemCommandHandler::new(Vec::new());
        let result = handler
            .execute(
                params("tianyan_no_such_command_xyz_42"),
                ExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(
            SystemCommandHandler::new(Vec::new()).skill_id(),
            "system_command"
        );
    }
}
