//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.

use std::sync::Arc;

use tianyan::vfs::{StorageBackend, VectorStorage};
use tianyan_server::{
    start_server_with_shutdown_signal_and_assembly, AssemblyServices, PluginServices,
    StorageBackendFactory, TaskFactory, ToolPluginFactory, VectorStorageFactory,
};
use tianyan_storage_sqlite::{LocalFileBackend, SqliteBackend};
use tianyan_storage_vector::LanceDbVectorStore;
use tracing::{error, info};

/// 服务器监听地址（仅本机回环，不对外暴露）。
pub const SERVER_HOST: &str = "127.0.0.1";
/// 首选端口（被占用时由 [`find_available_port`] 动态选择替代端口）。
pub const PREFERRED_PORT: u16 = 3000;

/// 探测并绑定可用端口：优先首选端口，被占用时递增扫描（最多 100 个）。
///
/// 返回**已绑定**的 listener：探测与监听原子化，消除
/// "探测端口 → 释放 → 重新绑定"之间的竞态窗口。
///
/// 返回 `None` 表示 `preferred..=preferred+100` 全部被占用。
pub fn find_available_port(preferred: u16) -> Option<std::net::TcpListener> {
    for port in preferred..=preferred.saturating_add(100) {
        if let Ok(listener) = std::net::TcpListener::bind((SERVER_HOST, port)) {
            return Some(listener);
        }
    }
    None
}

/// 向量存储工厂（LanceDB；`plugins/storage-vector`）。
///
/// 与插件工具清单同款类型擦除：server 库只认 [`VectorStorageFactory`]，
/// `LanceDbVectorStore` 只出现在装配层。`&StorageConfig` 是借用 → 先 `clone`
/// 再 move 进 async 块；工厂可重复调用（启动重试 / 搬迁重启）。
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
/// 与 `app/src/main.rs::storage_factory` **同形同源**（ADR-049 期 2 序 4）：
/// `config.storage.backend` 选 adapter 的语义只在装配层，server 库不认识这两个
/// 具体类型。同步工厂：两个后端的 `new` 都不做 IO。
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

/// 定时任务工厂（`plugins/task-builtin` 序 5a + `plugins/task-evolution` 序 5b）。
///
/// 与 `app/src/main.rs::task_factory` **同形同源**：5 个内置任务与自演化综述任务的
/// 实现与注册清单（名字 / 显示名 / 间隔 / 启用条件）都在插件里，server 库不认识任何
/// 具体任务。
///
/// **顺序拼接在装配层**（D2-2）：注册顺序 = 扫描顺序 = 执行顺序，而「演化任务排第 2
/// 位（紧跟 `summary_generation`）」是跨插件属性——插件之间不互相 import，只有装配
/// 清单知道「哪个插件该排在哪」。`splice(1..1, …)` = 在 index 1 之前插入。
///
/// 同步工厂——`TaskServices` 由 server 在调用前 await 好（共享 `ModelServices`，
/// 不重复建 HTTP client 池）。
fn task_factory() -> TaskFactory {
    Arc::new(|services: &tianyan_server::TaskServices| {
        let mut registrations = tianyan_task_builtin::tasks(services);
        // 顺序纪律：evolution 紧跟 summary_generation（清单第 2 位，与迁移前一致）
        registrations.splice(1..1, tianyan_task_evolution::tasks(services));
        registrations
    })
}

