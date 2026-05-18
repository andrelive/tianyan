use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, ExecuteCommandParams, ReadFileParams, RunTestsParams,
    SearchCodeParams, VerifyBuildParams, WriteFileParams,
};
use crate::executor::SecurityPolicy;
use crate::model::types::{FunctionDefinition, ToolCall, ToolDefinition};
use crate::skills::{SkillExecutionRequest, SkillExecutor};

/// 工具执行错误。
#[derive(thiserror::Error, Debug, Clone)]
pub enum ToolExecutionError {
    /// 未知工具。
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    /// 参数无效。
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    /// 执行失败。
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    /// 安全违规。
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    /// 需要追问。
    #[error("Ask user: {0}")]
    AskUser(String),
}

/// 工具注册表，维护工具定义并并行执行 tool_calls。
#[derive(Clone)]
pub struct ToolRegistry {
    security_policy: SecurityPolicy,
    skill_executor: Option<Arc<SkillExecutor>>,
    definitions: Vec<ToolDefinition>,
}

impl ToolRegistry {
    /// 创建新的注册表并注册内置工具。
    pub fn new(security_policy: SecurityPolicy) -> Self {
        let mut registry = Self {
            security_policy,
            skill_executor: None,
            definitions: Vec::new(),
        };
        registry.register_builtin_tools();
        registry
    }

    /// 设置技能执行器。
    pub fn with_skill_executor(mut self, executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(executor);
        self
    }

    /// 获取所有工具定义。
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    /// 并行执行多个 tool_call。
    pub async fn execute_parallel(
        &self,
        calls: &[ToolCall],
    ) -> Vec<(String, Result<serde_json::Value, ToolExecutionError>)> {
        let mut set = tokio::task::JoinSet::new();
        for call in calls {
            let call: ToolCall = call.clone();
            let this = self.clone();
            set.spawn(async move {
                let result = this.execute_single(&call).await;
                (call.id, result)
            });
        }
        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok((id, result)) = res {
                results.push((id, result));
            }
        }
        results
    }

    async fn execute_single(
        &self,
        call: &ToolCall,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let arguments = &call.function.arguments;
        match call.function.name.as_str() {
            "read_file" => {
                let params: ReadFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_read_file(&params.path)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "write_file" => {
                let params: WriteFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_file_write()
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                crate::executor::execute_write_file(&params.path, &params.content)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "execute_command" => {
                let params: ExecuteCommandParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_command(&params.command)
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                crate::executor::execute_command_action(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "search_code" => {
                let params: SearchCodeParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_search_code(&params.query, params.scope.as_deref())
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "call_skill" => {
                let params: CallSkillParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_call_skill(&params.skill_id, &params.parameters)
                    .await
            }
            "run_tests" => {
                let params: RunTestsParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_run_tests(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "verify_build" => {
                let params: VerifyBuildParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_verify_build(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "ask_user" => {
                let params: AskUserParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                Err(ToolExecutionError::AskUser(params.question))
            }
            "delegate_to_agent" => Err(ToolExecutionError::UnknownTool(
                "delegate_to_agent not yet implemented".to_string(),
            )),
            _ => Err(ToolExecutionError::UnknownTool(call.function.name.clone())),
        }
    }

    async fn execute_call_skill(
        &self,
        skill_id: &str,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        if let Some(ref skill_executor) = self.skill_executor {
            let request = SkillExecutionRequest::new(skill_id.to_string(), parameters.clone());
            match skill_executor.execute(request).await {
                Ok(result) => {
                    let mut data = HashMap::new();
                    if let Some(output) = result.output {
                        data.insert("output".to_string(), serde_json::Value::String(output));
                    }
                    if let Some(error) = result.error {
                        data.insert("error".to_string(), serde_json::Value::String(error));
                    }
                    if let Some(exit_code) = result.exit_code {
                        data.insert(
                            "exit_code".to_string(),
                            serde_json::Value::Number(exit_code.into()),
                        );
                    }
                    data.insert(
                        "execution_time_ms".to_string(),
                        serde_json::Value::Number(result.execution_time_ms.into()),
                    );
                    data.insert(
                        "success".to_string(),
                        serde_json::Value::Bool(result.success),
                    );
                    Ok(serde_json::Value::Object(data.into_iter().collect()))
                }
                Err(e) => Err(ToolExecutionError::ExecutionFailed(e.to_string())),
            }
        } else {
            Err(ToolExecutionError::ExecutionFailed(
                "SkillExecutor not configured".to_string(),
            ))
        }
    }

    fn register_builtin_tools(&mut self) {
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ReadFileParams,
            >(
                "read_file",
                "Read the full text content of a file from the given path.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                WriteFileParams,
            >(
                "write_file",
                "Write content to a file at the given path.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ExecuteCommandParams,
            >(
                "execute_command",
                "Execute a shell command with optional working directory and timeout.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                SearchCodeParams,
            >(
                "search_code",
                "Search for code patterns using ripgrep.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                CallSkillParams,
            >(
                "call_skill",
                "Call a registered skill by ID with parameters.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                RunTestsParams,
            >(
                "run_tests",
                "Run a test command (e.g. cargo test) and return results.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                VerifyBuildParams,
            >(
                "verify_build",
                "Run a build verification command (e.g. cargo check) and return results.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                AskUserParams,
            >(
                "ask_user",
                "Ask the user a question when more information is needed to proceed.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                crate::agent::tool_params::DelegateToAgentParams,
            >(
                "delegate_to_agent",
                "Delegate a sub-task to an isolated sub-agent with its own context.",
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        assert!(!registry.definitions().is_empty());
        assert!(registry
            .definitions()
            .iter()
            .any(|d| d.function.name == "read_file"));
    }

    #[test]
    fn test_tool_registry_has_ask_user() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions();
        assert!(defs.iter().any(|d| d.function.name == "ask_user"));
        assert!(defs.iter().any(|d| d.function.name == "delegate_to_agent"));
    }
}
