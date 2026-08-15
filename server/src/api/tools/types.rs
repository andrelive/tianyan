use serde::Serialize;

/// 系统工具信息（工具目录展示：名称/描述/参数 JSON Schema）。
#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    /// 工具名称。
    pub name: String,
    /// 工具描述（LLM 提示词同源）。
    pub description: String,
    /// 参数 JSON Schema。
    pub parameters: serde_json::Value,
}

/// 列出工具响应。
#[derive(Debug, Serialize)]
pub struct ListToolsResponse {
    /// 工具列表。
    pub tools: Vec<ToolInfo>,
    /// 工具总数。
    pub total: usize,
}
