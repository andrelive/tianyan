//! 角色注册表（ADR-016：统一角色实体模型）。
//!
//! 基础类型（[`crate::roles`]：AgentRole/RoleSource/RoleStatus）已移至基础层；
//! 本模块保留 agent 特定逻辑：注册表（按名字索引 + 内置种子）+ 配置签名。

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::Result;
use crate::config::AgentRolesConfig;
use crate::role_store::RoleStore;
pub use crate::roles::{AgentRole, RoleSource, RoleStatus};

/// 用户配置签名：排序后的角色序列化文本（配置变更检测）。
fn config_sig(config: &AgentRolesConfig) -> String {
    let mut entries: Vec<(String, String)> = config
        .roles
        .iter()
        .map(|(name, role)| {
            let v = serde_json::to_value(role).unwrap_or_default();
            (name.clone(), v.to_string())
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// 角色注册表：按名字索引角色定义，供委托循环解析。
///
/// 内部 `std::sync::RwLock`（短临界区、无异步持有）：注册表以 `Arc` 共享，
/// 运行期可增量刷新（ADR-016：会话边界从 VFS 合并学习产物，新会话立即可见）。
#[derive(Debug, Clone, Default)]
pub struct RoleRegistry {
    roles: Arc<std::sync::RwLock<HashMap<String, AgentRole>>>,
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
                    "search_vfs".to_string(),
                    "vfs_read".to_string(),
                    "vfs_list".to_string(),
                    "grep".to_string(),
                    "read_file".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
                source: RoleSource::Builtin,
                status: RoleStatus::Active,
                version: 1,
                lineage: None,
            },
        );
        roles.insert(
            "editor".to_string(),
            AgentRole {
                name: "editor".to_string(),
                model: None,
                system_prompt: Some(
                    "你是天演的代码编辑助手。你的职责是阅读代码、定位问题并实施修改：\
                     用 apply_patch（主力编辑，unified diff + 上下文锚定）原子地修改文件；仅对能精确复现原文的极小改动才用 apply_edit；修改后运行相关测试与构建验证。你专注于代码编辑闭环，\
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
                    "grep".to_string(),
                    "run_tests".to_string(),
                    "discover_tests".to_string(),
                    "verify_build".to_string(),
                    "symbol_outline".to_string(),
                    "lsp".to_string(),
                    "execute_command".to_string(),
                    "task_status".to_string(),
                    "task_cancel".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
                source: RoleSource::Builtin,
                status: RoleStatus::Active,
                version: 1,
                lineage: None,
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
                    "grep".to_string(),
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
                source: RoleSource::Builtin,
                status: RoleStatus::Active,
                version: 1,
                lineage: None,
            },
        );
        roles.insert(
            "evolution_reviewer".to_string(),
            AgentRole {
                name: "evolution_reviewer".to_string(),
                model: None,
                system_prompt: Some(
                    "你是天演的演化综述员。\n\
                     你的职责是完成一次自我演化综述：\n\
                     1. 用 execution_stats / execution_detail / delegation_stats 查看自上次运行以来的\n\
                     工具执行统计与委托统计，识别高频/低成功率的操作类别与组织模式；\n\
                     2. 用 session_recall 回忆近期会话内容，识别用户偏好、事实与重复的工作模式；\n\
                     3. 用 vfs_read / vfs_list 查看现有注册表：记忆（memory/）、技能（skill/learned/）、\
                     规则（agent/learned/）、角色（agent_role/）；\n\
                     4. 对照现状决定增删改：写新技能前先查现有技能（语义重复则完善而非新建）；\
                     只把稳定、跨会话可复用的偏好写入画像；删除必须有证据（被取代/已过时/低分）。\n\
                     你只产出结论与建议，不修改任何注册表内容——修改由演化任务统一应用。\n\
                     输出格式：结构化清单（新增/更新/删除/冲突），每条附理由。"
                        .to_string(),
                ),
                tools: Some(vec![
                    "execution_stats".to_string(),
                    "execution_detail".to_string(),
                    "delegation_stats".to_string(),
                    "session_recall".to_string(),
                    "vfs_read".to_string(),
                    "vfs_list".to_string(),
                    "search_vfs".to_string(),
                    "read_file".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "grep".to_string(),
                    "web_search".to_string(),
                    "web_fetch".to_string(),
                    "delegate_to_agent".to_string(),
                ]),
                max_turns: None,
                timeout_secs: None,
                source: RoleSource::Builtin,
                status: RoleStatus::Active,
                version: 1,
                lineage: None,
            },
        );
        Self {
            roles: Arc::new(std::sync::RwLock::new(roles)),
        }
    }

    /// 从用户配置构建注册表：先取内置角色，再叠加用户角色——
    /// 同名角色**整体覆盖**内置定义，新名字直接插入。
    ///
    /// 注：ADR-016 装配路径优先使用 [`Self::load_persisted`]（VFS 持久化）；
    /// 本方法保留为无 VFS 环境的轻量构造。
    pub fn from_config(config: &AgentRolesConfig) -> Self {
        let registry = Self::builtin();
        for (name, mut role) in config.roles.clone() {
            role.name = name.clone();
            registry.insert(role);
        }
        registry
    }

    /// ADR-016 装配：内置种子 + 用户配置（签名变更才 upsert）+ VFS 演化实体。
    ///
    /// 规则：
    /// - **VFS 角色为权威**：持久化的演化产物（含学习更新）直接加载；
    /// - **内置种子**：VFS 中缺失的内置角色首次写入（出厂种子落盘）；
    /// - **用户配置降级为种子**：配置签名（`[agent_roles]` 序列化）与上次不同时
    ///   覆盖对应角色（用户新意图生效）；签名相同不覆盖（演化产物保留）。
    pub async fn load_persisted(store: &RoleStore, config: &AgentRolesConfig) -> Result<Self> {
        let registry = Self::builtin();
        registry.apply_persisted(store, config).await?;
        Ok(registry)
    }

    /// 就地应用持久化状态（ADR-016 装配规则；与 [`Self::load_persisted`] 同规则，
    /// 供配置热重载在共享注册表上执行）。
    ///
    /// 规则：VFS 角色权威覆盖 → 内置种子首次落盘 → 用户配置按签名 upsert。
    pub async fn apply_persisted(
        &self,
        store: &RoleStore,
        config: &AgentRolesConfig,
    ) -> Result<()> {
        // 1. VFS 现有角色（权威）覆盖
        for role in store.load_roles().await? {
            self.insert(role);
        }
        // 2. 内置种子写入 VFS（首次启动）
        for name in self.names() {
            if store.load_role(&name).await?.is_none() {
                if let Some(role) = self.get(&name) {
                    store.save_role(&role).await?;
                }
            }
        }
        // 3. 用户配置：签名变更 → 覆盖（用户新意图生效）
        let sig = config_sig(config);
        let prev = store.load_config_sig().await?;
        if prev.as_deref() != Some(sig.as_str()) {
            for (name, mut role) in config.roles.clone() {
                role.name = name.clone();
                role.source = RoleSource::User;
                role.version = 1;
                self.insert(role);
                if let Some(saved) = self.get(&name) {
                    store.save_role(&saved).await?;
                }
            }
            store.save_config_sig(&sig).await?;
        }
        Ok(())
    }

    /// 注册或覆盖角色（含来源元数据；VFS 持久化由 RoleStore 负责）。
    pub fn insert(&self, role: AgentRole) {
        self.roles
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(role.name.clone(), role);
    }

    /// 从 VFS 增量合并角色（ADR-016：会话边界刷新——学习产物新会话立即可见）。
    ///
    /// 幂等：已存在的同名角色被覆盖（VFS 为权威），返回本次新增的角色数。
    pub async fn refresh_from_store(&self, store: &RoleStore) -> Result<usize> {
        let vfs_roles = store.load_roles().await?;
        let mut new_count = 0usize;
        for role in vfs_roles {
            if self.get(&role.name).is_none() {
                new_count += 1;
            }
            self.insert(role);
        }
        Ok(new_count)
    }

    /// 按名字获取角色定义（含试验性；委托解析用 [`Self::get_active`]）。
    pub fn get(&self, name: &str) -> Option<AgentRole> {
        self.roles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// 按名字获取**可调用**角色（ADR-016：过滤试验性——只展示不可调用）。
    pub fn get_active(&self, name: &str) -> Option<AgentRole> {
        self.roles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
            .filter(|r| r.status != RoleStatus::Experimental)
    }

    /// 可调用角色名（过滤试验性，字典序）。
    pub fn active_names(&self) -> Vec<String> {
        let roles = self.roles.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = roles
            .values()
            .filter(|r| r.status != RoleStatus::Experimental)
            .map(|r| r.name.clone())
            .collect();
        names.sort();
        names
    }

    /// 所有角色名（字典序，便于生成稳定的可用角色列表）。
    pub fn names(&self) -> Vec<String> {
        let roles = self.roles.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = roles.keys().cloned().collect();
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
    fn test_builtin_has_exactly_four_roles() {
        let registry = RoleRegistry::builtin();
        assert_eq!(
            registry.names(),
            vec![
                "editor".to_string(),
                "evolution_reviewer".to_string(),
                "researcher".to_string(),
                "reviewer".to_string()
            ]
        );
        for name in ["researcher", "editor", "reviewer", "evolution_reviewer"] {
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
                ..Default::default()
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
                source: RoleSource::Builtin,
                status: RoleStatus::Active,
                version: 1,
                lineage: None,
            },
        );
        let registry = RoleRegistry::from_config(&config_with_roles(roles));

        let added = registry.get("data-analyst").expect("新角色应被插入");
        assert_eq!(added.name, "data-analyst");
        assert_eq!(added.model.as_deref(), Some("gpt-4o"));
        assert_eq!(added.tools, Some(vec!["read_file".to_string()]));
        // 内置角色保留
        assert_eq!(registry.names().len(), 5);
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
                ..Default::default()
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
