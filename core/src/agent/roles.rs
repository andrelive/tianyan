//! 角色化子 Agent 委托：内置角色（researcher / editor / reviewer）+ 用户配置覆盖。
//!
//! `delegate_to_agent` 工具的 `role` 参数按名字从 [`RoleRegistry`] 解析角色；
//! 角色提供模型、系统提示、工具白名单、最大轮数与整体超时，替代逐次手写参数。
//! 用户可在 `tianyan.toml` 的 `[agent_roles]` 节覆盖内置角色或新增自定义角色。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::AgentRolesConfig;

/// 子 Agent 角色定义。
///
/// 所有字段均为可选：缺省时回落到主 Agent 配置（模型/轮数/超时）或不生效
/// （系统提示 / 工具白名单）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRole {
    /// 角色名（配置节 `[agent_roles.<name>]` 的键，序列化时省略——名字即键）。
    #[serde(skip)]
    pub name: String,
    /// 角色使用的模型名称（与主 Agent 模型同空间；缺省回落主 Agent 模型）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 角色系统提示（缺省无系统提示，仅携带任务描述）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 工具白名单（缺省不限制——与主 Agent 相同；Some 时白名单外工具被拒绝）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// 子 Agent 最大循环轮数（缺省 200，范围 1-500）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// 委托整体超时（秒；缺省不限制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 角色注册表：按名字索引角色定义，供委托循环解析。
#[derive(Debug, Clone, Default)]
pub struct RoleRegistry {
    roles: HashMap<String, AgentRole>,
}

