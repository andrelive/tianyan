use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::types::ExecutorError;
use crate::skills::SkillExecutor;

const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 30;

async fn read_reader_to_vec<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf).await;
    buf
}

fn extract_command_base(cmd: &str) -> String {
    let name = cmd.split_whitespace().next().unwrap_or(cmd);
    let file_name = name.rsplit(&['\\', '/'][..]).next().unwrap_or(name);
    let pure = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".bat"))
        .or_else(|| file_name.strip_suffix(".cmd"))
        .unwrap_or(file_name);
    pure.to_lowercase()
}

/// Executor 安全策略。
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// 允许的命令白名单。若为 None，则允许所有不在黑名单中的命令。
    pub allowed_commands: Option<Vec<String>>,
    /// 禁止的命令黑名单。黑名单优先于白名单。
    pub blocked_commands: Vec<String>,
    /// 是否允许写入文件。
    pub allow_file_write: bool,
    /// 命令执行超时时间（秒）。
    pub max_command_timeout_secs: u64,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            allowed_commands: None,
            blocked_commands: vec![
                "rm".to_string(),
                "del".to_string(),
                "format".to_string(),
                "rmdir".to_string(),
                "rd".to_string(),
                "shutdown".to_string(),
                "taskkill".to_string(),
            ],
            allow_file_write: true,
            max_command_timeout_secs: DEFAULT_COMMAND_TIMEOUT_SECS,
        }
    }
}

impl SecurityPolicy {
    /// 检查命令是否被允许执行。
    pub fn check_command(&self, command: &str) -> Result<(), ExecutorError> {
        let cmd_name = command.split_whitespace().next().unwrap_or(command);
        let pure_name = extract_command_base(cmd_name);
        let interpreters = ["cmd", "powershell", "pwsh", "bash", "sh", "wsl", "wsl.exe"];
        if interpreters.contains(&pure_name.as_str()) {
            return Err(ExecutorError::SecurityViolation(format!(
                "禁止通过解释器执行命令：{}",
                cmd_name
            )));
        }
        for blocked in &self.blocked_commands {
            let blocked_lower = blocked.to_lowercase();
            if pure_name == blocked_lower {
                return Err(ExecutorError::SecurityViolation(format!(
                    "命令被安全策略禁止：{}",
                    cmd_name
                )));
            }
        }
        if let Some(allowed) = &self.allowed_commands {
            let is_allowed = allowed.iter().any(|a| a.to_lowercase() == pure_name);
            if !is_allowed {
                return Err(ExecutorError::SecurityViolation(format!(
                    "命令不在白名单中：{}",
                    cmd_name
                )));
            }
        }
        Ok(())
    }

    /// 检查文件写入是否被允许。
    pub fn check_file_write(&self) -> Result<(), ExecutorError> {
        if self.allow_file_write {
            Ok(())
        } else {
            Err(ExecutorError::SecurityViolation(
                "文件写入被安全策略禁止".to_string(),
            ))
        }
    }
}

/// Executor 结构体（已废弃）。
#[deprecated(since = "0.2.0", note = "Use ToolRegistry instead")]
pub struct Executor {
    max_concurrency: usize,
    skill_executor: Option<Arc<SkillExecutor>>,
}

#[allow(deprecated)]
impl Executor {
    /// 创建新的 Executor（已废弃）。
    #[deprecated(since = "0.2.0", note = "Use ToolRegistry::new instead")]
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            skill_executor: None,
        }
    }

    /// 设置技能执行器（已废弃）。
    #[deprecated(
        since = "0.2.0",
        note = "Use ToolRegistry::with_skill_executor instead"
    )]
    pub fn with_skill_executor(mut self, skill_executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(skill_executor);
        self
    }
}

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
pub async fn execute_command_action(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, ExecutorError> {
    let timeout = timeout_secs.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);

    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ExecutorError::CommandError(e.to_string()))?;

    let stdout_handle = if let Some(reader) = child.stdout.take() {
        Some(tokio::spawn(read_reader_to_vec(reader)))
    } else {
        None
    };
    let stderr_handle = if let Some(reader) = child.stderr.take() {
        Some(tokio::spawn(read_reader_to_vec(reader)))
    } else {
        None
    };

    let result = tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await;

    match result {
        Ok(Ok(status)) => {
            let stdout_bytes = if let Some(handle) = stdout_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stdout 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let stderr_bytes = if let Some(handle) = stderr_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stderr 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            Ok(json!({
                "stdout": String::from_utf8_lossy(&stdout_bytes).to_string(),
                "stderr": String::from_utf8_lossy(&stderr_bytes).to_string(),
                "exit_code": status.code().unwrap_or(-1),
            }))
        }
        Ok(Err(e)) => Err(ExecutorError::CommandError(e.to_string())),
        Err(_elapsed) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(ExecutorError::Timeout)
        }
    }
}

