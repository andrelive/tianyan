//! 技能执行框架。
//!
//! 本模块提供技能执行框架，包括参数验证、安全检查、执行监控和内置技能。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::common::error::{Result, TianyanError};
use crate::executor::security::{check_path_rules, PathCheckOutcome, DEFAULT_BLOCKED_COMMANDS};

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
    /// 文件读取技能最大文件大小。
    pub skill_file_read_max_size: u64,
    /// 文件读取技能超时。
    pub skill_file_read_timeout_secs: u64,
    /// 文件写入技能最大内容大小。
    pub skill_file_write_max_size: u64,
    /// 文件写入技能超时。
    pub skill_file_write_timeout_secs: u64,
    /// 文件列表技能最大条目数。
    pub skill_file_list_max_entries: usize,
    /// HTTP 请求技能超时。
    pub skill_http_timeout_secs: u64,
    /// 系统命令技能超时。
    pub skill_command_timeout_secs: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 30,
            max_timeout_secs: 300,
            max_output_size: 10 * 1024 * 1024, // 10MB
            allow_dangerous_operations: false,
            working_directory: None,
            // 与工具路径共享单一默认源（SecurityPolicy::from_config 同源）
            blocked_commands: DEFAULT_BLOCKED_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            allowed_paths: Vec::new(),
            skill_file_read_max_size: 50 * 1024 * 1024,
            skill_file_read_timeout_secs: 30,
            skill_file_write_max_size: 10 * 1024 * 1024,
            skill_file_write_timeout_secs: 30,
            skill_file_list_max_entries: 10000,
            skill_http_timeout_secs: 60,
            skill_command_timeout_secs: 300,
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
}

/// 验证文件路径是否在允许的路径列表内。
///
/// 委托给共享的 [`check_path_rules`]（空列表 = 放行；规范化 + 组件级前缀匹配；
/// 黑名单绝对优先，本入口不传黑名单）。签名与错误消息保持既有契约。
pub(crate) fn validate_path(path: &Path, allowed_paths: &[PathBuf]) -> Result<()> {
    if allowed_paths.is_empty() {
        return Ok(());
    }

    match check_path_rules(path, allowed_paths, &[]) {
        PathCheckOutcome::Allowed => Ok(()),
        _ => Err(TianyanError::Custom(format!(
            "操作不被允许：路径 '{}' 不在允许的操作范围内",
            path.display()
        ))),
    }
}