/// 启动 Axum 服务
///
/// 此函数在后台任务中启动 Axum HTTP 服务器，
/// 服务器监听调用方传入的（已绑定）listener。
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
/// * `listener` - 已绑定的监听 socket（由 [`find_available_port`] 提供）
/// * `shutdown_signal` - 外部关闭信号：发送 `true` 触发与 Ctrl+C 相同的优雅关停
pub async fn start_axum_server(
    tianyan_config: tianyan::config::TianyanConfig,
    listener: std::net::TcpListener,
    shutdown_signal: tokio::sync::watch::Receiver<bool>,
) {
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let listener = match listener.set_nonblocking(true) {
        Ok(()) => match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                error!("转换监听 socket 失败（端口 {}）：{}", port, e);
                return;
            }
        },
        Err(e) => {
            error!("设置非阻塞模式失败（端口 {}）：{}", port, e);
            return;
        }
    };
    info!("Starting Axum server on {}:{}", SERVER_HOST, port);

    // 插件清单（ADR-049 期 1）：桌面端与独立 bin 共用同一工厂——
    // 清单单一事实源在插件 crate（`plugin_factory()`），此处只接线。
    let plugin_factory: ToolPluginFactory = Arc::new(
        |ctx: &tianyan::agent::ToolContext, services: PluginServices| {
            // 顺序即注册顺序（追加到动态工具末尾）：fs → code → todo/goal → web。
            let mut tools = tianyan_tools_fs::tools(ctx);
            tools.extend(tianyan_tools_code::tools(ctx));
            // `todo` / `goal` 需与 REST API 共享同一份 store（由 server 从 AppState
            // 经 `PluginServices` 递入；`tools()` 内部顺序固定 `todo` → `goal`）。
            tools.extend(tianyan_tools_todo::tools(
                services.todo_store(),
                services.goal_store(),
            ));
            // `web_search` / `web_fetch`（ADR-049 期 2）：客户端由插件从
            // `services.web_config()` 就地构造（配置非法时降级为空清单）。
            tools.extend(tianyan_tools_web::tools(&services.web_config()));
            tools
        },
    );
    // 完整装配（ADR-049 期 2 序 3/4/5a）：工具清单 + 向量存储 + 结构化存储 + 内置任务
    let assembly = AssemblyServices::new()
        .with_plugin_factory(plugin_factory)
        .with_vector_factory(vector_factory())
        .with_storage_factory(storage_factory())
        .with_task_factory(task_factory());
    if let Err(e) = start_server_with_shutdown_signal_and_assembly(
        listener,
        tianyan_config,
        shutdown_signal,
        assembly,
    )
    .await
    {
        error!("Server error: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port_preferred_when_free() {
        // 首选端口空闲时（CI 环境无占用）应返回首选端口
        if std::net::TcpListener::bind((SERVER_HOST, PREFERRED_PORT)).is_ok() {
            let listener = find_available_port(PREFERRED_PORT).expect("应找到可用端口");
            let port = listener.local_addr().expect("listener 应已绑定").port();
            assert_eq!(port, PREFERRED_PORT);
        }
    }

    #[test]
    fn test_find_available_port_skips_occupied() {
        // 首选端口被占用时应选择其他可用端口
        let Ok(listener) = std::net::TcpListener::bind((SERVER_HOST, PREFERRED_PORT)) else {
            return; // 端口被并发测试占用，跳过
        };
        let found = find_available_port(PREFERRED_PORT).expect("应找到可用端口");
        let port = found.local_addr().expect("listener 应已绑定").port();
        assert_ne!(port, PREFERRED_PORT, "首选端口被占用时应选择其他端口");
        drop(listener);
    }

    #[test]
    fn test_find_available_port_listener_holds_port() {
        // 竞态消除验证：返回的 listener 已持有端口——
        // 对同一端口再次 bind 必须失败，证明探测与监听是原子的
        let Ok(occupied) = std::net::TcpListener::bind((SERVER_HOST, PREFERRED_PORT)) else {
            return; // 端口被并发测试占用，跳过
        };
        let found = find_available_port(PREFERRED_PORT).expect("应找到可用端口");
        let port = found.local_addr().expect("listener 应已绑定").port();
        // 已选中的端口不应能被再次绑定
        assert!(std::net::TcpListener::bind((SERVER_HOST, port)).is_err());
        drop(occupied);
    }
}
