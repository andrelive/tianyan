use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 执行步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// 步骤 ID（在同轮计划中唯一）
    pub step_id: usize,
    /// 步骤描述（给 LLM 看的，帮助理解这个步骤的目的）
    pub description: String,
    /// 具体执行的动作
    pub action: Action,
    /// 失败处理策略
    pub on_failure: FailureHandling,
    /// Planner 预判的重要性（0-1）
    pub expected_importance: f32,
}

/// 动作类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum Action {
    /// 文件读取
    ReadFile { path: String },
    /// 文件写入
    WriteFile { path: String, content: String },
    /// 命令执行
    ExecuteCommand {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// 代码搜索
    SearchCode {
        query: String,
        scope: Option<String>,
    },
    /// 子 Planner（递归执行，最大深度由 PlannerConfig::max_recursion_depth 控制）
    SubPlanner { task: String },
    /// 调用已注册的技能
    CallSkill {
        skill_id: String,
        parameters: serde_json::Map<String, Value>,
    },
    /// 运行测试套件（如 cargo test）
    RunTests {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// 验证构建（如 cargo build / cargo check / cargo clippy）
    VerifyBuild {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
}

/// 失败处理策略
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FailureHandling {
    /// 忽略，继续执行
    #[serde(rename = "Ignore")]
    Ignore,
    /// 报错，终止执行
    #[serde(rename = "Abort")]
    Abort {
        #[serde(rename = "error_message")]
        error_message: String,
    },
    /// 重试
    #[serde(rename = "Retry")]
    Retry {
        #[serde(rename = "max_retries")]
        max_retries: usize,
    },
}

/// 步骤执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// 对应的步骤 ID
    pub step_id: usize,
    /// 是否执行成功
    pub success: bool,
    /// 执行结果（JSON 格式）
    pub output: Value,
    /// 错误信息（如果执行失败）
    pub error: Option<String>,
    /// 实际重要性（可选，用于 Executor 修正）
    pub actual_importance: Option<f32>,
}

/// Executor 错误类型
#[derive(thiserror::Error, Debug)]
pub enum ExecutorError {
    /// 文件操作失败
    #[error("文件操作失败：{0}")]
    FileError(String),
    /// 命令执行失败
    #[error("命令执行失败：{0}")]
    CommandError(String),
    /// 搜索失败
    #[error("搜索失败：{0}")]
    SearchError(String),
    /// 子 Planner 失败
    #[error("子 Planner 失败：{0}")]
    SubPlannerError(String),
    /// 超时
    #[error("执行超时")]
    Timeout,
    /// 安全策略违规
    #[error("安全策略违规：{0}")]
    SecurityViolation(String),
    /// 技能执行失败
    #[error("技能执行失败：{0}")]
    SkillExecution(String),
    /// 内部错误（架构约束违反）
    #[error("内部错误：{0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_display() {
        let err = ExecutorError::FileError("文件不存在".to_string());
        assert!(err.to_string().contains("文件操作失败"));
        assert!(err.to_string().contains("文件不存在"));

        let err = ExecutorError::Timeout;
        assert!(err.to_string().contains("执行超时"));
    }
}