/// 从 cargo test 输出中统计通过的测试数。
fn count_test_passed(stdout: &str) -> usize {
    for line in stdout.lines().rev() {
        if line.contains("test result:") {
            if let Some(passed_str) = line.split(';').next().and_then(|s| s.split("ok. ").nth(1)) {
                return passed_str
                    .split(' ')
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(pos) = line.find(" passed") {
                let before = &line[..pos];
                if let Some(num) = before.rsplit(' ').next() {
                    return num.parse().unwrap_or(0);
                }
            }
        }
    }
    0
}

/// 从 cargo test 输出中提取失败测试的名称和错误。
fn extract_test_failures(stdout: &str, stderr: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let combined = format!("{}\n{}", stdout, stderr);
    let mut in_failure = false;

    for line in combined.lines() {
        if line.contains("FAILED") || line.starts_with("thread '") {
            in_failure = true;
            failures.push(line.trim().to_string());
        } else if line.contains("failures:") {
            in_failure = false;
        } else if in_failure && !line.trim().is_empty() {
            failures.push(line.trim().to_string());
        }
        if failures.len() > 50 {
            failures.push("... (截断，过多失败输出)".to_string());
            break;
        }
    }

    failures
}

/// 从 build/lint 输出中提取编译错误。
fn extract_build_errors(stdout: &str, stderr: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let combined = format!("{}\n{}", stdout, stderr);

    for line in combined.lines() {
        if line.contains("error:") || line.contains("error[") {
            errors.push(line.trim().to_string());
        }
        if errors.len() > 50 {
            errors.push("... (截断，过多错误输出)".to_string());
            break;
        }
    }

    errors
}

pub async fn execute_read_file(path: &str) -> Result<Value, ExecutorError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ExecutorError::FileError(e.to_string()))?;
    Ok(Value::String(content))
}

pub async fn execute_write_file(path: &str, content: &str) -> Result<Value, ExecutorError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| ExecutorError::FileError(e.to_string()))?;
    Ok(Value::String("写入成功".to_string()))
}

pub async fn execute_search_code(query: &str, scope: Option<&str>) -> Result<Value, ExecutorError> {
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--line-number")
        .arg("--max-count")
        .arg("20")
        .arg(query);

    if let Some(dir) = scope {
        cmd.current_dir(dir);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| ExecutorError::SearchError(format!("执行 ripgrep 失败：{}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no matches found") || stderr.is_empty() {
            return Ok(json!({
                "query": query,
                "scope": scope,
                "results": [],
                "count": 0
            }));
        }
        return Err(ExecutorError::SearchError(format!(
            "ripgrep 执行失败：{}",
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Ok(json_value) = serde_json::from_str::<Value>(line) {
            if let Some(data) = json_value.get("data") {
                results.push(json!({
                    "path": data.get("path").map(|p| p.as_str().unwrap_or("")),
                    "line_number": data.get("line_number").map(|n| n.as_u64().unwrap_or(0)),
                    "text": data.get("text").map(|t| t.as_str().unwrap_or("")),
                }));
            }
        }
    }

    Ok(json!({
        "query": query,
        "scope": scope,
        "results": results,
        "count": results.len()
    }))
}

pub async fn execute_run_tests(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, ExecutorError> {
    let output = execute_command_action(command, cwd, timeout_secs).await?;
    let stdout = output["stdout"].as_str().unwrap_or("");
    let stderr = output["stderr"].as_str().unwrap_or("");
    let exit_code = output["exit_code"].as_i64().unwrap_or(-1);

    let passed = count_test_passed(stdout);
    let failures: Vec<String> = extract_test_failures(stdout, stderr);

    Ok(json!({
        "success": exit_code == 0,
        "passed": passed,
        "failed": failures.len(),
        "failures": failures,
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }))
}

pub async fn execute_verify_build(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, ExecutorError> {
    let output = execute_command_action(command, cwd, timeout_secs).await?;
    let stdout = output["stdout"].as_str().unwrap_or("");
    let stderr = output["stderr"].as_str().unwrap_or("");
    let exit_code = output["exit_code"].as_i64().unwrap_or(-1);

    let errors: Vec<String> = extract_build_errors(stdout, stderr);

    Ok(json!({
        "success": exit_code == 0,
        "error_count": errors.len(),
        "errors": errors,
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }))
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_executor_new() {
        let executor = Executor::new(5);
        assert_eq!(executor.max_concurrency, 5);
    }

    #[tokio::test]
    async fn test_execute_read_file() {
        let result = execute_read_file("Cargo.toml").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_write_file() {
        let temp_path = format!(
            "{}/test_executor_{}.txt",
            std::env::temp_dir().display(),
            std::process::id()
        );
        let result = execute_write_file(&temp_path, "测试内容").await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(temp_path);
    }

    #[tokio::test]
    async fn test_execute_command() {
        let result = execute_command_action("echo Hello", None, Some(5)).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output["stdout"].as_str().unwrap_or("").contains("Hello"));
    }
}
