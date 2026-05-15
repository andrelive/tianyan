//! 技能执行框架。
//!
//! 本模块提供技能执行框架，包括参数验证、安全检查、执行监控和内置技能。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::common::error::{Result, TianyanError};

use super::definition::{Skill, SkillRegistry};
use super::types::{ExecutionContext, SecurityLevel, SkillExecutionRequest, SkillExecutionResult};

/// 技能执行器配置。
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// 默认超时时间（秒）。
    pub default_timeout_secs: u64,
    /// 最大超时时间（秒）。
    pub max_timeout_secs: u64,
    /// 最大输出大小（字节）。
    pub max_output_size: usize,
    /// 是否允许危险操作。
    pub allow_dangerous_operations: bool,
    /// 执行工作目录。
    pub working_directory: Option<PathBuf>,
    /// 阻止的命令（用于系统命令技能）。
    pub blocked_commands: Vec<String>,
    /// 允许的文件操作路径。
    pub allowed_paths: Vec<PathBuf>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 30,
            max_timeout_secs: 300,
            max_output_size: 10 * 1024 * 1024, // 10MB
            allow_dangerous_operations: false,
            working_directory: None,
            blocked_commands: vec![
                "rm".to_string(),
                "del".to_string(),
                "format".to_string(),
                "mkfs".to_string(),
                "dd".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "powershell".to_string(),
                "python".to_string(),
                "python3".to_string(),
                "curl".to_string(),
                "wget".to_string(),
                "nc".to_string(),
                "netcat".to_string(),
            ],
            allowed_paths: Vec::new(),
        }
    }
}

impl ExecutorConfig {
    /// 创建新的执行器配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置默认超时时间。
    pub fn with_default_timeout(mut self, secs: u64) -> Self {
        self.default_timeout_secs = secs.min(self.max_timeout_secs);
        self
    }

    /// 设置是否允许危险操作。
    pub fn with_dangerous_operations(mut self, allow: bool) -> Self {
        self.allow_dangerous_operations = allow;
        self
    }

    /// 设置工作目录。
    pub fn with_working_directory(mut self, path: PathBuf) -> Self {
        self.working_directory = Some(path);
        self
    }
}

/// 安全执行技能的技能执行器。
pub struct SkillExecutor {
    registry: Arc<RwLock<SkillRegistry>>,
    config: ExecutorConfig,
    execution_log: Arc<RwLock<Vec<ExecutionLogEntry>>>,
}

/// 执行日志条目。
#[derive(Debug, Clone)]
pub struct ExecutionLogEntry {
    /// 技能 ID。
    pub skill_id: String,
    /// 执行时间戳。
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 执行是否成功。
    pub success: bool,
    /// 执行时间（毫秒）。
    pub execution_time_ms: u64,
    /// 错误消息（如果失败）。
    pub error: Option<String>,
    /// 使用的参数。
    pub parameters: HashMap<String, Value>,
}

/// 验证文件路径是否在允许的路径列表内。
pub(crate) fn validate_path(path: &PathBuf, allowed_paths: &[PathBuf]) -> Result<()> {
    if allowed_paths.is_empty() {
        return Ok(());
    }

    // 规范化路径：使用绝对路径并清理 `.` 和 `..`
    let canonical = path
        .canonicalize()
        .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(path)))
        .map_err(|e| {
            TianyanError::OperationNotAllowed(format!("无法解析路径 '{}': {}", path.display(), e))
        })?;

    for allowed in allowed_paths {
        let allowed_canonical = allowed.canonicalize().unwrap_or_else(|_| allowed.clone());
        if canonical.starts_with(&allowed_canonical) {
            return Ok(());
        }
    }

    Err(TianyanError::OperationNotAllowed(format!(
        "路径 '{}' 不在允许的操作范围内",
        path.display()
    )))
}

