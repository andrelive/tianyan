//! 角色 VFS 存储（ADR-016：注册表持久化）。
//!
//! **独立存储层**：依赖 vfs + [`crate::roles`] 纯类型，不依赖 agent——
//!
//! 角色存储于 `tianyan://agent/roles/<name>/`（复用 Agent 命名空间，
//! 路径前缀 `roles/` 区分角色与 Agent 自身内容，零命名空间改动）：
//! - L0 Abstract = 角色摘要（职责一句话 + 工具数 + 状态），delegate 描述渐进披露用；
//! - L2 Detail = 角色完整定义（JSON）。
//! - `_meta` 条目存用户配置签名（config_sig）：配置未变更时启动加载
//!   不覆盖演化产物（用户配置降级为种子来源，ADR-016 决策 2）。

use std::sync::Arc;

use crate::roles::{AgentRole, DelegationRecord, RoleStatus, RoleUsage};
use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::vfs::VirtualFileSystem;

/// 委托历史保留上限（超出重写截断）。
const DELEGATION_HISTORY_MAX: usize = 500;

/// 委托历史文件 URI（`tianyan://agent/roles/_history/delegations.jsonl`）。
fn delegation_history_uri() -> TianyanUri {
    TianyanUri::new(
        ContextNamespace::Agent,
        vec![
            "roles".to_string(),
            HISTORY_PREFIX.to_string(),
            "delegations.jsonl".to_string(),
        ],
    )
}

/// 角色存储的路径前缀（`tianyan://agent/roles/`）。
pub const ROLES_PREFIX: &str = "roles";

/// 配置签名元条目名（list 时过滤）。
const META_NAME: &str = "_meta";

/// 角色使用统计元目录名（list 时过滤）。
pub const USAGE_PREFIX: &str = "_usage";

/// 委托历史元目录名（list 时过滤）。
pub const HISTORY_PREFIX: &str = "_history";

/// 角色 VFS 存储：保存 / 加载 / 删除学习与配置角色，维护配置签名。
#[derive(Clone)]
pub struct RoleStore {
    vfs: Arc<dyn VirtualFileSystem>,
}

