//! 装配扩展点（ADR-049）：插件工具清单 + 向量存储实现。
//!
//! server 库**不认识任何具体插件 crate**：本模块只定义两根**类型擦除**的插槽
//! （以及没有插槽时的降级实现），由装配层（`app` bin / `tauri` 桌面端）在启动
//! 时注入。这样改一行插件实现只重编插件 + 装配包，server 库（13.4K 源码）不动
//! ——闸门 A 的前提。
//!
//! 两个插槽同型：
//! - [`ToolPluginFactory`]（`crate::ToolPluginFactory`）：工具清单，**可重复调用**
//!   （agent 会因配置热重载 / 模型切换反复重建，每次都要重新注入插件工具）。
//! - [`VectorStorageFactory`]：向量存储实现（`tianyan-storage-vector` 的
//!   LanceDB 后端）。**异步**：LanceDB 建连/建表是 async，且失败要能向上抛（启动
//!   期明确失败优于静默无向量）。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use tianyan::config::StorageConfig;
use tianyan::vfs::{VectorPoint, VectorSearchQuery, VectorSearchResult, VectorStorage, VectorType};

use crate::ToolPluginFactory;

/// 向量存储工厂（**装配层注入**，期 2 序 3）。
///
/// `Arc<dyn Fn(&StorageConfig) -> Pin<Box<dyn Future<Output = Result<Arc<dyn
/// VectorStorage>>> + Send>>>`：参数是 `&StorageConfig`（借用的，实现内部须自行
/// 克隆需要的数据），返回 boxed future（trait 对象无法用 `async fn`）。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：启动入口可能被复用（数据目录搬迁后
/// 以新配置重启、多端口探测重试），工厂必须是可重复调用的。
pub type VectorStorageFactory = Arc<
    dyn Fn(
            &StorageConfig,
        ) -> Pin<
            Box<dyn std::future::Future<Output = tianyan::Result<Arc<dyn VectorStorage>>> + Send>,
        > + Send
        + Sync,
>;

/// 启装配所需的外部能力集合（`None` = 该扩展点未装配，走降级路径）。
///
/// 目前两个插槽：插件工具清单（[`ToolPluginFactory`]）与向量存储
/// （[`VectorStorageFactory`]）。用**一个结构**而不是给每个启动入口再加一个
/// 参数：入口变体的数量是组合爆炸（listener × 关闭信号 × 插件 × 向量），打包
/// 后只需「默认装配」与「装配齐全」两类入口。
#[derive(Clone, Default)]
pub struct AssemblyServices {
    plugin_factory: Option<ToolPluginFactory>,
    vector_factory: Option<VectorStorageFactory>,
}

impl AssemblyServices {
    /// 空装配（`create_app` / 嵌入式 / 测试路径：无插件工具、无持久化向量存储）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 装配插件工具清单工厂（`app` / `tauri` 用）。
    pub fn with_plugin_factory(mut self, factory: ToolPluginFactory) -> Self {
        self.plugin_factory = Some(factory);
        self
    }

    /// 装配向量存储工厂（`app` / `tauri` 用：LanceDB）。
    pub fn with_vector_factory(mut self, factory: VectorStorageFactory) -> Self {
        self.vector_factory = Some(factory);
        self
    }

    /// 插件工具清单工厂（`None` = 不注入插件工具）。
    pub(crate) fn plugin_factory(&self) -> Option<ToolPluginFactory> {
        self.plugin_factory.clone()
    }

