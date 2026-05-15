use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum Action {
    ReadFile { path: String },
    WriteFile { path: String, content: String },
    ExecuteCommand { command: String, cwd: Option<String>, timeout_secs: Option<u64> },
    SearchCode { query: String, scope: Option<String> },
    #[serde(skip)]
    SubPlanner { task: String },
    CallSkill { skill_id: String, parameters: serde_json::Map<String, Value> },
    RunTests { command: String, cwd: Option<String>, timeout_secs: Option<u64> },
    VerifyBuild { command: String, cwd: Option<String>, timeout_secs: Option<u64> },
}

#[derive(thiserror::Error, Debug)]
pub enum ExecutorError {
    #[error("文件操作失败：{0}")]
    FileError(String),
    #[error("命令执行失败：{0}")]
    CommandError(String),
    #[error("搜索失败：{0}")]
    SearchError(String),
    #[error("子 Planner 失败：{0}")]
    SubPlannerError(String),
    #[error("执行超时")]
    Timeout,
    #[error("安全策略违规：{0}")]
    SecurityViolation(String),
    #[error("技能执行失败：{0}")]
    SkillExecution(String),
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
        let err = ExecutorError::Timeout;
        assert!(err.to_string().contains("执行超时"));
    }

    #[test]
    fn test_action_serialization() {
        let action = Action::ReadFile { path: "test.txt".to_string() };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("ReadFile"));
    }
}