impl SkillExecutor {
    /// 创建新的技能执行器。
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, config: ExecutorConfig) -> Self {
        Self {
            registry,
            config,
            execution_log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 使用默认配置创建执行器。
    pub fn with_defaults(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self::new(registry, ExecutorConfig::default())
    }

    /// 执行技能。
    pub async fn execute(&self, request: SkillExecutionRequest) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        // 获取技能
        let skill = {
            let registry = self.registry.read().await;
            registry
                .get(&request.skill_id)
                .cloned()
                .ok_or_else(|| TianyanError::SkillNotFound(request.skill_id.clone()))?
        };

        // 检查技能是否启用
        if !skill.enabled {
            return Err(TianyanError::SkillExecution(format!(
                "技能 '{}' 已禁用",
                request.skill_id
            )));
        }

        // 验证参数
        self.validate_parameters(&skill, &request.parameters)?;

        // 安全检查
        self.security_check(&skill, &request.parameters, &request.context)?;

        // 获取处理程序
        let handler = {
            let registry = self.registry.read().await;
            registry.get_handler(&request.skill_id).ok_or_else(|| {
                TianyanError::SkillExecution(format!(
                    "技能 '{}' 没有注册处理程序",
                    request.skill_id
                ))
            })?
        };

        // 带超时执行
        let timeout_secs = request
            .context
            .timeout
            .unwrap_or(self.config.default_timeout_secs)
            .min(self.config.max_timeout_secs);

        let execution_result = timeout(
            Duration::from_secs(timeout_secs),
            handler.execute(request.parameters.clone(), request.context),
        )
        .await;

        let result = match execution_result {
            Ok(Ok(mut result)) => {
                // 如需要则截断输出
                if let Some(ref output) = result.output {
                    if output.len() > self.config.max_output_size {
                        result.output = Some(output[..self.config.max_output_size].to_string());
                    }
                }
                result
            }
            Ok(Err(e)) => SkillExecutionResult {
                success: false,
                output: None,
                error: Some(e.to_string()),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            },
            Err(_) => SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("执行 {} 秒后超时", timeout_secs)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            },
        };

        // 记录执行
        self.log_execution(
            request.skill_id,
            request.parameters,
            result.success,
            start.elapsed().as_millis() as u64,
            result.error.clone(),
        )
        .await;

        Ok(result)
    }

    /// 根据技能模式验证参数。
    fn validate_parameters(&self, skill: &Skill, params: &HashMap<String, Value>) -> Result<()> {
        if let Some(ref schema) = skill.parameters {
            schema.validate(params)?;
        }

        // 检查必需参数
        for required in &skill.required_parameters {
            if !params.contains_key(required) {
                return Err(TianyanError::InvalidSkillParameters {
                    skill: skill.id.clone(),
                    message: format!("缺少必需参数: {}", required),
                });
            }
        }

        Ok(())
    }

    /// 执行安全检查。
    fn security_check(
        &self,
        skill: &Skill,
        params: &HashMap<String, Value>,
        _context: &ExecutionContext,
    ) -> Result<()> {
        match skill.security_level {
            SecurityLevel::Safe => {
                // 安全操作始终允许
                Ok(())
            }
            SecurityLevel::Moderate => {
                // 中等风险操作需要一些检查
                if !self.config.allow_dangerous_operations {
                    // 检查潜在危险的参数值
                    for value in params.values() {
                        if let Value::String(s) = value {
                            // 检查命令注入模式
                            let dangerous_patterns = [
                                "&&", "||", "|", ";", "`", "$()", ">>", ">", "<", "$(", "${",
                                "eval", "exec",
                            ];
                            if dangerous_patterns.iter().any(|p| s.contains(p)) {
                                return Err(TianyanError::OperationNotAllowed(
                                    "检测到潜在危险的参数值".to_string(),
                                ));
                            }
                        }
                    }
                }
                Ok(())
            }
            SecurityLevel::Dangerous => {
                // 危险操作需要明确允许
                if !self.config.allow_dangerous_operations {
                    return Err(TianyanError::OperationNotAllowed(format!(
                        "技能 '{}' 需要危险操作权限，当前未允许",
                        skill.id
                    )));
                }
                Ok(())
            }
        }
    }

    /// 记录执行。
    async fn log_execution(
        &self,
        skill_id: String,
        parameters: HashMap<String, Value>,
        success: bool,
        execution_time_ms: u64,
        error: Option<String>,
    ) {
        let entry = ExecutionLogEntry {
            skill_id,
            timestamp: chrono::Utc::now(),
            success,
            execution_time_ms,
            error,
            parameters,
        };

        let mut log = self.execution_log.write().await;
        log.push(entry);

        // 只保留最近 1000 条记录
        if log.len() > 1000 {
            let excess = log.len() - 1000;
            log.drain(0..excess);
        }
    }

    /// 获取执行日志。
    pub async fn get_execution_log(&self) -> Vec<ExecutionLogEntry> {
        self.execution_log.read().await.clone()
    }

    /// 清除执行日志。
    pub async fn clear_execution_log(&self) {
        self.execution_log.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::definition::{ParameterDefinition, ParameterType};
    use crate::skills::registry::{create_builtin_skills, register_builtin_skills};
    use crate::skills::types::SkillCategory;

    #[test]
    fn test_executor_config() {
        let config = ExecutorConfig::new()
            .with_default_timeout(60)
            .with_dangerous_operations(true);

        assert_eq!(config.default_timeout_secs, 60);
        assert!(config.allow_dangerous_operations);
    }

    #[test]
    fn test_create_builtin_skills() {
        let skills = create_builtin_skills();
        assert!(!skills.is_empty());

        let file_read = skills.iter().find(|s| s.id == "file_read");
        assert!(file_read.is_some());
        assert_eq!(file_read.unwrap().category, SkillCategory::FileOperations);
    }

    #[tokio::test]
    async fn test_skill_executor() {
        let mut registry = SkillRegistry::new();
        let config = ExecutorConfig::new();

        register_builtin_skills(&mut registry, &config);

        let registry = Arc::new(RwLock::new(registry));
        let executor = SkillExecutor::new(registry.clone(), config);

        // 测试文件列表（安全操作）
        let request = SkillExecutionRequest {
            skill_id: "file_list".to_string(),
            parameters: {
                let mut params = HashMap::new();
                params.insert("path".to_string(), Value::String(".".to_string()));
                params
            },
            context: ExecutionContext::default(),
        };

        let result = executor.execute(request).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_parameter_definition_validation() {
        let def = ParameterDefinition::new(ParameterType::String)
            .with_min_length(3)
            .with_max_length(10);

        // 有效
        assert!(def
            .validate("test", &Value::String("hello".to_string()))
            .is_ok());

        // 太短
        assert!(def
            .validate("test", &Value::String("hi".to_string()))
            .is_err());

        // 太长
        assert!(def
            .validate("test", &Value::String("this is too long".to_string()))
            .is_err());

        // 类型错误
        assert!(def.validate("test", &Value::Number(123.into())).is_err());
    }
}
