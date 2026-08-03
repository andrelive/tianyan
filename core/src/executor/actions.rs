use std::time::Duration;

use serde_json::{json, Value};

use crate::common::error::TianyanError;

const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 30;

async fn read_reader_to_vec<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if let Err(e) = reader.read_to_end(&mut buf).await {
        tracing::warn!(error = %e, "读取命令输出失败");
    }
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

use crate::config::{SafetyMode, SecurityConfig};

/// Executor 安全策略。
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// 安全模式（strict / transform / permissive）。
    pub safety_mode: SafetyMode,
    /// 回收站目录（transform 模式下 rm/del 命令的目标路径）。
    pub trash_directory: std::path::PathBuf,
    /// 允许的命令白名单。若为 None，则允许所有不在黑名单中的命令。
    pub allowed_commands: Option<Vec<String>>,
    /// 禁止的命令黑名单。黑名单优先于白名单。
    pub blocked_commands: Vec<String>,
    /// 文件操作允许的目录白名单。空 = 不限制。
    pub allowed_directories: Vec<std::path::PathBuf>,
    /// 文件操作禁止的目录黑名单。黑名单优先。
    pub blocked_directories: Vec<std::path::PathBuf>,
    /// 是否允许写入文件。
    pub allow_file_write: bool,
    /// 命令执行超时时间（秒）。
    pub max_command_timeout_secs: u64,
    /// 单次文件读写最大大小（字节）。
    pub max_file_size: u64,
    /// 是否允许通过解释器执行命令。
    pub block_interpreters: bool,
}

impl SecurityPolicy {
    /// 从用户安全配置创建 SecurityPolicy。
    pub fn from_config(config: &SecurityConfig) -> Self {
        let blocked = if config.blocked_commands.is_empty() {
            vec![
                "rm".to_string(),
                "del".to_string(),
                "format".to_string(),
                "rmdir".to_string(),
                "rd".to_string(),
                "shutdown".to_string(),
                "taskkill".to_string(),
            ]
        } else {
            config.blocked_commands.clone()
        };

        Self {
            safety_mode: config.safety_mode,
            trash_directory: config.trash_directory.clone(),
            allowed_commands: if config.allowed_commands.is_empty() {
                None
            } else {
                Some(config.allowed_commands.clone())
            },
            blocked_commands: blocked,
            allowed_directories: config.allowed_directories.clone(),
            blocked_directories: config.blocked_directories.clone(),
            allow_file_write: true,
            max_command_timeout_secs: DEFAULT_COMMAND_TIMEOUT_SECS,
            max_file_size: config.max_file_size,
            block_interpreters: config.safety_mode != SafetyMode::Permissive,
        }
    }

    /// 尝试将危险命令转换为安全等价操作。
    ///
    /// 当前支持的转换：
    /// - `rm -rf <path>` → `mv <path> <trash_dir>/<timestamp>/`
    /// - `rm <path>` → `mv <path> <trash_dir>/<timestamp>/`
    /// - `del /f <path>` (Windows) → `move <path> <trash_dir>/<timestamp>/`
    /// - `rmdir <path>` → `mv <path> <trash_dir>/<timestamp>/`
    ///
    /// 返回 `Some(transformed_command)` 如果可以转换，`None` 表示无法安全转换。
    pub fn transform_command(&self, command: &str) -> Option<String> {
        if self.safety_mode != SafetyMode::Transform {
            return None;
        }

        let cmd_name = command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        // Only transform explicitly dangerous commands
        let transformable = matches!(cmd_name.as_str(), "rm" | "rmdir" | "del" | "rd");
        if !transformable {
            return None;
        }

        // Extract the path argument(s). For simplicity, handle the common cases:
        //   rm <path>
        //   rm -rf <path>
        //   rm -r <path>
        //   rm --force <path>
        //   del /f /q <path>
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.len() < 2 {
            return None; // Nothing to delete — refuse
        }

        // Collect non-flag arguments as paths
        let paths: Vec<&str> = parts[1..]
            .iter()
            .filter(|p| !p.starts_with('-') && !p.starts_with('/'))
            .copied()
            .collect();

        if paths.is_empty() {
            return None; // All arguments were flags — too dangerous to transform
        }

        // Create a timestamped subdirectory in trash to avoid collisions
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let trash_target = self.trash_directory.join(format!("{}_{}", ts, cmd_name));

        let mut result = String::new();
        if cfg!(target_os = "windows") {
            result.push_str("mkdir ");
            result.push_str(&trash_target.to_string_lossy());
            for path in &paths {
                result.push_str(" && move ");
                result.push_str(path);
                result.push(' ');
                result.push_str(&trash_target.to_string_lossy());
                result.push('\\');
            }
        } else {
            result.push_str("mkdir -p ");
            result.push_str(&trash_target.to_string_lossy());
            for path in &paths {
                result.push_str(" && mv ");
                result.push_str(path);
                result.push(' ');
                result.push_str(&trash_target.to_string_lossy());
                result.push('/');
            }
        }

        tracing::info!(
            original = %command,
            transformed = %result,
            "命令已转换为安全等价操作"
        );
        Some(result)
    }

