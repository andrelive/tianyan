//! 角色向量路由（ADR-016 P3）：任务描述 → 角色摘要的语义匹配建议。
//!
//! 主 agent 决定委托时基于 L0 摘要列表（渐进披露）选择角色；本模块提供
//! 语义匹配建议作为决策辅助：`suggest` 对任务描述做 embedding，与各角色
//! 摘要向量计算余弦相似度（`Embedding::cosine_similarity`）排序返回。
//!
//! 角色注册表规模小（≤ 数十），摘要向量**缓存化**：角色集合变化时重建，
//! 查询只需 1 次任务 embedding + 内存排序。

use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::roles::RoleStatus;
use crate::common::error::Result;
use crate::common::types::Embedding;
use crate::model::EmbeddingService;
use crate::role_store::RoleStore;

/// 角色匹配建议。
#[derive(Debug, Clone)]
pub struct RoleMatch {
    /// 角色名。
    pub name: String,
    /// 余弦相似度（0-1）。
    pub score: f32,
    /// 职责一句话（L0 摘要）。
    pub purpose: String,
    /// 激活状态（试验性仅供评估——主 agent 不可调用）。
    pub status: RoleStatus,
}

/// 摘要向量缓存条目。
type CacheEntry = (Embedding, String, RoleStatus);

/// 角色向量路由器。
#[derive(Clone)]
pub struct RoleRouter {
    store: RoleStore,
    embedding: Arc<dyn EmbeddingService>,
    embed_model: String,
    /// 摘要向量缓存（None = 未构建；角色集合变化时失效重建）。
    cache: Arc<tokio::sync::RwLock<Option<HashMap<String, CacheEntry>>>>,
}

impl RoleRouter {
    /// 创建路由器。`embed_model` 为嵌入模型名（配置解析）。
    pub fn new(
        store: RoleStore,
        embedding: Arc<dyn EmbeddingService>,
        embed_model: String,
    ) -> Self {
        Self {
            store,
            embedding,
            embed_model,
            cache: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// 建议匹配角色（按相似度降序，截断 top_k；含试验性——仅供评估）。
    pub async fn suggest(&self, task: &str, top_k: usize) -> Result<Vec<RoleMatch>> {
        let cache = self.ensure_cache().await?;
        if cache.is_empty() {
            return Ok(Vec::new());
        }
        let task_emb = self.embedding.embed_single(&self.embed_model, task).await?;
        let mut scored: Vec<RoleMatch> = cache
            .iter()
            .map(|(name, (emb, text, status))| RoleMatch {
                name: name.clone(),
                score: emb.cosine_similarity(&task_emb),
                purpose: text.clone(),
                status: *status,
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k.max(1));
        Ok(scored)
    }

    /// 摘要向量缓存：角色集合与缓存不一致时重建（embedding 全部角色摘要）。
    async fn ensure_cache(&self) -> Result<HashMap<String, CacheEntry>> {
        {
            let cache = self.cache.read().await;
            if let Some(c) = cache.as_ref() {
                // 角色集合变化检测：VFS 一级目录名（list 成本低）
                let current: Vec<String> = self.store_role_names().await.unwrap_or_default();
                let mut cached: Vec<String> = c.keys().cloned().collect();
                cached.sort();
                if current == cached {
                    return Ok(c.clone());
                }
            }
        }
        // 重建（锁外 embedding，锁内写）
        let roles = self.store.load_roles().await?;
        let mut built: HashMap<String, CacheEntry> = HashMap::new();
        for role in roles {
            let purpose = role
                .system_prompt
                .as_deref()
                .map(|p| p.lines().next().unwrap_or("").trim().to_string())
                .unwrap_or_else(|| "无系统提示".to_string());
            let tools = role.tools.as_deref().unwrap_or_default().join("、");
            let text = format!("{}。工具：{}", purpose, tools);
            if let Ok(emb) = self.embedding.embed_single(&self.embed_model, &text).await {
                built.insert(role.name.clone(), (emb, purpose, role.status));
            }
        }
        *self.cache.write().await = Some(built.clone());
        Ok(built)
    }

    /// VFS 角色名集合（缓存一致性检测）。
    async fn store_role_names(&self) -> Result<Vec<String>> {
        let roles = self.store.load_roles().await?;
        let mut names: Vec<String> = roles.into_iter().map(|r| r.name).collect();
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StorageConfig;
    use crate::test_utils::MockEmbeddingService;
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::{
        MockVectorStorage, VectorStorage, VfsCore, VirtualFileSystem, VirtualFileSystemImpl,
    };

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

    #[tokio::test]
    async fn test_suggest_returns_top_matches() {
        // mock embedding 恒等向量：流程验证（全部同分，排序稳定）
        let (vfs, _dir) = real_vfs().await;
        let store = RoleStore::new(vfs);
        let role = crate::agent::AgentRole {
            name: "data-analyst".to_string(),
            system_prompt: Some("你是数据分析助手。".to_string()),
            tools: Some(vec!["read_file".to_string()]),
            version: 1,
            ..Default::default()
        };
        store.save_role(&role).await.unwrap();

        let router = RoleRouter::new(
            store,
            Arc::new(MockEmbeddingService),
            "test-embed".to_string(),
        );
        let matches = router.suggest("帮我分析这份数据", 3).await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "data-analyst");
        assert!(matches[0].score > 0.0);
        assert!(matches[0].purpose.contains("数据分析"));
    }

    #[tokio::test]
    async fn test_suggest_empty_store_returns_empty() {
        let (vfs, _dir) = real_vfs().await;
        let router = RoleRouter::new(
            RoleStore::new(vfs),
            Arc::new(MockEmbeddingService),
            "test-embed".to_string(),
        );
        let matches = router.suggest("任意任务", 3).await.unwrap();
        assert!(matches.is_empty());
    }
}
