//! `storage` 组（ADR-049 期 4b）：**结构化存储 + 向量存储**的工厂。
//!
//! 两个插槽共用同一份 `StorageConfig`（`config.storage` / `config.vector`）、同一次
//! 启动链（`initialize_vfs_for_app`：先建后端再建向量库）、同一批「换后端 / 换向量库」
//! 的改动面，因此同组。没有插槽时的**降级实现**（[`NoopVectorStorage`] /
//! [`EphemeralStorageBackend`]）也跟着本组走：它们是「未装配时 VFS 仍要能起来」的
//! 兜底，语义与本组两个工厂一一对应。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use tianyan::config::StorageConfig;
use tianyan::vfs::{
    ContextEntry, StorageBackend, VectorPoint, VectorSearchQuery, VectorSearchResult,
    VectorStorage, VectorType,
};

use super::{StorageBackendFactory, VectorStorageFactory};

/// 结构化存储 + 向量存储工厂（`None` = 该插槽未装配，走降级实现）。
///
/// 两个字段的降级语义见 `backend()`（进程内存，`error!`）与 `vector()`
/// （空向量存储，`warn!`）——存储是启动硬依赖，因此另有
/// `has_storage_factory()` 供启动入口做硬前置。
#[derive(Clone, Default)]
pub struct StorageAssemblies {
    storage_factory: Option<StorageBackendFactory>,
    vector_factory: Option<VectorStorageFactory>,
}

impl StorageAssemblies {
    /// 装配结构化存储工厂（`app` / `tauri` 用：SQLite / 本地文件系统）。
    pub fn with_storage_factory(mut self, factory: StorageBackendFactory) -> Self {
        self.storage_factory = Some(factory);
        self
    }

    /// 装配向量存储工厂（`app` / `tauri` 用：LanceDB）。
    pub fn with_vector_factory(mut self, factory: VectorStorageFactory) -> Self {
        self.vector_factory = Some(factory);
        self
    }

    /// 结构化存储工厂（`None` = 用进程内存兜底，见 [`Self::backend`]）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根
    /// （`initialize_vfs_for_app` / `start_server_inner` 的守卫）取用——装配层负责
    /// **注入**，不负责调用。
    pub(crate) fn storage_factory(&self) -> Option<StorageBackendFactory> {
        self.storage_factory.clone()
    }

    /// 向量存储工厂（`None` = 用空向量存储兜底，见 [`Self::vector`]）。
    pub(crate) fn vector_factory(&self) -> Option<VectorStorageFactory> {
        self.vector_factory.clone()
    }

    /// 是否装配了结构化存储工厂（**启动服务的硬前置**，见 `start_server_inner`）。
    ///
    /// 为什么单独开一个查询方法而不是让 `backend()` 自己报错：
    /// `create_app`（测试/嵌入式构造对象图）需要允许内存兜底，而
    /// `start_server*`（真跑服务）必须拒绝——两条路径的严格度不同，判定点必须能
    /// 被调用方分别选用。守卫放在 `start_server_inner` 入口而不是工厂里，是为了让
    /// 失败发生在**启动前**（清晰报错）而不是运行期第一次写入时（数据已丢）。
    pub(crate) fn has_storage_factory(&self) -> bool {
        self.storage_factory.is_some()
    }