impl RoleRegistry {
    /// 内置角色：researcher（检索/调研）、editor（代码编辑）、reviewer（验证/评审）。
    ///
    /// 内置角色仅携带系统提示与工具白名单；模型/轮数/超时缺省，
    /// 回落到主 Agent 配置（`ToolRegistry.model` / 委托循环默认值）。
    pub fn builtin() -> Self {
        let mut roles = HashMap::new();
        roles.insert(
            "researcher".to_string(),
            AgentRole {
                name: "researcher".to_string(),
                model: None,
                system_prompt: Some(
                    "你是天演的检索调研助手。你的职责是通过网络搜索、知识库检索与代码\
                     搜索收集信息，并输出条理清晰、带来源引用的调研结论。\
                     你只负责调研与信息整理，不修改任何文件；\
                     需要改动代码或验证结果时，明确告知主任务由相应角色处理。"
                        .to_string(),
                ),
                tools: Some(vec![
                    "web_search".to_string(),
                    "web_fetch".to_string(),
                    "search_knowledge".to_string(),
                    "vfs_read".to_string(),
                    "vfs_list".to_string(),
                    "search_code".to_string(),
                    "read_file".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
            },
        );
        roles.insert(
            "editor".to_string(),
            AgentRole {
                name: "editor".to_string(),
                model: None,
                system_prompt: Some(
                    "你是天演的代码编辑助手。你的职责是阅读代码、定位问题并实施修改：\
                     用语义化编辑（apply_edit / apply_patch）原子地修改文件，\
                     修改后运行相关测试与构建验证。你专注于代码编辑闭环，\
                     不进行大规模调研；遇到需要外部信息或全局决策的问题时，\
                     汇报主任务处理。"
                        .to_string(),
                ),
                tools: Some(vec![
                    "read_file".to_string(),
                    "write_file".to_string(),
                    "apply_edit".to_string(),
                    "apply_patch".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "search_code".to_string(),
                    "run_tests".to_string(),
                    "discover_tests".to_string(),
                    "verify_build".to_string(),
                    "symbol_outline".to_string(),
                    "lsp".to_string(),
                    "execute_command".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
            },
        );
        roles.insert(
            "reviewer".to_string(),
            AgentRole {
                name: "reviewer".to_string(),
                model: None,
                system_prompt: Some(
                    "你是天演的验证评审助手。你的职责是审查代码变更与设计：\
                     阅读实现、检查符号与诊断、运行测试与构建验证，\
                     输出逐条评审意见（问题位置、严重程度、修改建议）。\
                     你只评审与验证，不直接修改文件；\
                     需要修改变更时上报主任务。"
                        .to_string(),
                ),
                tools: Some(vec![
                    "read_file".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "search_code".to_string(),
                    "symbol_outline".to_string(),
                    "lsp".to_string(),
                    "run_tests".to_string(),
                    "discover_tests".to_string(),
                    "verify_build".to_string(),
                    "self_check".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
            },
        );
        Self { roles }
    }

    /// 从用户配置构建注册表：先取内置角色，再叠加用户角色——
    /// 同名角色**整体覆盖**内置定义，新名字直接插入。
    pub fn from_config(config: &AgentRolesConfig) -> Self {
        let mut registry = Self::builtin();
        for (name, mut role) in config.roles.clone() {
            role.name = name.clone();
            registry.roles.insert(name, role);
        }
        registry
    }

    /// 按名字获取角色定义（返回克隆）。
    pub fn get(&self, name: &str) -> Option<AgentRole> {
        self.roles.get(name).cloned()
    }

    /// 所有角色名（字典序，便于生成稳定的可用角色列表）。
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.roles.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_roles(roles: HashMap<String, AgentRole>) -> AgentRolesConfig {
        AgentRolesConfig { roles }
    }

    #[test]
    fn test_builtin_has_exactly_three_roles() {
        let registry = RoleRegistry::builtin();
        assert_eq!(
            registry.names(),
            vec![
                "editor".to_string(),
                "researcher".to_string(),
                "reviewer".to_string()
            ]
        );
        for name in ["researcher", "editor", "reviewer"] {
            let role = registry.get(name).expect("内置角色应存在");
            assert_eq!(role.name, name);
            assert!(role.model.is_none(), "内置角色应回落主 Agent 模型");
            assert!(role.max_turns.is_none(), "内置角色应回落默认轮数");
            assert!(role.timeout_secs.is_none(), "内置角色应回落默认超时");
            let tools = role.tools.expect("内置角色应携带工具白名单");
            assert!(
                tools.iter().any(|t| t == "delegate_to_agent"),
                "角色应允许嵌套委托: {tools:?}"
            );
            assert!(
                role.system_prompt.as_deref().unwrap_or("").len() >= 20,
                "内置角色应携带系统提示"
            );
        }
        // 白名单差异：researcher 不含写文件工具，reviewer 不含修改类工具
        let researcher = registry.get("researcher").unwrap();
        assert!(!researcher.tools.unwrap().iter().any(|t| t == "write_file"));
        let reviewer = registry.get("reviewer").unwrap();
        assert!(!reviewer.tools.unwrap().iter().any(|t| t == "write_file"));
        let editor = registry.get("editor").unwrap();
        assert!(editor.tools.unwrap().iter().any(|t| t == "write_file"));
    }

    #[test]
    fn test_from_config_overrides_same_name() {
        // 同名角色整体覆盖内置定义（字段不继承）
        let mut roles = HashMap::new();
        roles.insert(
            "researcher".to_string(),
            AgentRole {
                name: String::new(),
                model: Some("deepseek-r1".to_string()),
                system_prompt: None,
                tools: None,
                max_turns: Some(10),
                timeout_secs: Some(60),
            },
        );
        let registry = RoleRegistry::from_config(&config_with_roles(roles));

        let role = registry.get("researcher").expect("覆盖后角色仍存在");
        assert_eq!(role.name, "researcher", "名字应以配置节键为准");
        assert_eq!(role.model.as_deref(), Some("deepseek-r1"));
        assert_eq!(role.max_turns, Some(10));
        assert_eq!(role.timeout_secs, Some(60));
        assert!(
            role.system_prompt.is_none() && role.tools.is_none(),
            "整体覆盖：未配置字段不继承内置值"
        );
        // 其余内置角色不受影响
        assert!(registry.get("editor").is_some());
        assert!(registry.get("reviewer").is_some());
    }

    #[test]
    fn test_from_config_inserts_new_name() {
        let mut roles = HashMap::new();
        roles.insert(
            "data-analyst".to_string(),
            AgentRole {
                name: String::new(),
                model: Some("gpt-4o".to_string()),
                system_prompt: Some("你是数据分析助手。".to_string()),
                tools: Some(vec!["read_file".to_string()]),
                max_turns: None,
                timeout_secs: None,
            },
        );
        let registry = RoleRegistry::from_config(&config_with_roles(roles));

        let added = registry.get("data-analyst").expect("新角色应被插入");
        assert_eq!(added.name, "data-analyst");
        assert_eq!(added.model.as_deref(), Some("gpt-4o"));
        assert_eq!(added.tools, Some(vec!["read_file".to_string()]));
        // 内置角色保留
        assert_eq!(registry.names().len(), 4);
        assert!(registry.get("researcher").is_some());
    }

    #[test]
    fn test_get_unknown_returns_none() {
        let registry = RoleRegistry::builtin();
        assert!(registry.get("ghost").is_none());
    }

    #[test]
    fn test_names_sorted_and_stable() {
        let registry = RoleRegistry::builtin();
        let names = registry.names();
        assert_eq!(names, registry.names(), "两次调用应一致");
        assert!(names.windows(2).all(|w| w[0] < w[1]), "应字典序排列");
    }

    #[test]
    fn test_role_config_toml_roundtrip() {
        // [agent_roles] 配置节与 AgentRole 的 TOML 往返（名字即键，序列化省略）
        let mut roles = HashMap::new();
        roles.insert(
            "researcher".to_string(),
            AgentRole {
                name: String::new(),
                model: Some("role-model".to_string()),
                system_prompt: Some("提示".to_string()),
                tools: Some(vec!["read_file".to_string()]),
                max_turns: Some(20),
                timeout_secs: Some(120),
            },
        );
        let config = config_with_roles(roles);
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AgentRolesConfig = toml::from_str(&toml_str).unwrap();
        let role = parsed.roles.get("researcher").expect("往返后角色应存在");
        assert_eq!(role.model.as_deref(), Some("role-model"));
        assert_eq!(role.max_turns, Some(20));
        assert_eq!(role.timeout_secs, Some(120));
    }
}