    /// 构造向量存储实例（**唯一的向量实现选择点**）。
    ///
    /// 未装配工厂时返回 [`NoopVectorStorage`]：VFS 仍可工作（内容读写/摘要/工具
    /// 全部正常），只是**没有**语义检索。这条路径服务于测试与嵌入式调用
    /// （`create_app`）——生产装配（`app` / `tauri`）必须注入工厂，否则启动日志
    /// 会有明确告警。
    pub(crate) async fn vector_storage(
        &self,
        config: &StorageConfig,
    ) -> tianyan::Result<Arc<dyn VectorStorage>> {
        match self.vector_factory.as_ref() {
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
/// **有意的**——真装配由装配层注入（[`AssemblyServices::with_vector_factory`]）。
struct NoopVectorStorage {
    embedding_dim: usize,
    /// 首次向量写入时告警一次。
    ///
    /// `upsert_point` 丢弃数据是**静默**的：若不留下证据，生产误配的唯一表现会是
    /// 「语义检索永远为空」，看不出是降级还是数据缺失（ADR-038 可见失败）。启动期
    /// 的 warn 只能覆盖「进程起动那一刻在读日志的人」，本字段把证据补到**真正
    /// 丢数据的那一刻**，且只打一次（向量写入是热路径，不能刷屏）。
    upsert_warned: AtomicBool,
}

impl NoopVectorStorage {
    /// 维度取配置值（保持 `embedding_dim()` 与配置一致，避免下游按 0 维构造请求）。
    fn new(config: &StorageConfig) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 未装配工厂时降级为空向量存储：VFS 不会因此构建失败，但检索恒空。
    #[tokio::test]
    async fn test_default_assembly_yields_noop_vector_storage() {
        let config = StorageConfig {
            vector: tianyan::config::VectorStorageConfig {
                collection_name: "noop_test".to_string(),
                vector_dimension: 8,
            },
            ..Default::default()
        };
        let storage = AssemblyServices::new()
            .vector_storage(&config)
            .await
            .unwrap();
        assert_eq!(storage.embedding_dim(), 8);
        assert_eq!(storage.count_rows().await.unwrap(), 0);
        assert!(storage.list_point_ids().await.unwrap().is_empty());
    }

    /// 装配了工厂时必须**只用工厂**（降级路径不得抢占）。
    #[tokio::test]
    async fn test_assembly_uses_injected_vector_factory() {
        let factory: VectorStorageFactory = Arc::new(|config| {
            let dim = config.vector.vector_dimension;
            Box::pin(async move {
                let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                    embedding_dim: dim + 1,
                    upsert_warned: AtomicBool::new(false),
                });
                Ok(storage)
            })
        });
        let config = StorageConfig {
            vector: tianyan::config::VectorStorageConfig {
                collection_name: "injected".to_string(),
                vector_dimension: 8,
            },
            ..Default::default()
        };
        let storage = AssemblyServices::new()
            .with_vector_factory(factory)
            .vector_storage(&config)
            .await
            .unwrap();
        assert_eq!(
            storage.embedding_dim(),
            9,
            "应使用注入的工厂（维度 +1 是工厂内的判别标记）"
        );
    }

    /// 工厂必须可重复调用（启动入口会被数据目录搬迁后的重启复用）。
    #[tokio::test]
    async fn test_vector_factory_is_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: VectorStorageFactory = Arc::new(move |_config| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                    embedding_dim: 4,
                    upsert_warned: AtomicBool::new(false),
                });
                Ok(storage)
            })
        });
        let assembly = AssemblyServices::new().with_vector_factory(factory);
        let config = StorageConfig::default();
        let _ = assembly.vector_storage(&config).await.unwrap();
        let _ = assembly.vector_storage(&config).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );
    }

    /// 空向量存储必须在**首次写入**时留下丢弃证据。
    ///
    /// 判别力：若删掉 `upsert_point` 里的 `swap`，生产误配的唯一表现就是「检索恒空」，
    /// 日志无任何线索（ADR-038 可见失败）；本测试锁住「写入路径有痕迹」。
    #[tokio::test]
    async fn test_noop_vector_storage_warns_on_first_upsert_only() {
        let config = StorageConfig::default();
        let storage = NoopVectorStorage::new(&config);
        let point = VectorPoint {
            schema_version: 1,
            id: "p1".to_string(),
            abstract_vector: Some(vec![0.0; 4]),
            overview_vector: None,
            visual_vector: None,
            payload: tianyan::common::types::EntryMetadata::default(),
        };
        // 丢弃不等于失败：写入必须仍然 Ok，否则会把「降级」变成「报错」。
        for _ in 0..3 {
            storage.upsert_point(&point).await.expect("丢弃写入不报错");
        }
        assert!(
            storage.upsert_warned.load(Ordering::Relaxed),
            "首次写入后须置起告警标志（证据链）；随后不再重复打日志（热路径不刷屏）"
        );
        assert_eq!(storage.count_rows().await.expect("计数"), 0);
    }
}
