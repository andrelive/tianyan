//! 角色 VFS 存储（ADR-016：注册表持久化）。
//!
//! 角色存储于 `tianyan://agent/roles/<name>/`（复用 Agent 命名空间，
//! 路径前缀 `roles/` 区分角色与 Agent 自身内容，零命名空间改动）：
//! - L0 Abstract = 角色摘要（职责一句话 + 工具数 + 状态），delegate 描述渐进披露用；
//! - L2 Detail = 角色完整定义（JSON）。
//! - `_meta` 条目存用户配置签名（config_sig）：配置未变更时启动加载
//!   不覆盖演化产物（用户配置降级为种子来源，ADR-016 决策 2）。

use std::sync::Arc;

use crate::agent::roles::{AgentRole, RoleStatus};
use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, ContextNamespace, Message, TianyanUri};
use crate::vfs::VirtualFileSystem;

/// 角色会话快照（ADR-016 决策 5：durable 角色会话）。
#[derive(Debug, Clone, Default)]
pub struct RoleSession {
    /// 消息历史（JSONL 持久化；第一条为角色系统提示）。
    pub messages: Vec<Message>,
    /// 累计任务数（主 agent 决策依据）。
    pub task_count: u32,
    /// 会话创建时间（epoch 毫秒）。
    pub created_at: i64,
    /// 最后更新时间（epoch 毫秒）。
    pub updated_at: i64,
}

/// 角色会话元信息（L0 存储）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct RoleSessionMeta {
    task_count: u32,
    created_at: i64,
    updated_at: i64,
}

/// 角色使用统计（ADR-016 P3：退役信号/演化门控输入）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RoleUsage {
    /// 累计调用次数。
    pub calls: u32,
    /// 成功次数。
    pub success: u32,
    /// 失败次数。
    pub failed: u32,
    /// 最后使用时间（epoch 毫秒）。
    pub last_used: i64,
}

impl RoleUsage {
    /// 记录一次调用结果（success 决定成功/失败计数）。
    pub fn record(&mut self, success: bool) {
        self.calls += 1;
        if success {
            self.success += 1;
        } else {
            self.failed += 1;
        }
        self.last_used = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
    }

    /// 成功率（无调用时 0）。
    pub fn success_rate(&self) -> f32 {
        if self.calls == 0 {
            0.0
        } else {
            self.success as f32 / self.calls as f32
        }
    }
}

/// 单次委托记录（ADR-016：统计面板明细）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationRecord {
    /// 委托时间（epoch 毫秒）。
    pub ts: i64,
    /// 角色名。
    pub role: String,
    /// 任务描述（截断）。
    pub task: String,
    /// 是否成功。
    pub success: bool,
    /// 会话模式（continue/new/discard）。
    pub mode: String,
    /// 委托耗时（毫秒）。
    pub duration_ms: u64,
    /// 子 Agent token 消耗。
    pub tokens: usize,
}

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

    /// 角色会话 URI（`tianyan://agent/role_sessions/<name>`）。
    fn role_session_uri(name: &str) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Agent,
            vec!["role_sessions".to_string(), name.to_string()],
        )
    }

    /// 加载角色会话（ADR-016 决策 5：durable 角色会话）。
    ///
    /// 消息存 L2（JSONL），元信息（任务数/时间戳）存 L0。None = 无会话。
    pub async fn load_role_session(&self, name: &str) -> Result<Option<RoleSession>> {
        let uri = Self::role_session_uri(name);
        if !self.vfs.exists(&uri).await? {
            return Ok(None);
        }
        let jsonl = self
            .vfs
            .read_content(&uri, ContentLevel::Detail)
            .await
            .map_err(|e| TianyanError::Custom(format!("role_store: 角色会话读取失败：{e}")))?;
        let messages: Vec<Message> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Message>(l.trim()).ok())
            .collect();
        if messages.is_empty() {
            return Ok(None);
        }
        // 元信息（L0）解析失败时使用默认值（会话仍可用）
        let meta = self
            .vfs
            .read_abstract(&uri)
            .await
            .ok()
            .and_then(|m| serde_json::from_str::<RoleSessionMeta>(&m).ok())
            .unwrap_or_default();
        Ok(Some(RoleSession {
            messages,
            task_count: meta.task_count,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
        }))
    }

    /// 保存角色会话（覆盖写；消息截断由调用方负责）。
    pub async fn save_role_session(
        &self,
        name: &str,
        messages: &[Message],
        task_count: u32,
    ) -> Result<()> {
        let uri = Self::role_session_uri(name);
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // 首次创建时间保留（写入前读取旧会话——写后读取会拿到尚未写入的新 meta）
        let prev_created = self
            .load_role_session(name)
            .await
            .ok()
            .flatten()
            .map(|s| s.created_at)
            .unwrap_or(now);
        let jsonl = messages
            .iter()
            .filter_map(|m| serde_json::to_string(m).ok())
            .collect::<Vec<_>>()
            .join("\n");
        self.vfs.write_content(&uri, &jsonl).await?;
        let meta = RoleSessionMeta {
            task_count,
            created_at: prev_created,
            updated_at: now,
        };
        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| TianyanError::Custom(format!("role_store: 会话元信息序列化失败：{e}")))?;
        self.vfs.write_abstract(&uri, &meta_json).await
    }

    /// 废弃角色会话（discard 语义：删除消息与元信息）。
    pub async fn clear_role_session(&self, name: &str) -> Result<()> {
        let uri = Self::role_session_uri(name);
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

    #[tokio::test]
    async fn test_role_session_roundtrip_and_clear() {
        // durable 角色会话：消息 JSONL + 元信息（task_count/时间戳）往返
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs);
        let msgs = vec![
            Message::system("你是检索助手。"),
            Message::user("调研 A"),
            Message::assistant("完成 A"),
        ];
        store
            .save_role_session("researcher", &msgs, 1)
            .await
            .unwrap();

        let session = store
            .load_role_session("researcher")
            .await
            .unwrap()
            .expect("会话应存在");
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[1].content, "调研 A");
        assert_eq!(session.task_count, 1);
        assert!(session.created_at > 0);
        assert_eq!(session.updated_at, session.created_at);

        // 续写：追加任务，task_count 递增
        let mut msgs2 = session.messages.clone();
        msgs2.push(Message::user("调研 B"));
        msgs2.push(Message::assistant("完成 B"));
        store
            .save_role_session("researcher", &msgs2, 2)
            .await
            .unwrap();
        let session2 = store
            .load_role_session("researcher")
            .await
            .unwrap()
            .expect("会话应存在");
        assert_eq!(session2.messages.len(), 5);
        assert_eq!(session2.task_count, 2);
        assert_eq!(
            session2.created_at, session.created_at,
            "created_at 应保留首次创建时间"
        );

        // discard 语义：clear 后会话消失
        store.clear_role_session("researcher").await.unwrap();
        assert!(store
            .load_role_session("researcher")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_role_session_missing_returns_none() {
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs);
        assert!(store.load_role_session("ghost").await.unwrap().is_none());
    }
}
