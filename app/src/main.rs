//! 天演装配器（服务器二进制入口）。
//!
//! ## 为什么单独成包（ADR-049 期 1）
//!
//! **装配清单住在 bin**。Cargo 的依赖粒度是 package，若清单写在
//! `tianyan-server` 的 bin 里，整个 server 包就依赖插件 crate —— 改一行工具会
//! 重编 server 库（13.4K 源码），闸门 A（≤3.5K）直接失效。
//!
//! 因此本包只做三件事：加载配置与日志、给出插件清单、启动 server。
//! server 库通过 `ToolPluginFactory`（类型擦除）接收清单，**不认识任何插件类型**。
//!
//! 期 2 序 3 起同一模式也用于**向量存储**：`lancedb`/`arrow`/`datafusion` 链搬进
//! `plugins/storage-vector` 后，core 不再承担它；本包把 `LanceDbVectorStore` 包成
//! `VectorStorageFactory` 注入启动入口（server 库同样不认识该实现）。
//!
//! 期 2 序 4 再扩一个插槽：VFS 的**结构化存储**（`SqliteBackend` /
//! `LocalFileBackend`）搬进 `plugins/storage-sqlite`，本包把两者包成
//! `StorageBackendFactory` 注入——`config.storage.backend` 选哪个 adapter 也在这里
//! 决定（server 库只递配置与共享连接）。
//!
//! 期 2 序 5a 扩第四插槽：**内置定时任务**（摘要 / GC / 统计刷盘 / 快照 GC / 提醒）
//! 搬进 `plugins/task-builtin`，本包把插件的 `tasks()` 包成 `TaskFactory` 注入
//! （server 库只认清单项类型，不认识任何具体任务）。

use std::sync::Arc;

use tianyan::vfs::{StorageBackend, VectorStorage};
use tianyan_server::{
    start_server_with_assembly, AssemblyServices, StorageBackendFactory, TaskFactory,
    ToolPluginFactory, VectorStorageFactory,
};
use tianyan_storage_sqlite::{LocalFileBackend, SqliteBackend};
use tianyan_storage_vector::LanceDbVectorStore;

/// 插件清单（**全仓唯一 `use` 具体插件类型的地方**）。
///
/// 新增插件在此追加一行；插件内部顺序即注册顺序（追加到动态工具末尾，
/// 内置工具顺序不受影响 —— 提示词前缀稳定性）。
fn plugin_factory() -> ToolPluginFactory {
    Arc::new(|ctx, services| {
        // 顺序即注册顺序（追加到动态工具末尾）：fs → code → todo/goal → web。
        let mut tools = tianyan_tools_fs::tools(ctx);
        tools.extend(tianyan_tools_code::tools(ctx));
        // `todo` / `goal` 还需要**数据存储**（会话绑定的待办 / 目标）：store 由
        // server 从 `AppState` 递过来（`PluginServices`）——工具必须与 REST API
        // 读写同一份实例，否则工具写的待办在 UI 里看不见。`tools()` 内部顺序
        // 固定 `todo` → `goal`（前缀缓存稳定性）。
        tools.extend(tianyan_tools_todo::tools(
            services.todo_store(),
            services.goal_store(),
        ));
        // `web_search` / `web_fetch`（ADR-049 期 2）：客户端由插件从
        // `services.web_config()` 就地构造（配置非法时降级为空清单，装配不
        // panic）。顺序固定在最后（web）。
        tools.extend(tianyan_tools_web::tools(&services.web_config()));
        tools
    })
}

/// 向量存储工厂（LanceDB；`plugins/storage-vector`）。
///
/// 类型擦除的同款手法：server 库只认 [`VectorStorageFactory`]（`Arc<dyn Fn>`），
/// `LanceDbVectorStore` 只在本包出现。工厂**可重复调用**（数据目录搬迁后以新配置
/// 重启会再次装配）；`&StorageConfig` 是借用，需先 `clone` 再 move 进 async 块。
fn vector_factory() -> VectorStorageFactory {
    Arc::new(|config: &tianyan::config::StorageConfig| {
        let config = config.clone();
        Box::pin(async move {
            let store = LanceDbVectorStore::new(&config).await?;
            Ok(Arc::new(store) as Arc<dyn VectorStorage>)
        })
    })
}

