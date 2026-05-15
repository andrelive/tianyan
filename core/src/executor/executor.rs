use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::task::JoinSet;

use super::traits::ExecutorTrait;
use super::types::ExecutorError;
use super::types::{Action, FailureHandling, Step, StepResult};
use crate::skills::{SkillExecutionRequest, SkillExecutor};

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

/// Executor 结构体。
///
/// 负责机械地执行 Planner 生成的步骤，无思考能力。
/// 支持并行执行、失败重试、超时控制等功能。
///
/// 注意：SubPlanner 类型的 Action 不应传入 Executor。
/// Planner 应在调用 Executor 之前自行处理 SubPlanner 动作。
pub struct Executor {
    max_concurrency: usize,
    skill_executor: Option<Arc<SkillExecutor>>,
}

impl Executor {
    /// 创建新的 Executor。
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            skill_executor: None,
        }
    }

    /// 设置技能执行器。
    pub fn with_skill_executor(mut self, skill_executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(skill_executor);
        self
    }

    /// 执行步骤（并行）。
    ///
    /// - `steps` - 要执行的步骤列表（不应包含 SubPlanner 动作）
    /// - returns: 按 step_id 排序的执行结果列表
    pub async fn execute_steps(&self, steps: Vec<Step>) -> Vec<StepResult> {
        let executor = Arc::new(Self {
            max_concurrency: self.max_concurrency,
            skill_executor: self.skill_executor.clone(),
        });
        let mut results = Vec::with_capacity(steps.len());

        let mut join_set: JoinSet<StepResult> = JoinSet::new();

        for step in steps {
            let executor_clone = executor.clone();
            join_set.spawn(async move { executor_clone.execute_step(step).await });

            if join_set.len() >= self.max_concurrency {
                match join_set.join_next().await {
                    Some(Ok(result)) => results.push(result),
                    Some(Err(e)) => {
                        tracing::error!("步骤任务异常: {}", e);
                    }
                    None => {}
                }
            }
        }

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(r) => results.push(r),
                Err(e) => {
                    tracing::error!("步骤任务异常: {}", e);
                }
            }
        }

        results.sort_by_key(|r| r.step_id);
        results
    }

    /// 执行单个步骤。
    async fn execute_step(&self, step: Step) -> StepResult {
        let mut retries = 0;
        let max_retries = match &step.on_failure {
            FailureHandling::Retry { max_retries } => *max_retries,
            _ => 0,
        };

        loop {
            match self.execute_action(&step.action).await {
                Ok(output) => {
                    return StepResult {
                        step_id: step.step_id,
                        success: true,
                        output,
                        error: None,
                        actual_importance: None,
                    };
                }
                Err(e) => {
                    retries += 1;

                    match &step.on_failure {
                        FailureHandling::Ignore => {
                            return StepResult {
                                step_id: step.step_id,
                                success: false,
                                output: Value::Null,
                                error: Some(e.to_string()),
                                actual_importance: Some(0.1),
                            };
                        }
                        FailureHandling::Abort { error_message } => {
                            return StepResult {
                                step_id: step.step_id,
                                success: false,
                                output: Value::Null,
                                error: Some(format!("{}: {}", error_message, e)),
                                actual_importance: Some(0.1),
                            };
                        }
                        FailureHandling::Retry { .. } => {
                            if retries > max_retries {
                                return StepResult {
                                    step_id: step.step_id,
                                    success: false,
                                    output: Value::Null,
                                    error: Some(format!("重试 {} 次后仍失败: {}", retries, e)),
                                    actual_importance: Some(0.1),
                                };
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }

    /// 执行具体动作。
    ///
    /// 注意：SubPlanner 动作不应到达此方法。
    /// Planner 应在调用 execute_steps 之前预处理 SubPlanner 步骤。
    async fn execute_action(&self, action: &Action) -> Result<Value, ExecutorError> {
        match action {
            Action::ReadFile { path } => {
                let content = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| ExecutorError::FileError(e.to_string()))?;
                Ok(Value::String(content))
            }
            Action::WriteFile { path, content } => {
                tokio::fs::write(path, content)
                    .await
                    .map_err(|e| ExecutorError::FileError(e.to_string()))?;
                Ok(Value::String("写入成功".to_string()))
            }
            Action::ExecuteCommand {
                command,
                cwd,
                timeout_secs,
            } => execute_command_action(command, cwd.as_deref(), *timeout_secs).await,
            Action::SearchCode { query, scope } => {
                self.execute_search_code(query, scope.as_deref()).await
            }
            Action::CallSkill {
                skill_id,
                parameters,
            } => {
                if let Some(ref skill_executor) = self.skill_executor {
                    let params: HashMap<String, Value> = parameters
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    let request = SkillExecutionRequest::new(skill_id.clone(), params);
                    match skill_executor.execute(request).await {
                        Ok(result) => {
                            let mut data = HashMap::new();
                            if let Some(output) = result.output {
                                data.insert("output".to_string(), Value::String(output));
                            }
                            if let Some(error) = result.error {
                                data.insert("error".to_string(), Value::String(error));
                            }
                            if let Some(exit_code) = result.exit_code {
                                data.insert(
                                    "exit_code".to_string(),
                                    Value::Number(exit_code.into()),
                                );
                            }
                            data.insert(
                                "execution_time_ms".to_string(),
                                Value::Number(result.execution_time_ms.into()),
                            );
                            data.insert("success".to_string(), Value::Bool(result.success));
                            Ok(Value::Object(data.into_iter().collect()))
                        }
                        Err(e) => Err(ExecutorError::SkillExecution(e.to_string())),
                    }
                } else {
                    Err(ExecutorError::SkillExecution(
                        "SkillExecutor 未配置，无法执行 CallSkill".to_string(),
                    ))
                }
            }
            Action::RunTests {
                command,
                cwd,
                timeout_secs,
            } => {
                let output = run_command(command, cwd.as_deref(), *timeout_secs).await?;
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
            Action::VerifyBuild {
                command,
                cwd,
                timeout_secs,
            } => {
                let output = run_command(command, cwd.as_deref(), *timeout_secs).await?;
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
            Action::SubPlanner { .. } => {
                // SubPlanner 应由 Planner 在调用 execute_steps 之前预处理。
                // 如果到达此处，说明 Planner 没有正确处理，返回明确错误。
                Err(ExecutorError::Internal(
                    "SubPlanner 动作不应传入 Executor.execute_steps，应由 Planner 预处理"
                        .to_string(),
                ))
            }
        }
    }

    /// 执行代码搜索。
    async fn execute_search_code(
        &self,
        query: &str,
        scope: Option<&str>,
    ) -> Result<Value, ExecutorError> {
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
}

#[async_trait::async_trait]
impl ExecutorTrait for Executor {
    async fn execute_steps(&self, steps: Vec<Step>) -> Vec<StepResult> {
        self.execute_steps(steps).await
    }
}

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
async fn execute_command_action(
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

/// 运行命令并返回结构化输出。
async fn run_command(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, ExecutorError> {
    execute_command_action(command, cwd, timeout_secs).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::types::{Action, FailureHandling, Step};

    #[tokio::test]
    async fn test_executor_new() {
        let executor = Executor::new(5);
        assert_eq!(executor.max_concurrency, 5);
    }

    #[tokio::test]
    async fn test_execute_steps_empty() {
        let executor = Executor::new(5);
        let steps: Vec<Step> = vec![];
        let results = executor.execute_steps(steps).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_read_file() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "测试读取文件".to_string(),
            action: Action::ReadFile {
                path: "Cargo.toml".to_string(),
            },
            on_failure: FailureHandling::Ignore,
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_execute_write_file() {
        let executor = Executor::new(5);
        let temp_path = format!(
            "{}/test_executor_{}.txt",
            std::env::temp_dir().display(),
            std::process::id()
        );

        let step = Step {
            step_id: 1,
            description: "测试写入文件".to_string(),
            action: Action::WriteFile {
                path: temp_path.clone(),
                content: "测试内容".to_string(),
            },
            on_failure: FailureHandling::Ignore,
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);

        let _ = std::fs::remove_file(temp_path);
    }

    #[tokio::test]
    async fn test_execute_command() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "测试执行命令".to_string(),
            action: Action::ExecuteCommand {
                command: "echo Hello".to_string(),
                cwd: None,
                timeout_secs: Some(5),
            },
            on_failure: FailureHandling::Ignore,
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_execute_steps_concurrent() {
        let executor = Executor::new(2);

        let steps = vec![
            Step {
                step_id: 1,
                description: "任务 1".to_string(),
                action: Action::ExecuteCommand {
                    command: "echo 1".to_string(),
                    cwd: None,
                    timeout_secs: None,
                },
                on_failure: FailureHandling::Ignore,
                expected_importance: 0.5,
            },
            Step {
                step_id: 2,
                description: "任务 2".to_string(),
                action: Action::ExecuteCommand {
                    command: "echo 2".to_string(),
                    cwd: None,
                    timeout_secs: None,
                },
                on_failure: FailureHandling::Ignore,
                expected_importance: 0.5,
            },
            Step {
                step_id: 3,
                description: "任务 3".to_string(),
                action: Action::ExecuteCommand {
                    command: "echo 3".to_string(),
                    cwd: None,
                    timeout_secs: None,
                },
                on_failure: FailureHandling::Ignore,
                expected_importance: 0.5,
            },
        ];

        let results = executor.execute_steps(steps).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].step_id, 1);
        assert_eq!(results[1].step_id, 2);
        assert_eq!(results[2].step_id, 3);
    }

    #[tokio::test]
    async fn test_failure_handling_ignore() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "测试失败处理".to_string(),
            action: Action::ReadFile {
                path: "不存在的文件.txt".to_string(),
            },
            on_failure: FailureHandling::Ignore,
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.is_some());
        assert_eq!(results[0].actual_importance, Some(0.1));
    }

    #[tokio::test]
    async fn test_failure_handling_abort() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "测试终止处理".to_string(),
            action: Action::ReadFile {
                path: "不存在的文件.txt".to_string(),
            },
            on_failure: FailureHandling::Abort {
                error_message: "关键文件缺失".to_string(),
            },
            expected_importance: 0.9,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.as_ref().unwrap().contains("关键文件缺失"));
    }

    #[tokio::test]
    async fn test_failure_handling_retry() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "测试重试处理".to_string(),
            action: Action::ReadFile {
                path: "不存在的文件.txt".to_string(),
            },
            on_failure: FailureHandling::Retry { max_retries: 2 },
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.as_ref().unwrap().contains("重试"));
    }

    #[tokio::test]
    async fn test_sub_planner_rejected() {
        let executor = Executor::new(5);
        let step = Step {
            step_id: 1,
            description: "子规划器".to_string(),
            action: Action::SubPlanner {
                task: "分析代码".to_string(),
            },
            on_failure: FailureHandling::Ignore,
            expected_importance: 0.8,
        };

        let results = executor.execute_steps(vec![step]).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("Planner 预处理"));
    }
}
