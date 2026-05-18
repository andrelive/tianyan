use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 读取文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileParams {
    /// 文件路径。
    pub path: String,
}

/// 写入文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileParams {
    /// 文件路径。
    pub path: String,
    /// 文件内容。
    pub content: String,
}

/// 执行命令参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteCommandParams {
    /// 命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 搜索代码参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    /// 查询字符串。
    pub query: String,
    /// 搜索范围。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 调用技能参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallSkillParams {
    /// 技能 ID。
    pub skill_id: String,
    /// 参数。
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
}

/// 运行测试参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunTestsParams {
    /// 测试命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 验证构建参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerifyBuildParams {
    /// 构建命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 追问用户参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserParams {
    /// 问题内容。
    pub question: String,
}

/// 委托子 Agent 参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateToAgentParams {
    /// 子任务描述。
    pub task: String,
    /// 系统提示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 最大轮数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_file_params_schema() {
        let def = crate::model::FunctionDefinition::from_schema::<ReadFileParams>(
            "read_file",
            "Read a file",
        );
        assert_eq!(def.name, "read_file");
        assert!(def.parameters.get("properties").is_some());
    }

    #[test]
    fn test_ask_user_params_serialization() {
        let params = AskUserParams {
            question: "What is your name?".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("What is your name?"));
    }
}
