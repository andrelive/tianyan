//! Agent 工具定义，供 ReAct 循环调用。
//!
//! 统一 VFS 工具和技能调用。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent 可调用的工具类型。
/// 包含 VFS 完整 CRUD 操作和技能调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum AgentTool {
    VfsSearch {
        query: String,
        scope: Option<String>,
        top_k: Option<usize>,
    },
    VfsRead {
        uri: String,
    },
    VfsList {
        uri: String,
    },
    VfsWrite {
        uri: String,
        content: String,
    },
    VfsDelete {
        uri: String,
    },
    VfsMkdir {
        uri: String,
    },
    Skill {
        skill_id: String,
        #[serde(default)]
        params: HashMap<String, serde_json::Value>,
    },
}

/// 工具执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub content: String,
    pub related_uris: Vec<String>,
}

impl AgentTool {
    pub fn parse(output: &str) -> Option<Self> {
        if let Ok(tool) = serde_json::from_str(output) {
            return Some(tool);
        }

        if let Some(action_line) = output.lines().find(|l| l.starts_with("Action:")) {
            let action = action_line.strip_prefix("Action:")?.trim();
            return Self::parse_text_action(action);
        }

        None
    }

    fn parse_text_action(action: &str) -> Option<Self> {
        if action.starts_with("vfs_search") {
            let (query, scope, top_k) = Self::parse_search_params(action)?;
            Some(AgentTool::VfsSearch {
                query,
                scope,
                top_k,
            })
        } else if action.starts_with("vfs_read") {
            let uri = Self::parse_single_string_param(action)?;
            Some(AgentTool::VfsRead { uri })
        } else if action.starts_with("vfs_list") {
            let uri = Self::parse_single_string_param(action)?;
            Some(AgentTool::VfsList { uri })
        } else if action.starts_with("vfs_write") {
            let (uri, content) = Self::parse_write_params(action)?;
            Some(AgentTool::VfsWrite { uri, content })
        } else if action.starts_with("vfs_delete") {
            let uri = Self::parse_single_string_param(action)?;
            Some(AgentTool::VfsDelete { uri })
        } else if action.starts_with("vfs_mkdir") {
            let uri = Self::parse_single_string_param(action)?;
            Some(AgentTool::VfsMkdir { uri })
        } else if action.starts_with("skill:") {
            let skill_id = action
                .strip_prefix("skill:")?
                .split('(')
                .next()?
                .to_string();
            let params = Self::parse_skill_params(action).unwrap_or_default();
            Some(AgentTool::Skill { skill_id, params })
        } else {
            None
        }
    }

    fn parse_search_params(action: &str) -> Option<(String, Option<String>, Option<usize>)> {
        let start = action.find('(')?;
        let end = action.rfind(')')?;
        let params_str = &action[start + 1..end];

        let mut query = None;
        let mut scope = None;
        let mut top_k = None;

        for part in split_params(params_str) {
            let part = part.trim();
            if part.starts_with('"') || part.starts_with("'") {
                if query.is_none() {
                    query = Some(unquote_string(part)?);
                }
            } else if part.starts_with("scope=") {
                scope = Some(unquote_string(part.strip_prefix("scope=")?.trim())?);
            } else if part.starts_with("top_k=") {
                top_k = part.strip_prefix("top_k=")?.parse().ok();
            }
        }

        Some((query?, scope, top_k))
    }

    fn parse_single_string_param(action: &str) -> Option<String> {
        let start = action.find('(')?;
        let end = action.rfind(')')?;
        let param = &action[start + 1..end].trim();
        unquote_string(param)
    }

    fn parse_write_params(action: &str) -> Option<(String, String)> {
        let start = action.find('(')?;
        let end = action.rfind(')')?;
        let params_str = &action[start + 1..end];

        let parts: Vec<&str> = split_params(params_str).collect();
        if parts.len() < 2 {
            return None;
        }

        let uri = unquote_string(parts[0].trim())?;
        let content = unquote_string(parts[1].trim())?;

        Some((uri, content))
    }

    fn parse_skill_params(action: &str) -> Option<HashMap<String, serde_json::Value>> {
        let start = action.find('(')?;
        let end = action.rfind(')')?;
        let params_str = &action[start + 1..end];

        if params_str.trim().is_empty() {
            return Some(HashMap::new());
        }

        let mut params = HashMap::new();
        for part in split_params(params_str) {
            let part = part.trim();
            if let Some(eq_pos) = part.find('=') {
                let key = part[..eq_pos].trim().to_string();
                let value = part[eq_pos + 1..].trim();
                let json_value = parse_json_value(value);
                params.insert(key, json_value);
            }
        }

        Some(params)
    }

    pub fn to_description(&self) -> String {
        match self {
            AgentTool::VfsSearch {
                query,
                scope,
                top_k,
            } => {
                let mut parts = format!("vfs_search(\"{}\"", query);
                if let Some(s) = scope {
                    parts.push_str(&format!(", scope=\"{}\"", s));
                }
                if let Some(k) = top_k {
                    parts.push_str(&format!(", top_k={}", k));
                }
                parts.push(')');
                parts
            }
            AgentTool::VfsRead { uri } => format!("vfs_read(\"{}\")", uri),
            AgentTool::VfsList { uri } => format!("vfs_list(\"{}\")", uri),
            AgentTool::VfsWrite { uri, content } => {
                let truncated = if content.len() > 50 {
                    format!("{}...", &content[..50])
                } else {
                    content.clone()
                };
                format!("vfs_write(\"{}\", \"{}\")", uri, truncated)
            }
            AgentTool::VfsDelete { uri } => format!("vfs_delete(\"{}\")", uri),
            AgentTool::VfsMkdir { uri } => format!("vfs_mkdir(\"{}\")", uri),
            AgentTool::Skill { skill_id, params } => {
                let params_str = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ");
                if params_str.is_empty() {
                    format!("skill:{}", skill_id)
                } else {
                    format!("skill:{}({})", skill_id, params_str)
                }
            }
        }
    }
}

