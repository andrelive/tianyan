use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A tool definition passed to the model in a chat completion request.
///
/// Describes a callable function including its name, description, and
/// JSON Schema parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The type of tool. Currently only `Function` is supported.
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    /// Function definition for this tool.
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// Create a tool definition from a function definition.
    pub fn function(function: FunctionDefinition) -> Self {
        Self {
            tool_type: ToolType::Function,
            function,
        }
    }
}

/// Supported tool types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Function,
}

/// Definition of a callable function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// The name of the function to be called.
    pub name: String,
    /// A description of what the function does, used by the model to choose when and how to call it.
    pub description: String,
    /// The parameters the function accepts, described as a JSON Schema object.
    pub parameters: serde_json::Value,
}

impl FunctionDefinition {
    /// Create a function definition with explicit JSON Schema parameters.
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

    /// Create a function definition deriving JSON Schema from a Rust type.
    ///
    /// The type must implement [`schemars::JsonSchema`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// #[derive(schemars::JsonSchema)]
    /// struct SearchParams {
    ///     query: String,
    ///     scope: Option<String>,
    /// }
    ///
    /// let def = FunctionDefinition::from_schema::<SearchParams>(
    ///     "vfs_search",
    ///     "Search files in the virtual file system"
    /// );
    /// ```
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

/// A tool call produced by the model in an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// The ID of the tool call.
    pub id: String,
    /// The type of tool call. Currently only `function`.
    #[serde(rename = "type")]
    pub call_type: ToolCallType,
    /// The function call details.
    pub function: FunctionCall,
}

/// Type of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallType {
    Function,
}

/// A function call issued by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// The name of the function to call.
    pub name: String,
    /// The arguments to pass to the function, as a JSON string.
    ///
    /// Note: the model may generate invalid JSON; callers must validate before parsing.
    pub arguments: String,
}

/// Controls which (if any) tool the model should call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// The model can pick between generating a message or calling one or more tools.
    Auto,
    /// The model will not call any tool and instead generates a message.
    None,
    /// The model is forced to call a specific tool.
    Function { function: ToolChoiceFunction },
}

/// Specifies a tool by name for [`ToolChoice::Function`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoiceFunction {
    pub name: String,
}

impl ToolChoice {
    /// Create a `ToolChoice::Auto` variant.
    pub fn auto() -> Self {
        Self::Auto
    }

    /// Create a `ToolChoice::None` variant.
    pub fn none() -> Self {
        Self::None
    }

    /// Create a `ToolChoice::Function` variant forcing a named tool.
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