    /// 检查文件路径是否在允许/禁止目录范围内。
    ///
    /// - `blocked_directories` 优先：路径在任一禁止目录下则拒绝。
    /// - `allowed_directories` 不为空时：路径必须在某一允许目录下。
    /// - Permissive 模式下跳过所有检查。
    pub fn check_path(&self, path: &std::path::Path) -> Result<(), TianyanError> {
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Blocked directories first (highest priority)
        for blocked in &self.blocked_directories {
            if canonical.starts_with(blocked) {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：路径在黑名单目录中：{} (禁止：{})",
                    path.display(),
                    blocked.display()
                )));
            }
        }

        // If allowed list is set, path must be under an allowed directory
        if !self.allowed_directories.is_empty() {
            let allowed = self
                .allowed_directories
                .iter()
                .any(|d| canonical.starts_with(d));
            if !allowed {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：路径不在白名单目录中：{}",
                    path.display()
                )));
            }
        }

        Ok(())
    }

    /// 检查文件大小是否在限制内。
    pub fn check_file_size(&self, size: u64) -> Result<(), TianyanError> {
        if size > self.max_file_size {
            return Err(TianyanError::Custom(format!(
                "executor: 安全策略违规：文件大小 {} 超出限制 {} 字节",
                size, self.max_file_size
            )));
        }
        Ok(())
    }
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self::from_config(&SecurityConfig::default())
    }
}

impl SecurityPolicy {
    /// 检查命令是否被允许执行。
    pub fn check_command(&self, command: &str) -> Result<(), TianyanError> {
        // Permissive mode: skip all checks
        if self.safety_mode == SafetyMode::Permissive {
            return Ok(());
        }

        // 检测命令链和命令替换元字符，防止注入
        if Self::has_shell_metacharacters(command) {
            return Err(TianyanError::Custom(
                "executor: 安全策略违规：命令包含不被允许的 shell 元字符（&&、||、;、`、$() 等）"
                    .to_string(),
            ));
        }

        let cmd_name = command.split_whitespace().next().unwrap_or(command);
        let pure_name = extract_command_base(cmd_name);

        if self.block_interpreters {
            let interpreters = ["cmd", "powershell", "pwsh", "bash", "sh", "wsl", "wsl.exe"];
            if interpreters.contains(&pure_name.as_str()) {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：禁止通过解释器执行命令：{}",
                    cmd_name
                )));
            }
        }

        for blocked in &self.blocked_commands {
            let blocked_lower = blocked.to_lowercase();
            if pure_name == blocked_lower {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：命令被安全策略禁止：{}",
                    cmd_name
                )));
            }
        }
        if let Some(allowed) = &self.allowed_commands {
            let is_allowed = allowed.iter().any(|a| a.to_lowercase() == pure_name);
            if !is_allowed {
                return Err(TianyanError::Custom(format!(
                    "executor: 安全策略违规：命令不在白名单中：{}",
                    cmd_name
                )));
            }
        }
        Ok(())
    }

    /// 检测命令中是否包含命令链或命令替换元字符。
    ///
    /// 阻止的模式：`&&`（命令链）、`||`（条件链）、`;`（分隔符）、
    /// `` ` ``（反引号替换）、`$(`（命令替换）。
    /// 管道 `|` 和重定向 `>` `<` 不在阻止范围内，因为它们是合法的
    /// 单命令操作。
    fn has_shell_metacharacters(command: &str) -> bool {
        command.contains("&&")
            || command.contains("||")
            || command.contains(';')
            || command.contains('`')
            || command.contains("$(")
    }

    /// 检查文件写入是否被允许。
    pub fn check_file_write(&self) -> Result<(), TianyanError> {
        if self.allow_file_write {
            Ok(())
        } else {
            Err(TianyanError::Custom(
                "executor: 安全策略违规：文件写入被安全策略禁止".to_string(),
            ))
        }
    }
}

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
///
/// 跨平台：Unix (Linux/macOS) 用 `sh -c`，Windows 用 `cmd /C`。
pub async fn execute_command_action(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
    let timeout = timeout_secs.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C");
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c");
        c
    };
    cmd.arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| TianyanError::Custom(format!("executor: 命令执行失败：{}", e)))?;

    let stdout_handle = child
        .stdout
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));

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
        Ok(Err(e)) => Err(TianyanError::Custom(format!(
            "executor: 命令执行失败：{}",
            e
        ))),
        Err(_elapsed) => {
            if let Err(e) = child.kill().await {
                tracing::warn!(error = %e, "终止超时子进程失败");
            }
            if let Err(e) = child.wait().await {
                tracing::warn!(error = %e, "等待超时子进程退出失败");
            }
            Err(TianyanError::Custom("executor: 执行超时".to_string()))
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

/// 执行文件读取操作。
pub async fn execute_read_file(path: &str) -> Result<Value, TianyanError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String(content))
}

/// 执行文件写入操作。
pub async fn execute_write_file(path: &str, content: &str) -> Result<Value, TianyanError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String("写入成功".to_string()))
}

/// 执行代码搜索操作。
pub async fn execute_search_code(query: &str, scope: Option<&str>) -> Result<Value, TianyanError> {
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--line-number")
        .arg("--max-count")
        .arg("20")
        .arg(query);

    if let Some(dir) = scope {
        cmd.current_dir(dir);
    }

    let output = cmd.output().await.map_err(|e| {
        TianyanError::Custom(format!("executor: 搜索失败：执行 ripgrep 失败：{}", e))
    })?;

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
        return Err(TianyanError::Custom(format!(
            "executor: 搜索失败：ripgrep 执行失败：{}",
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

/// 执行测试运行操作。
pub async fn execute_run_tests(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
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

/// 执行构建验证操作。
pub async fn execute_verify_build(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
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
mod tests {
    use super::*;

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