    /// 构造 VFS 结构化存储后端实例（**唯一的存储实现选择点**）。
    ///
    /// 未装配工厂时返回 [`EphemeralStorageBackend`]（**进程内存**：内容读写正常
    /// 但**不落盘**，重启即丢）。为什么是兜底而不是 `Err`——两个理由：
    ///
    /// 1. **旧入口不能坏**：`create_app` / `start_server*` 这组不带装配的入口是
    ///    公开 API，约 60 处测试与嵌入式调用靠它起一个能跑的实例（ADR-049 序 3
    ///    明确「旧签名全部保留且语义等价」）。存储是**启动硬依赖**，返回 `Err`
    ///    会让这批入口在**任何**构建里都起不来——那等于删掉它们，而不是收敛装配。
    /// 2. **实现拿不到**：`LocalFileBackend` / `SqliteBackend` 已在插件里
    ///    （`tianyan-storage-sqlite`），本库**不能** `use` 它；兜底只能在 server
    ///    内自带。写一份「本地文件/SQLite 的复制品」是两份实现同时漂移，比内存
    ///    兜底危险得多。
    ///
    /// **代价必须可见**（ADR-038 可见失败，比序 3 的向量降级更严）：装配期
    /// `error!` 一条、首次写入再 `error!` 一次（`atomic` 只打一次）。序 3 的向量
    /// 降级只是「没有语义检索」，这里降级是「**VFS 内容不落盘**」——生产误配的
    /// 症状会是「重启后数据全部消失」，所以用 `error` 级而不是 `warn`。
    pub(crate) fn backend(
        &self,
        config: &StorageConfig,
        database: Arc<tianyan::db::Database>,
    ) -> tianyan::Result<Arc<dyn StorageBackend>> {
        match self.storage_factory() {
            Some(factory) => factory(config, database),
            None => {
                error!(
                    backend = ?config.backend,
                    "未装配结构化存储工厂：VFS 使用**进程内存**存储（内容不落盘，重启即丢）——\
                     生产装配请用 create_app_with_assembly / start_server_with_assembly"
                );
                Ok(Arc::new(EphemeralStorageBackend::new()))
            }
        }
    }

    /// 构造向量存储实例（**唯一的向量实现选择点**）。
    ///
    /// 未装配工厂时返回 [`NoopVectorStorage`]：VFS 仍可工作（内容读写/摘要/工具
    /// 全部正常），只是**没有**语义检索。这条路径服务于测试与嵌入式调用
    /// （`create_app`）——生产装配（`app` / `tauri`）必须注入工厂，否则启动日志
    /// 会有明确告警。
    pub(crate) async fn vector(
        &self,
        config: &StorageConfig,
    ) -> tianyan::Result<Arc<dyn VectorStorage>> {
        match self.vector_factory() {
            Some(factory) => factory(config).await,
            None => {
                warn!(
                    "未装配向量存储工厂：VFS 使用空向量存储（无语义检索，向量写入被丢弃）——\
                     生产装配请用 create_app_with_assembly / start_server_with_assembly"
                );
                Ok(Arc::new(NoopVectorStorage::new(config)))
            }
        }
    }
}

/// 空向量存储（未装配 [`VectorStorageFactory`] 时的降级实现）。
///
/// 语义：**写入丢弃、检索恒空、行数恒 0**。存在的意义是让 VFS 的依赖
/// （`Arc<dyn VectorStorage>`）在测试/嵌入式路径上可满足，而不必把 LanceDB 这类
/// 重实现拖回 core 或引入 `Option` 分支污染整个 VFS。
///
/// 不持久化任何东西：`create_app`（无装配）跑出来的实例没有语义检索能力，这是
/// **有意的**——真装配由装配层注入（[`StorageAssemblies::with_vector_factory`]）。
///
/// `pub(crate)` 只为**本库的插槽测试**（`assembly/tests.rs` 要直接构造它来断言
/// 「装配了工厂就只用工厂」与「首次写入留下证据」）；不对外暴露。
pub(crate) struct NoopVectorStorage {
    pub(crate) embedding_dim: usize,
    /// 首次向量写入时告警一次。
    ///
    /// `upsert_point` 丢弃数据是**静默**的：若不留下证据，生产误配的唯一表现会是
    /// 「语义检索永远为空」，看不出是降级还是数据缺失（ADR-038 可见失败）。启动期
    /// 的 warn 只能覆盖「进程起动那一刻在读日志的人」，本字段把证据补到**真正
    /// 丢数据的那一刻**，且只打一次（向量写入是热路径，不能刷屏）。
    pub(crate) upsert_warned: AtomicBool,
}