fn split_params(s: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0;
    let mut start = 0;
    let mut in_string = false;
    let mut escape = false;

    s.char_indices()
        .filter_map(move |(i, c)| {
            if escape {
                escape = false;
                return None;
            }

            match c {
                '\\' if in_string => {
                    escape = true;
                    None
                }
                '"' | '\'' if !escape => {
                    in_string = !in_string;
                    None
                }
                '(' | '[' | '{' if !in_string => {
                    depth += 1;
                    None
                }
                ')' | ']' | '}' if !in_string => {
                    depth -= 1;
                    None
                }
                ',' if depth == 0 && !in_string => {
                    let result = &s[start..i];
                    start = i + 1;
                    Some(result)
                }
                _ => None,
            }
        })
        .chain(std::iter::once(&s[start..]))
}

fn unquote_string(s: &str) -> Option<String> {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with("'") && s.ends_with("'")) {
        let inner = &s[1..s.len() - 1];
        Some(inner.replace("\\\"", "\"").replace("\\'", "'"))
    } else {
        Some(s.to_string())
    }
}

fn parse_json_value(s: &str) -> serde_json::Value {
    let s = s.trim();

    if s.starts_with('"') && s.ends_with('"') {
        serde_json::Value::String(s[1..s.len() - 1].to_string())
    } else if s == "true" {
        serde_json::Value::Bool(true)
    } else if s == "false" {
        serde_json::Value::Bool(false)
    } else if s == "null" {
        serde_json::Value::Null
    } else if let Ok(n) = s.parse::<i64>() {
        serde_json::Value::Number(n.into())
    } else if let Ok(n) = s.parse::<f64>() {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(s.to_string()))
    } else {
        serde_json::Value::String(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vfs_search_json() {
        let json =
            r#"{"tool": "vfs_search", "query": "test query", "scope": "memory", "top_k": 10}"#;
        let tool = AgentTool::parse(json).unwrap();

        match tool {
            AgentTool::VfsSearch {
                query,
                scope,
                top_k,
            } => {
                assert_eq!(query, "test query");
                assert_eq!(scope, Some("memory".to_string()));
                assert_eq!(top_k, Some(10));
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_vfs_search_text() {
        let text = r#"Action: vfs_search("Rust 项目")"#;
        let tool = AgentTool::parse(text).unwrap();

        match tool {
            AgentTool::VfsSearch {
                query,
                scope,
                top_k,
            } => {
                assert_eq!(query, "Rust 项目");
                assert_eq!(scope, None);
                assert_eq!(top_k, None);
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_vfs_read_json() {
        let json = r#"{"tool": "vfs_read", "uri": "tianyan://knowledge/doc"}"#;
        let tool = AgentTool::parse(json).unwrap();

        match tool {
            AgentTool::VfsRead { uri } => {
                assert_eq!(uri, "tianyan://knowledge/doc");
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_vfs_read_text() {
        let text = r#"Action: vfs_read("tianyan://memory/session")"#;
        let tool = AgentTool::parse(text).unwrap();

        match tool {
            AgentTool::VfsRead { uri } => {
                assert_eq!(uri, "tianyan://memory/session");
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_vfs_write_json() {
        let json =
            r#"{"tool": "vfs_write", "uri": "tianyan://knowledge/note", "content": "Hello"}"#;
        let tool = AgentTool::parse(json).unwrap();

        match tool {
            AgentTool::VfsWrite { uri, content } => {
                assert_eq!(uri, "tianyan://knowledge/note");
                assert_eq!(content, "Hello");
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_skill_json() {
        let json = r#"{"tool": "skill", "skill_id": "file_read", "params": {"path": "/test"}}"#;
        let tool = AgentTool::parse(json).unwrap();

        match tool {
            AgentTool::Skill { skill_id, params } => {
                assert_eq!(skill_id, "file_read");
                assert_eq!(params.get("path").unwrap().as_str().unwrap(), "/test");
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_parse_skill_text() {
        let input = r#"Action: skill:file_read(path="/test/file.txt")"#;
        let tool = AgentTool::parse(input).unwrap();

        match tool {
            AgentTool::Skill { skill_id, params } => {
                assert_eq!(skill_id, "file_read");
                assert_eq!(
                    params.get("path").unwrap().as_str().unwrap(),
                    "/test/file.txt"
                );
            }
            _ => panic!("Wrong tool type"),
        }
    }

    #[test]
    fn test_to_description() {
        let tool = AgentTool::VfsSearch {
            query: "test".to_string(),
            scope: Some("memory".to_string()),
            top_k: Some(5),
        };
        assert_eq!(
            tool.to_description(),
            r#"vfs_search("test", scope="memory", top_k=5)"#
        );

        let tool = AgentTool::VfsRead {
            uri: "tianyan://test".to_string(),
        };
        assert_eq!(tool.to_description(), r#"vfs_read("tianyan://test")"#);
    }

    #[test]
    fn test_tool_result() {
        let result = ToolResult {
            success: true,
            content: "Test content".to_string(),
            related_uris: vec!["tianyan://test".to_string()],
        };

        assert!(result.success);
        assert_eq!(result.content, "Test content");
        assert_eq!(result.related_uris.len(), 1);
    }
}
