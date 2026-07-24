use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::common::types::tool::{FunctionCall, ToolCall, ToolCallType};

/// 工具定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// 工具类型。
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    /// 函数定义。
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// 创建函数工具。
    pub fn function(function: FunctionDefinition) -> Self {
        Self {
            tool_type: ToolType::Function,
            function,
        }
    }
}

/// 工具类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    /// 函数调用类型。
    Function,
}

/// 函数定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名称。
    pub name: String,
    /// 函数描述。
    pub description: String,
    /// 函数参数 Schema。
    pub parameters: serde_json::Value,
}

impl FunctionDefinition {
    /// 创建函数定义。
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// 从 JSON Schema 创建函数定义。
    pub fn from_schema<T: JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let schema = schemars::schema_for!(T);
        Self {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::to_value(&schema).unwrap_or_else(|_| serde_json::json!({})),
        }
    }
}

/// 工具调用选择策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// 自动选择。
    Auto,
    /// 禁止调用。
    None,
    /// 强制调用指定函数。
    Function {
        /// 指定的函数。
        function: ToolChoiceFunction,
    },
}

/// 强制调用的函数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoiceFunction {
    /// 函数名称。
    pub name: String,
}

impl ToolChoice {
    /// 自动选择策略。
    pub fn auto() -> Self {
        Self::Auto
    }

    /// 禁止调用策略。
    pub fn none() -> Self {
        Self::None
    }

    /// 强制调用指定函数。
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function {
            function: ToolChoiceFunction { name: name.into() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(schemars::JsonSchema, serde::Serialize)]
    struct TestParams {
        query: String,
        limit: Option<usize>,
    }

    #[test]
    fn test_tool_definition_creation() {
        let func = FunctionDefinition::new(
            "test_func",
            "A test function",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                }
            }),
        );
        let tool = ToolDefinition::function(func);
        assert_eq!(tool.tool_type, ToolType::Function);
        assert_eq!(tool.function.name, "test_func");
    }

    #[test]
    fn test_function_definition_from_schema() {
        let func = FunctionDefinition::from_schema::<TestParams>("search", "Search for items");
        assert_eq!(func.name, "search");
        assert_eq!(func.description, "Search for items");
        let params = func.parameters;
        assert!(params.get("properties").is_some());
    }

    #[test]
    fn test_tool_call_creation() {
        let call = ToolCall {
            id: "call_123".to_string(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "search".to_string(),
                arguments: r#"{"query":"hello"}"#.to_string(),
            },
        };
        assert_eq!(call.id, "call_123");
        assert_eq!(call.function.name, "search");
    }

    #[test]
    fn test_tool_choice_variants() {
        assert!(matches!(ToolChoice::auto(), ToolChoice::Auto));
        assert!(matches!(ToolChoice::none(), ToolChoice::None));
        let forced = ToolChoice::function("my_tool");
        assert!(
            matches!(forced, ToolChoice::Function { function: ToolChoiceFunction { name } } if name == "my_tool")
        );
    }

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