impl NoopVectorStorage {
    /// 维度取配置值（保持 `embedding_dim()` 与配置一致，避免下游按 0 维构造请求）。
    pub(crate) fn new(config: &StorageConfig) -> Self {
        Self {
            embedding_dim: config.vector.vector_dimension,
            upsert_warned: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl VectorStorage for NoopVectorStorage {
    fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    async fn initialize(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn upsert_point(&self, _point: &VectorPoint) -> tianyan::Result<()> {
        if !self.upsert_warned.swap(true, Ordering::Relaxed) {
            warn!(
                "空向量存储**丢弃**了向量写入（该实例无语义检索能力）：装配层未注入 \
                 VectorStorageFactory —— 生产装配请用 start_server_with_assembly / \
                 create_app_with_assembly（本告警只打一次）"
            );
        }
        Ok(())
    }

    async fn delete_point(&self, _id: &str) -> tianyan::Result<()> {
        Ok(())
    }

    async fn search(&self, _query: VectorSearchQuery) -> tianyan::Result<Vec<VectorSearchResult>> {
        Ok(Vec::new())
    }

    async fn get_point(&self, _id: &str) -> tianyan::Result<Option<VectorPoint>> {
        Ok(None)
    }

    async fn search_fused(
        &self,
        _query_vector: Vec<f32>,
        _vector_types: &[VectorType],
        _top_k: usize,
        _category_filter: Option<&str>,
        _min_score: Option<f32>,
    ) -> tianyan::Result<Vec<VectorSearchResult>> {
        Ok(Vec::new())
    }
}

/// 进程内存存储后端（未装配 [`StorageBackendFactory`] 时的降级实现）。
///
/// 语义**照 `SqliteBackend` 抄**：键是 URI，`is_directory` 是条目自有字段，删除与
/// 列举都用「URI 后接斜杠」的字符串前缀。两者唯一的差别是「不落盘」。刻意不实现成
/// 「本地文件后端」——那会与插件里的 `LocalFileBackend` 争同一批路径约定，且形态
/// 语义（条目是文件还是目录）与生产默认后端 `SqliteBackend` 不一致，正是 ADR-049
/// 反复强调的「Mock 形态掩盖缺陷」的坑。
///
/// 它服务于 `create_app` / `start_server*` 这组**不带装配**的入口（测试与嵌入式，
/// 见 [`StorageAssemblies::backend`]）。**没有任何持久化承诺**：进程退出即
/// 全部消失——这不是缺陷而是刻意的（写 `error!` 留证据，不静默丢数据）。
///
/// `pub(crate)` 只为**本库的启动守卫测试**：`lib.rs::tests` 要构造「已装配存储、未装配
/// 上下文策略」的装配来锁住第二道守卫，而它不需要一个真的落盘后端（守卫在 `create_app`
/// 之前就返回了）。不对外暴露（`pub(crate)`，`tianyan_server` 的公开面仍然只有工厂类型）。
pub(crate) struct EphemeralStorageBackend {
    /// 键 = `uri.to_string()`（与 `vfs_entries` 主键同口径）。
    pub(crate) entries: tokio::sync::RwLock<std::collections::HashMap<String, ContextEntry>>,
    /// 首次「本应落盘」的写入时告警一次（写入是热路径，不能刷屏）。
    pub(crate) write_warned: AtomicBool,
}

impl EphemeralStorageBackend {
    /// 构造空的进程内存后端（语义见类型文档）。
    pub(crate) fn new() -> Self {
        Self {
            entries: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            write_warned: AtomicBool::new(false),
        }
    }

    /// 首次写入时补一条证据：装配期的 `error!` 只覆盖「起进程那一刻在读日志的人」。
    fn warn_write_once(&self) {
        if !self.write_warned.swap(true, Ordering::Relaxed) {
            error!(
                "内存存储后端**未落盘**地写入了 VFS 条目（进程退出即丢）：装配层未注入 \
                 StorageBackendFactory —— 生产装配请用 start_server_with_assembly / \
                 create_app_with_assembly（本告警只打一次）"
            );
        }
    }

    /// 取某层级字段的可变引用（`ContentLevel` → `ContextEntry` 字段的单点映射）。
    fn level_mut(
        entry: &mut ContextEntry,
        level: tianyan::common::types::ContentLevel,
    ) -> &mut Option<String> {
        use tianyan::common::types::ContentLevel;
        match level {
            ContentLevel::Abstract => &mut entry.abstract_content,
            ContentLevel::Overview => &mut entry.overview_content,
            ContentLevel::Detail => &mut entry.detail_content,
        }
    }
}

#[async_trait]
impl StorageBackend for EphemeralStorageBackend {
    async fn initialize(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn exists(&self, uri: &tianyan::common::types::TianyanUri) -> tianyan::Result<bool> {
        Ok(self.entries.read().await.contains_key(&uri.to_string()))
    }

    async fn read_entry(
        &self,
        uri: &tianyan::common::types::TianyanUri,
    ) -> tianyan::Result<ContextEntry> {
        self.entries
            .read()
            .await
            .get(&uri.to_string())
            .cloned()
            .ok_or_else(|| tianyan::TianyanError::not_found(uri.to_string()))
    }

    async fn write_entry(&self, entry: &ContextEntry) -> tianyan::Result<()> {
        self.warn_write_once();
        self.entries
            .write()
            .await
            .insert(entry.uri().to_string(), entry.clone());
        Ok(())
    }

    async fn delete_entry(&self, uri: &tianyan::common::types::TianyanUri) -> tianyan::Result<()> {
        self.warn_write_once();
        let key = uri.to_string();
        // 子条目前缀：URI 后接斜杠——与 SqliteBackend 的 substr/length 同口径
        // （只匹配直接/间接子条目，不误删同前缀兄弟 ab 与 abc）。
        let dir_prefix = format!("{}/", key);
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|k, _| k != &key && !k.starts_with(&dir_prefix));
        if entries.len() == before {
            return Err(tianyan::TianyanError::not_found(key));
        }
        Ok(())
    }

    async fn list_directory(
        &self,
        uri: &tianyan::common::types::TianyanUri,
    ) -> tianyan::Result<Vec<ContextEntry>> {
        let prefix = format!("{}/", uri);
        let entries = self.entries.read().await;
        Ok(entries
            .iter()
            .filter_map(|(key, entry)| {
                let rest = key.strip_prefix(&prefix)?;
                // 只列一级子条目：前缀之后不得再含 '/'
                if rest.contains('/') {
                    return None;
                }
                Some(entry.clone())
            })
            .collect())
    }

    async fn read_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
    ) -> tianyan::Result<String> {
        let entries = self.entries.read().await;
        let missing = || tianyan::TianyanError::not_found(format!("{uri} 无 {level:?} 层级内容"));
        let mut entry = entries.get(&uri.to_string()).cloned().ok_or_else(missing)?;
        Self::level_mut(&mut entry, level)
            .clone()
            .ok_or_else(missing)
    }

    async fn write_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
        content: &str,
    ) -> tianyan::Result<()> {
        self.warn_write_once();
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(uri.to_string())
            .or_insert_with(|| ContextEntry::new_file(uri.clone()));
        *Self::level_mut(entry, level) = Some(content.to_string());
        entry.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }

    async fn append_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
        content: &str,
    ) -> tianyan::Result<()> {
        self.warn_write_once();
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(uri.to_string())
            .or_insert_with(|| ContextEntry::new_file(uri.clone()));
        let slot = Self::level_mut(entry, level);
        match slot {
            Some(existing) => existing.push_str(content),
            None => *slot = Some(content.to_string()),
        }
        entry.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }
}
