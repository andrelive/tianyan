use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 执行器可执行的动作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum Action {
    /// 读取文件。
    ReadFile {
        /// 文件路径。
        path: String,
    },
    /// 写入文件。
    WriteFile {
        /// 文件路径。
        path: String,
        /// 写入内容。
        content: String,
    },
    /// 执行命令。
    ExecuteCommand {
        /// 命令字符串。
        command: String,
        /// 工作目录。
        cwd: Option<String>,
        /// 超时时间（秒）。
        timeout_secs: Option<u64>,
    },
    /// 搜索代码。
    SearchCode {
        /// 搜索查询。
        query: String,
        /// 搜索范围。
        scope: Option<String>,
    },
    /// 调用技能。
    CallSkill {
        /// 技能 ID。
        skill_id: String,
        /// 技能参数。
        parameters: serde_json::Map<String, Value>,
    },
    /// 运行测试。
    RunTests {
        /// 测试命令。
        command: String,
        /// 工作目录。
        cwd: Option<String>,
        /// 超时时间（秒）。
        timeout_secs: Option<u64>,
    },
    /// 验证构建。
    VerifyBuild {
        /// 构建命令。
        command: String,
        /// 工作目录。
        cwd: Option<String>,
        /// 超时时间（秒）。
        timeout_secs: Option<u64>,
    },
}

/// 执行器错误。
/// 执行器错误。
#[derive(thiserror::Error, Debug)]
pub enum ExecutorError {
    /// 文件操作失败。
    #[error("文件操作失败：{0}")]
    FileError(String),
    /// 命令执行失败。
    #[error("命令执行失败：{0}")]
    CommandError(String),
    /// 搜索失败。
    #[error("搜索失败：{0}")]
    SearchError(String),
    /// 执行超时。
    #[error("执行超时")]
    Timeout,
    /// 安全策略违规。
    #[error("安全策略违规：{0}")]
    SecurityViolation(String),
    /// 技能执行失败。
    #[error("技能执行失败：{0}")]
    SkillExecution(String),
    /// 内部错误。
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
        let action = Action::ReadFile {
            path: "test.txt".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("ReadFile"));
    }
}