impl SkillExecutor {
    /// 创建新的技能执行器。
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, config: ExecutorConfig) -> Self {
        Self { registry, config }
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
                .ok_or_else(|| TianyanError::Custom(format!("技能未找到：{}", request.skill_id)))?
        };

        // 检查技能是否启用
        if !skill.enabled {
            return Err(TianyanError::Custom(format!(
                "技能执行错误：技能 '{}' 已禁用",
                request.skill_id
            )));
        }

        // 验证参数
        self.validate_parameters(&skill, &request.parameters)?;

        // 安全检查
        self.security_check(&skill, &request.parameters, &request.context)?;

        // 获取处理程序
        // 已学习技能（GEPA 产物）无内置 handler：返回说明指引，
        // 由 LLM 参考技能描述后使用基础工具执行，而不是报硬错误。
        let handler = {
            let registry = self.registry.read().await;
            match registry.get_handler(&request.skill_id) {
                Some(h) => h,
                None => {
                    if skill.tags.iter().any(|t| t == "learned") {
                        return Ok(SkillExecutionResult {
                            success: true,
                            output: Some(format!(
                                "技能 '{}' 为学习型技能（无内置处理器）。\n\n{}",
                                skill.id, skill.description
                            )),
                            error: None,
                            exit_code: None,
                            execution_time_ms: 0,
                            data: HashMap::new(),
                        });
                    }
                    return Err(TianyanError::Custom(format!(
                        "技能执行错误：技能 '{}' 没有注册处理程序",
                        request.skill_id
                    )));
                }
            }
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
                return Err(TianyanError::Custom(format!(
                    "[{}] 缺少必需参数: {}",
                    skill.id, required
                )));
            }
        }

        Ok(())
    }

    /// 执行安全检查。
    ///
    /// 安全域与工具路径统一（候选 2）：危险判定只由 [`SecurityLevel`] 门控
    /// （Safe/Moderate 放行、Dangerous 需显式许可），命令拦截由 handler 的
    /// blocklist（与工具路径共享 [`DEFAULT_BLOCKED_COMMANDS`] 源）承担；
    /// 参数内容注入扫描已移除——它与工具路径（execute_write_file 等）语义
    /// 分歧，且误伤合法内容（如写入含 `&&` 的脚本）。
    fn security_check(
        &self,
        skill: &Skill,
        _params: &HashMap<String, Value>,
        _context: &ExecutionContext,
    ) -> Result<()> {
        match skill.security_level {
            SecurityLevel::Safe | SecurityLevel::Moderate => {
                // 安全/中等风险操作放行：参数 schema 校验 + handler 沙箱
                // （allowed_paths / max_size / blocklist）已承担管控。
                Ok(())
            }
            SecurityLevel::Dangerous => {
                // 危险操作需要明确允许（配置 security.allow_dangerous_skills）
                if !self.config.allow_dangerous_operations {
                    return Err(TianyanError::Custom(format!(
                        "操作不被允许：技能 '{}' 需要危险操作权限，当前未允许",
                        skill.id
                    )));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::security::normalize_path_for_check;
    use crate::skills::definition::{ParameterDefinition, ParameterType};
    use crate::skills::registry::{create_builtin_skills, register_builtin_skills};
    use crate::skills::types::SkillCategory;
    use tempfile::tempdir;

    #[test]
    fn test_validate_path_empty_allowed_is_unrestricted() {
        let p = PathBuf::from("/anywhere");
        assert!(validate_path(&p, &[]).is_ok());
    }

    #[test]
    fn test_validate_path_allows_inside_allowed() {
        let dir = tempdir().unwrap();
        let inside = dir.path().join("sub").join("file.txt");
        assert!(validate_path(&inside, &[dir.path().to_path_buf()]).is_ok());
    }

    #[test]
    fn test_validate_path_rejects_outside_allowed() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let p = outside.path().join("file.txt");
        let err = validate_path(&p, &[dir.path().to_path_buf()]).unwrap_err();
        assert!(err.to_string().contains("不在允许的操作范围内"));
    }

    #[test]
    fn test_validate_path_rejects_prefix_sibling() {
        // 允许目录 /ab 时，/abc 不得被视为其子路径
        let dir = tempdir().unwrap();
        let base = dir.path().join("ab");
        let sibling = dir.path().join("abc");
        std::fs::create_dir(&base).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        assert!(validate_path(&base.join("x.txt"), std::slice::from_ref(&base)).is_ok());
        let err = validate_path(&sibling.join("x.txt"), &[base]).unwrap_err();
        assert!(err.to_string().contains("不在允许的操作范围内"));
    }

    #[test]
    fn test_normalize_for_compare_strips_verbatim_prefix() {
        let win = PathBuf::from(r"\\?\C:\Users\me\dir");
        assert_eq!(
            normalize_path_for_check(&win),
            PathBuf::from(r"C:\Users\me\dir")
        );
        let unc = PathBuf::from(r"\\?\UNC\server\share");
        assert_eq!(
            normalize_path_for_check(&unc),
            PathBuf::from(r"\\server\share")
        );
        let plain = PathBuf::from(r"C:\Users\me\dir");
        assert_eq!(normalize_path_for_check(&plain), plain);
    }

    #[test]
    fn test_validate_path_verbatim_allowed_compared_consistently() {
        // 模拟 Windows canonicalize 产生的 \\?\ 前缀路径仍应匹配普通 allowed 路径
        let dir = tempdir().unwrap();
        let verbatim = PathBuf::from(format!(
            r"\\?\{}",
            dir.path().join("x.txt").to_string_lossy()
        ));
        assert!(validate_path(&verbatim, &[dir.path().to_path_buf()]).is_ok());
    }

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

    /// 回归测试：内置技能注册后必须保留参数 schema 与安全级别。
    ///
    /// 曾因先注册带元数据的技能、再用裸 `Skill::new` 覆盖注册，
    /// 导致所有内置技能的参数定义与安全级别静默丢失。
    #[test]
    fn test_builtin_skills_keep_metadata_after_registration() {
        let mut registry = SkillRegistry::new();
        let config = ExecutorConfig::new();
        register_builtin_skills(&mut registry, &config);

        assert_eq!(registry.count(), 7, "内置技能：6 执行型 + planning 软约束");

        let file_write = registry.get("file_write").unwrap();
        assert_eq!(file_write.category, SkillCategory::FileOperations);
        assert_eq!(file_write.security_level, SecurityLevel::Moderate);
        let params = file_write.parameters.as_ref().unwrap();
        assert!(
            params.required.contains(&"path".to_string()),
            "path 应为必填参数"
        );
        assert!(
            params.required.contains(&"content".to_string()),
            "content 应为必填参数"
        );
        assert!(
            params.properties.contains_key("content"),
            "content 参数定义应保留"
        );

        let system_command = registry.get("system_command").unwrap();
        assert_eq!(
            system_command.security_level,
            SecurityLevel::Dangerous,
            "system_command 安全级别应保留"
        );

        // 处理器必须与元数据技能绑定
        assert!(registry.get_handler("file_write").is_some());
        assert!(registry.get_handler("http_request").is_some());
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

    /// 回归测试：GEPA 学习技能（无 handler，带 learned 标签）执行时
    /// 返回说明指引而非硬错误 —— 闭合学习回路。
    #[tokio::test]
    async fn test_learned_skill_execution_returns_guidance() {
        use crate::skills::types::SkillCategory;

        let mut registry = SkillRegistry::new();
        registry.register(
            Skill::new("learned_x", "Learned X", "基于成功执行生成的流程说明")
                .with_category(SkillCategory::Custom)
                .with_tag("learned"),
        );

        let registry = Arc::new(RwLock::new(registry));
        let executor = SkillExecutor::new(registry.clone(), ExecutorConfig::new());

        let result = executor
            .execute(SkillExecutionRequest::new("learned_x", HashMap::new()))
            .await
            .unwrap();

        assert!(result.success, "学习技能执行应返回成功（说明指引）");
        let output = result.output.unwrap_or_default();
        assert!(
            output.contains("学习型技能"),
            "输出应包含学习型技能指引: {output}"
        );
        assert!(output.contains("learned_x"));
    }

    /// 回归测试：无 handler 且非学习技能仍应报错（防止误放行普通技能）。
    #[tokio::test]
    async fn test_unhandled_non_learned_skill_still_errors() {
        let mut registry = SkillRegistry::new();
        registry.register(Skill::new("orphan", "Orphan", "无处理器技能"));

        let registry = Arc::new(RwLock::new(registry));
        let executor = SkillExecutor::new(registry.clone(), ExecutorConfig::new());

        let err = executor
            .execute(SkillExecutionRequest::new("orphan", HashMap::new()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("没有注册处理程序"));
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