/// 结构化存储工厂（SQLite / 本地文件系统；`plugins/storage-sqlite`）。
///
/// **adapter 选择的唯一落地处**（ADR-049 D5）：`config.storage.backend` 决定用
/// `SqliteBackend`（默认，走共享连接——ADR-005 单连接）还是 `LocalFileBackend`。
/// server 库拿不到这个判断，因为它不认识两个具体类型。
///
/// 同步工厂（两个后端的 `new` 都只存字段，建表/建目录在 `initialize` 里）。
fn storage_factory() -> StorageBackendFactory {
    Arc::new(
        |config: &tianyan::config::StorageConfig, database: Arc<tianyan::db::Database>| {
            use tianyan::config::StorageBackendType;
            let backend: Arc<dyn StorageBackend> = match config.backend {
                StorageBackendType::Local => Arc::new(LocalFileBackend::new(config.clone())),
                StorageBackendType::Sqlite => Arc::new(SqliteBackend::new(database)),
            };
            Ok(backend)
        },
    )
}

/// 内置任务工厂（`plugins/task-builtin`；ADR-049 期 2 序 5a）。
///
/// 5 个内置任务（摘要生成 / 垃圾回收 / 统计刷盘 / 快照 GC / 主动提醒）的实现与
/// **注册清单**（名字 / 显示名 / 间隔 / 启用条件）都在插件里；本包只把
/// `tasks()` 包成类型擦除的 [`TaskFactory`]——server 库因此不认识任何具体任务。
///
/// **同步**工厂：能力集合（`TaskServices`）由 server 在调用前 await 好（聊天模型
/// 服务是 `AppState` 里共享 `ModelServices` 的克隆，不重复建 HTTP client 池），
/// 插件里只做「按配置构造处理器 + 判启用条件」。
fn task_factory() -> TaskFactory {
    Arc::new(|services: &tianyan_server::TaskServices| tianyan_task_builtin::tasks(services))
}

/// 完整装配（插件工具清单 + 向量存储 + 结构化存储 + 内置任务）。
fn assembly() -> AssemblyServices {
    AssemblyServices::new()
        .with_plugin_factory(plugin_factory())
        .with_vector_factory(vector_factory())
        .with_storage_factory(storage_factory())
        .with_task_factory(task_factory())
}

#[tokio::main]
async fn main() -> tianyan::common::error::Result<()> {
    // 日志初始化只做一次（tracing 全局 subscriber 不可重复设置；
    // 搬迁重启循环复用同一日志）
    let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
        eprintln!("配置加载失败（使用默认配置）: {}", e);
        tianyan::config::TianyanConfig::default()
    });
    tianyan::common::logging::init_logging(&config.logging)?;
    // panic hook（最后防线）：panic 现场写入 <日志目录>/panic.log
    // （与桌面端同一目录：core::common::panic_hook::default_log_dir）。
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // 主循环：数据目录搬迁后以新配置重启（搬迁由 start_server 关停后的
    // handle_pending_migration 执行；无搬迁时单次运行即退出）
    loop {
        let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
            eprintln!("配置加载失败（使用默认配置）: {}", e);
            tianyan::config::TianyanConfig::default()
        });

        // 端口来源单点：from_env 读 TIANYAN_PORT（未设 → 默认 127.0.0.1:3000）。
        // e2e 借此用备用端口启动，避免与运行中的桌面应用抢占 3000。
        let server_config = tianyan_server::ServerConfig::from_env();
        match start_server_with_assembly(server_config, config.clone(), assembly()).await {
            Ok(()) => {
                // 服务器正常关停：检查是否有待处理的数据目录搬迁
                if tianyan_server::migration::handle_pending_migration(&config)
                    .await
                    .is_some()
                {
                    eprintln!("数据目录搬迁完成，以新配置重启");
                    continue;
                }
                break;
            }
            Err(e) => {
                eprintln!("服务器错误：{}", e);
                break;
            }
        }
    }
    Ok(())
}
