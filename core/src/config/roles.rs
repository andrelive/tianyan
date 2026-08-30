//! 子 Agent 角色配置（`[agent_roles]` 配置节）。
//!
//! 角色按名字定义：内置角色（researcher / editor / reviewer）可被同名覆盖，
//! 新名字直接新增角色。配置内容复用 [`crate::agent::AgentRole`]（名字即
//! 配置节键，序列化时省略 `name` 字段）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::roles::AgentRole;

/// 子 Agent 角色配置节。
///
/// TOML 形式为 `[agent_roles.<name>]` 嵌套表，例如：
///
/// ```toml
/// [agent_roles.researcher]
/// model = "deepseek-r1"
/// max_turns = 30
/// ```
///
/// `roles` 字段以 `#[serde(flatten)]` 展开：`[agent_roles.researcher]` 的
/// 键直接映射为角色名（在 `TianyanConfig` 中表现为 `[agent_roles.<name>]`，
/// 单独解析 `AgentRolesConfig` 时表现为顶层 `[<name>]`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRolesConfig {
    /// 角色定义：键为角色名，值为角色配置（同名覆盖内置角色）。
    #[serde(default, flatten)]
    pub roles: HashMap<String, AgentRole>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_roles_section_roundtrip() {
        // 嵌套表 TOML 往返：flatten 后角色键直接映射
        // （单独解析 AgentRolesConfig 时键为顶层 `[<name>]`；
        // 在 TianyanConfig 中由 [agent_roles.<name>] 映射）
        let toml_str = r#"
            [researcher]
            model = "deepseek-r1"
            max_turns = 30
            timeout_secs = 120
            tools = ["web_search", "read_file"]
        "#;
        let config: AgentRolesConfig = toml::from_str(toml_str).unwrap();
        let role = config.roles.get("researcher").expect("应解析出角色");
        assert_eq!(role.model.as_deref(), Some("deepseek-r1"));
        assert_eq!(role.max_turns, Some(30));
        assert_eq!(role.timeout_secs, Some(120));
        assert_eq!(
            role.tools,
            Some(vec!["web_search".to_string(), "read_file".to_string()])
        );
        assert!(role.system_prompt.is_none());

        // 序列化后重新解析仍一致（name 省略，不产生多余字段）
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed: AgentRolesConfig = toml::from_str(&serialized).unwrap();
        let role2 = reparsed.roles.get("researcher").unwrap();
        assert_eq!(role2.model, role.model);
        assert_eq!(role2.max_turns, role.max_turns);
        assert_eq!(role2.tools, role.tools);
    }

    #[test]
    fn test_agent_roles_default_empty() {
        let config = AgentRolesConfig::default();
        assert!(config.roles.is_empty());
        let parsed: AgentRolesConfig = toml::from_str("").unwrap();
        assert!(parsed.roles.is_empty());
    }
}