impl RoleStore {
    /// 创建存储。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs }
    }

    /// 角色 URI（`tianyan://agent/roles/<name>`）。
    fn role_uri(name: &str) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Agent,
            vec![ROLES_PREFIX.to_string(), name.to_string()],
        )
    }

    /// 角色存储根。
    fn root_uri() -> TianyanUri {
        TianyanUri::new(ContextNamespace::Agent, vec![ROLES_PREFIX.to_string()])
    }

    /// 保存角色：写 L0 摘要 + L2 定义（JSON）。
    pub async fn save_role(&self, role: &AgentRole) -> Result<()> {
        let uri = Self::role_uri(&role.name);
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        // L2：完整定义 JSON（name 补回——AgentRole 的 name 序列化省略）
        let mut value = serde_json::to_value(role)
            .map_err(|e| TianyanError::Custom(format!("role_store: 角色序列化失败：{e}")))?;
        value["name"] = serde_json::Value::String(role.name.clone());
        let json_text = serde_json::to_string_pretty(&value)
            .map_err(|e| TianyanError::Custom(format!("role_store: 角色序列化失败：{e}")))?;
        self.vfs.write_content(&uri, &json_text).await?;
        // L0：渐进披露摘要
        let purpose = role
            .system_prompt
            .as_deref()
            .map(|p| p.lines().next().unwrap_or("").trim())
            .unwrap_or("无系统提示");
        let tool_count = role.tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let mut abstract_content = format!("{} | 工具 {} 个", purpose, tool_count);
        if role.status == RoleStatus::Experimental {
            abstract_content.push_str(" | 试验性");
        }
        self.vfs.write_abstract(&uri, &abstract_content).await?;
        Ok(())
    }

    /// 加载全部角色（排除 `_meta` 元条目；解析失败跳过并告警）。
    pub async fn load_roles(&self) -> Result<Vec<AgentRole>> {
        let root = Self::root_uri();
        // 目录未创建（从未保存过角色）时视为空，不报错（ADR-014 语义谓词）
        let entries = match self.vfs.list(&root).await {
            Ok(entries) => entries,
            Err(e) if e.is_not_found() => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut roles = Vec::new();
        for entry in entries {
            // 不做目录标记过滤：SQLite 后端条目为单行（is_directory 与内容互斥），
            // 角色条目可能是内容行——按名加载即可，两种后端统一
            let Some(name) = entry.uri().path().last().cloned() else {
                continue;
            };
            if name == META_NAME || name == USAGE_PREFIX || name == HISTORY_PREFIX {
                continue;
            }
            match self.load_role(&name).await {
                Ok(Some(role)) => roles.push(role),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(role = %name, error = %e, "角色加载跳过（解析失败）");
                }
            }
        }
        Ok(roles)
    }

    /// 按名加载单个角色。
    pub async fn load_role(&self, name: &str) -> Result<Option<AgentRole>> {
        let uri = Self::role_uri(name);
        if !self.vfs.exists(&uri).await? {
            return Ok(None);
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        let mut role: AgentRole = serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("role_store: 角色解析失败：{e}")))?;
        role.name = name.to_string();
        Ok(Some(role))
    }

    /// 删除角色（含子条目）。
    pub async fn delete_role(&self, name: &str) -> Result<()> {
        let uri = Self::role_uri(name);
        if self.vfs.exists(&uri).await? {
            self.vfs.delete(&uri).await?;
        }
        Ok(())
    }

    /// 角色使用统计 URI（`tianyan://agent/roles/_usage/<name>`）。
    fn role_usage_uri(name: &str) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Agent,
            vec!["roles".to_string(), "_usage".to_string(), name.to_string()],
        )
    }

    /// 加载角色使用统计（无记录时返回默认空统计）。
    pub async fn load_role_usage(&self, name: &str) -> Result<RoleUsage> {
        let uri = Self::role_usage_uri(name);
        if !self.vfs.exists(&uri).await? {
            return Ok(RoleUsage::default());
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("role_store: 使用统计解析失败：{e}")))
    }

    /// 保存角色使用统计（覆盖写）。
    pub async fn save_role_usage(&self, name: &str, usage: &RoleUsage) -> Result<()> {
        let uri = Self::role_usage_uri(name);
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        let json = serde_json::to_string_pretty(usage)
            .map_err(|e| TianyanError::Custom(format!("role_store: 使用统计序列化失败：{e}")))?;
        self.vfs.write_content(&uri, &json).await
    }

    /// 追加一条委托记录（统计面板明细；超上限重写截断保留最近 N 条）。
    pub async fn append_delegation_record(&self, record: &DelegationRecord) -> Result<()> {
        let uri = delegation_history_uri();
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        let mut records = self.load_delegation_records().await?;
        records.push(record.clone());
        if records.len() > DELEGATION_HISTORY_MAX {
            let drop = records.len() - DELEGATION_HISTORY_MAX;
            records.drain(..drop);
        }
        let jsonl = records
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect::<Vec<_>>()
            .join("\n");
        self.vfs.write_content(&uri, &jsonl).await
    }

    /// 读取全部委托记录（无历史时返回空；解析失败行跳过）。
    pub async fn load_delegation_records(&self) -> Result<Vec<DelegationRecord>> {
        let uri = delegation_history_uri();
        if !self.vfs.exists(&uri).await? {
            return Ok(Vec::new());
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        Ok(content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<DelegationRecord>(l.trim()).ok())
            .collect())
    }

    /// 保存用户配置签名（`[agent_roles]` 序列化文本；配置变更检测用）。
    pub async fn save_config_sig(&self, sig: &str) -> Result<()> {
        let uri = TianyanUri::new(
            ContextNamespace::Agent,
            vec![ROLES_PREFIX.to_string(), META_NAME.to_string()],
        );
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        self.vfs.write_content(&uri, sig).await
    }

    /// 读取用户配置签名（None 表示从未保存——视为配置变更）。
    pub async fn load_config_sig(&self) -> Result<Option<String>> {
        let uri = TianyanUri::new(
            ContextNamespace::Agent,
            vec![ROLES_PREFIX.to_string(), META_NAME.to_string()],
        );
        if !self.vfs.exists(&uri).await? {
            return Ok(None);
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        Ok(Some(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StorageConfig;
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::{MockVectorStorage, VectorStorage, VfsCore, VirtualFileSystemImpl};

    /// 真实文件后端 + tempdir 的 VFS（write 自动建条目，可 list）。
    async fn real_vfs() -> (Arc<dyn VirtualFileSystem>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = StorageConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        };
        let storage = Arc::new(LocalFileBackend::new(config.clone()));
        let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
        let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
        vfs.initialize().await.unwrap();
        (Arc::new(vfs), dir)
    }

    fn sample_role(name: &str) -> AgentRole {
        AgentRole {
            name: name.to_string(),
            system_prompt: Some(format!("你是{name}助手。")),
            tools: Some(vec!["read_file".to_string()]),
            version: 1,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_role_store_roundtrip() {
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs);
        store.save_role(&sample_role("data-analyst")).await.unwrap();

        let roles = store.load_roles().await.unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].name, "data-analyst");
        assert_eq!(roles[0].source, crate::agent::roles::RoleSource::User);
        assert_eq!(roles[0].version, 1);
    }

    #[tokio::test]
    async fn test_role_store_delete_and_meta() {
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs);
        store.save_role(&sample_role("data-analyst")).await.unwrap();
        store.save_config_sig("sig-1").await.unwrap();

        assert_eq!(
            store.load_config_sig().await.unwrap().as_deref(),
            Some("sig-1")
        );
        // _meta 不出现在角色列表
        let roles = store.load_roles().await.unwrap();
        assert!(roles.iter().all(|r| r.name != "_meta"));

        store.delete_role("data-analyst").await.unwrap();
        let roles = store.load_roles().await.unwrap();
        assert!(roles.is_empty());
    }
}
