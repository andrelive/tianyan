//! 技能类型定义。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 技能分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SkillCategory {
    /// 文件操作。
    FileOperations,
    /// 系统命令。
    SystemCommands,
    /// 网络请求。
    NetworkRequests,
    /// 应用程序控制。
    ApplicationControl,
    /// 数据处理。
    DataProcessing,
    /// 自定义技能。
    #[default]
    Custom,
}

impl SkillCategory {
    /// 获取显示名称。
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FileOperations => "File Operations",
            Self::SystemCommands => "System Commands",
            Self::NetworkRequests => "Network Requests",
            Self::ApplicationControl => "Application Control",
            Self::DataProcessing => "Data Processing",
            Self::Custom => "Custom",
        }
    }
}

/// 技能安全级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SecurityLevel {
    /// 安全操作（只读）。
    #[default]
    Safe,
    /// 中等风险操作。
    Moderate,
    /// 高风险操作（需要确认）。
    Dangerous,
}

impl SecurityLevel {
    /// 获取显示名称。
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Safe => "Safe",
            Self::Moderate => "Moderate",
            Self::Dangerous => "Dangerous",
        }
    }
}

/// 技能使用示例。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExample {
    /// 示例描述。
    pub description: String,
    /// 示例参数。
    pub parameters: HashMap<String, Value>,
    /// 预期结果。
    pub result: Option<String>,
}

impl SkillExample {
    /// 创建新的技能示例。
    pub fn new(description: impl Into<String>, parameters: HashMap<String, Value>) -> Self {
        Self {
            description: description.into(),
            parameters,
            result: None,
        }
    }

    /// 设置预期结果。
    pub fn with_result(mut self, result: impl Into<String>) -> Self {
        self.result = Some(result.into());
        self
    }
}

/// 技能执行请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionRequest {
    /// 技能 ID。
    pub skill_id: String,
    /// 执行参数。
    pub parameters: HashMap<String, Value>,
    /// 执行上下文。
    #[serde(default)]
    pub context: ExecutionContext,
}

impl SkillExecutionRequest {
    /// 创建新的执行请求。
    pub fn new(skill_id: impl Into<String>, parameters: HashMap<String, Value>) -> Self {
        Self {
            skill_id: skill_id.into(),
            parameters,
            context: ExecutionContext::default(),
        }
    }

    /// 设置执行上下文。
    pub fn with_context(mut self, context: ExecutionContext) -> Self {
        self.context = context;
        self
    }
}

/// 执行上下文。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// 工作目录。
    pub working_directory: Option<String>,
    /// 超时时间（秒）。
    pub timeout: Option<u64>,
    /// 环境变量。
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// 是否捕获输出。
    #[serde(default = "default_true")]
    pub capture_output: bool,
    /// 用于权限检查的用户 ID。
    pub user_id: Option<String>,
    /// 用于跟踪的会话 ID。
    pub session_id: Option<String>,
}

impl ExecutionContext {
    /// 创建新的执行上下文。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置工作目录。
    pub fn with_working_directory(mut self, dir: impl Into<String>) -> Self {
        self.working_directory = Some(dir.into());
        self
    }

    /// 设置超时时间。
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout = Some(timeout_secs);
        self
    }

    /// 添加环境变量。
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }
}

/// 技能执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionResult {
    /// 执行是否成功。
    pub success: bool,
    /// 执行输出。
    pub output: Option<String>,
    /// 失败时的错误消息。
    pub error: Option<String>,
    /// 退出码（用于命令执行）。
    pub exit_code: Option<i32>,
    /// 执行时间（毫秒）。
    pub execution_time_ms: u64,
    /// 额外的结果数据。
    #[serde(default)]
    pub data: HashMap<String, Value>,
}

impl SkillExecutionResult {
    /// 创建成功结果。
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
            exit_code: None,
            execution_time_ms: 0,
            data: HashMap::new(),
        }
    }

    /// 创建失败结果。
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
            exit_code: None,
            execution_time_ms: 0,
            data: HashMap::new(),
        }
    }

    /// 设置执行时间。
    pub fn with_execution_time(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }

    /// 设置退出码。
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self
    }

    /// 添加额外数据。
    pub fn with_data(mut self, key: impl Into<String>, value: Value) -> Self {
        self.data.insert(key.into(), value);
        self
    }
}

/// 技能发现结果。
#[derive(Debug, Clone)]
pub struct SkillDiscoveryResult {
    /// 技能。
    pub skill: super::definition::Skill,
    /// 相似度分数（用于语义搜索）。
    pub similarity: f32,
    /// 匹配原因。
    pub match_reason: MatchReason,
}

/// 技能匹配原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchReason {
    /// 精确 ID 匹配。
    ExactId,
    /// 名称包含查询。
    NameMatch,
    /// 标签匹配。
    TagMatch,
    /// 语义相似度。
    SemanticSimilarity,
    /// 分类匹配。
    CategoryMatch,
}

/// 技能统计信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillStatistics {
    /// 技能总数。
    pub total_skills: usize,
    /// 已启用的技能数。
    pub enabled_skills: usize,
    /// 按分类统计的技能数。
    pub by_category: HashMap<String, usize>,
    /// 按安全级别统计的技能数。
    pub by_security_level: HashMap<String, usize>,
    /// 总执行次数。
    pub total_executions: u64,
    /// 成功执行次数。
    pub successful_executions: u64,
    /// 失败执行次数。
    pub failed_executions: u64,
    /// 平均执行时间（毫秒）。
    pub avg_execution_time_ms: f64,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_category() {
        assert_eq!(
            SkillCategory::FileOperations.display_name(),
            "File Operations"
        );
        assert_eq!(SkillCategory::default(), SkillCategory::Custom);
    }

    #[test]
    fn test_security_level() {
        assert_eq!(SecurityLevel::Safe.display_name(), "Safe");
        assert_eq!(SecurityLevel::default(), SecurityLevel::Safe);
    }

    #[test]
    fn test_execution_context() {
        let ctx = ExecutionContext::new()
            .with_working_directory("/tmp")
            .with_timeout(30)
            .with_env("PATH", "/usr/bin");

        assert_eq!(ctx.working_directory, Some("/tmp".to_string()));
        assert_eq!(ctx.timeout, Some(30));
        assert_eq!(ctx.environment.get("PATH"), Some(&"/usr/bin".to_string()));
    }

    #[test]
    fn test_skill_execution_result() {
        let result = SkillExecutionResult::success("Hello, world!").with_execution_time(100);

        assert!(result.success);
        assert_eq!(result.output, Some("Hello, world!".to_string()));
        assert_eq!(result.execution_time_ms, 100);

        let failure = SkillExecutionResult::failure("Something went wrong");
        assert!(!failure.success);
        assert_eq!(failure.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_skill_example() {
        let mut params = HashMap::new();
        params.insert("path".to_string(), Value::String("/test".to_string()));

        let example = SkillExample::new("Read a file", params.clone()).with_result("File contents");

        assert_eq!(example.description, "Read a file");
        assert_eq!(example.result, Some("File contents".to_string()));
    }

    #[test]
    fn test_skill_execution_request() {
        let mut params = HashMap::new();
        params.insert("path".to_string(), Value::String("/test".to_string()));

        let request = SkillExecutionRequest::new("file_read", params)
            .with_context(ExecutionContext::new().with_timeout(60));

        assert_eq!(request.skill_id, "file_read");
        assert_eq!(request.context.timeout, Some(60));
    }
}
