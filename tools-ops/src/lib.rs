//! 天演运维工具（ADR-049 期 4d）：**服务停止后直接修数据**的一次性脚本。
//!
//! ## 为什么单独成包
//! 这些脚本要「不启动服务、手工装配一个最小 VFS、直接改库」，因此需要
//! `storage-sqlite`（结构化后端）+ `storage-vector`（向量）+ 两个 provider 插件 +
//! 服务层配置。它们原本寄居在 `plugins/storage-vector/examples/`，靠 5 条**插件间
//! dev-dependency** 撑着（每个 Cargo.toml 都要解释一遍「只服务 examples」）。
//!
//! 本包是**终端消费者**（不被任何 crate 依赖）：依赖一切是合法的——与 `app` / `tauri`
//! 同类。插件之间因此连 dev 边都不再有（D2-2 的生产图与开发图都干净）。
//!
//! ## 为什么不能复用 `tianyan_assembly::full()`
//! `full()` 返回的是工厂集合（`AssemblyServices`），真正构造 VFS 还要经 server 的
//! `initialize_vfs_for_app`（依赖 `AppState` 一整串对象图）；而本包的前提恰恰是
//! **不启动服务**。因此这里保留一份「最小装配」——它是运维语义，不进生产装配。

use std::path::PathBuf;
use std::sync::Arc;

use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::db::Database;
use tianyan::vfs::{StorageBackend, VfsCore, VirtualFileSystemBuilder, VirtualFileSystemImpl};
use tianyan_provider_embed::services as embedding_services;
use tianyan_storage_sqlite::SqliteBackend;
use tianyan_storage_vector::LanceDbVectorStore;

/// 加载真实配置（`~/.tianyan/tianyan.toml`），可覆盖数据目录。
///
/// `data_dir_override` 来自脚本的 `--data-dir`（彩排 / 隔离环境）；覆盖时清空
/// `sqlite_path`，让数据库回到 `{data_dir}/tianyan.db`（与脚本原逻辑一致）。
pub fn load_config(
    data_dir_override: Option<PathBuf>,
) -> Result<TianyanConfig, tianyan::TianyanError> {
    // 加载器随 ADR-049 期 3-T4 搬去 `tianyan-service-config`
    let mut config = tianyan_service_config::load()?;
    if let Some(d) = &data_dir_override {
        config.storage.data_dir = d.clone();
        config.storage.sqlite_path = None;
    }
    Ok(config)
}

/// 用配置装配并初始化一个最小 VFS（结构化 SQLite + LanceDB 向量 + 嵌入 provider）。
///
/// 与 `server::initialize_vfs_for_app` **同构但更小**：只装脚本需要的那几件，不碰
/// 会话/快照/角色/MCP。返回具体类型 `VirtualFileSystemImpl`（builder 的产物形态），
/// 脚本直接调用其公开方法（`read_content` / `write_overview` / `search` 等）。
pub async fn open_vfs(
    config: &TianyanConfig,
) -> Result<VirtualFileSystemImpl, tianyan::TianyanError> {
    let database = Database::open(db_path(config))?;
    database.init_schemas().await?;
    let storage: Arc<dyn StorageBackend> = Arc::new(SqliteBackend::new(database));
    let vector_storage = Arc::new(LanceDbVectorStore::new(&config.storage).await?);
    let embed = embedding_services(&config.models)?;
    let emb_model = config
        .models
        .resolve(ModelCapability::TextEmbedding)
        .map(|r| r.model)
        .unwrap_or_else(|| "text-embedding-3-small".to_string());
    let vfs = VirtualFileSystemBuilder::new()
        .with_config(config.storage.clone())
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_embedding_provider(embed.embedding, emb_model)
        .build()?;
    vfs.initialize().await?;
    Ok(vfs)
}

/// 打印「数据目录 / 数据库」两行（两个脚本的固定开场，避免各写一遍）。
pub fn print_targets(config: &TianyanConfig) {
    println!("数据目录：{}", config.storage.data_dir.display());
    println!("数据库：{}", db_path(config).display());
}

/// 数据库文件路径：显式 `sqlite_path` 优先，否则落在数据目录下的 `tianyan.db`。
fn db_path(config: &TianyanConfig) -> PathBuf {
    config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"))
}
