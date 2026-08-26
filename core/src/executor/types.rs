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
    /// 应用精确编辑（哈希锚定行编辑）。
    ApplyEdit {
        /// 文件路径。
        path: String,
        /// 编辑规格列表（JSON 序列化的 [`crate::executor::edit::ContentEdit`]）。
        edits: Vec<Value>,
    },
    /// 应用统一 diff 补丁（codex 风格 `*** Update File` 信封格式，可含多文件）。
    ApplyPatch {
        /// 补丁涉及的主要文件路径（首个 `*** Update File:` 头部；审批展示用）。
        path: String,
        /// 补丁文本（可能包含多个文件的补丁块）。
        patch: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_serialization() {
        let action = Action::ReadFile {
            path: "test.txt".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("ReadFile"));
    }
}
