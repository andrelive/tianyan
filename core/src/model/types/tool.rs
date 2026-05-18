use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use crate::common::types::tool::{FunctionCall, ToolCall, ToolCallType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    pub fn function(function: FunctionDefinition) -> Self {
        Self {
            tool_type: ToolType::Function,
            function,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl FunctionDefinition {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Function { function: ToolChoiceFunction },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoiceFunction {
    pub name: String,
}

impl ToolChoice {
    pub fn auto() -> Self {
        Self::Auto
    }

    pub fn none() -> Self {
        Self::None
    }

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
