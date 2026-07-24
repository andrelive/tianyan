use serde::{Deserialize, Serialize};

/// 模型产生的工具调用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// 工具调用 ID。
    pub id: String,
    /// 工具调用类型。
    #[serde(rename = "type")]
    pub call_type: ToolCallType,
    /// 函数调用详情。
    pub function: FunctionCall,
}

/// 工具调用类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallType {
    /// 函数调用类型。
    Function,
}

/// 模型发出的函数调用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// 函数名称。
    pub name: String,
    /// 函数参数（JSON 字符串）。
    pub arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_serialization() {
        let call = ToolCall {
            id: "call_abc".to_string(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/test"}"#.to_string(),
            },
        };
        let json = serde_json::to_string(&call).unwrap();
        assert!(json.contains("call_abc"));
        assert!(json.contains("read_file"));
    }
}
