//! Tianyan HTTP 服务器。

use axum::{extract::DefaultBodyLimit, response::Json, routing::get, Router};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::watch;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

use tianyan::scheduler::{TaskContext, TaskDefinition, TaskScheduler};
use tianyan::tasks::{MemoryTask, SummaryTask};

use crate::agent_builder::create_model_services;

// Import API module
pub mod agent_builder;
pub mod api;
pub mod core_bridge;
pub mod state;

use api::create_api_router;
use state::AppState;

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "127.0.0.1".to_string(),
        }
    }
}

impl ServerConfig {
    /// Create a new server config with custom port
    pub fn with_port(port: u16) -> Self {
        Self {
            port,
            ..Default::default()
        }
    }

    /// Create a new server config with custom host and port
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
}

/// Health check handler
async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

/// Get the static files directory path
///
/// This function tries to find the gui/dist directory from various possible locations
fn get_static_dir() -> std::path::PathBuf {
    // Try to find the project root by looking for Cargo.toml
    let current_dir = std::env::current_dir().unwrap_or_default();

    // Check if we're running from target/release or target/debug
    let possible_paths = [
        // From target/release or target/debug - go up 3 levels to project root
        current_dir.join("../../..").join("gui/dist"),
        // Direct from project root
        current_dir.join("gui/dist"),
        // From tauri directory
        current_dir.join("../gui/dist"),
    ];

    for path in &possible_paths {
        if path.exists() && path.join("index.html").exists() {
            return path.clone();
        }
    }

    // Default fallback - use the first option and let it fail gracefully if not found
    possible_paths[0].clone()
}

/// 应用级 VFS 初始化
///
/// 创建并初始化单一的 VFS 实例，供整个应用使用
fn initialize_vfs_for_app(
    config: &tianyan::config::TianyanConfig,
) -> anyhow::Result<Arc<tianyan::vfs::VirtualFileSystemImpl>> {
    use tianyan::vfs::{
        LocalStorageBackend, QdrantVectorStore, VectorStorage, VirtualFileSystemBuilder,
    };

    // 1. 创建存储后端
    let storage = Arc::new(LocalStorageBackend::new(config.storage.clone()));
    let vector_storage = Arc::new(QdrantVectorStore::new(&config.storage)?);
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { vector_storage.initialize().await })
    })?;

    // 2. 创建模型服务（用于嵌入服务）
    let model_services = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { create_model_services(config).await })
    })
    .map_err(|e| anyhow::anyhow!("模型服务创建失败：{}", e))?;

    let embedding_service = model_services.embedding;
    let embedding_model = config.models.default_embedding_model.clone();

    // 3. 初始化 VFS（单一实例）
    let vfs = VirtualFileSystemBuilder::new()
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_config(config.storage.clone())
        .with_embedding_service(embedding_service, embedding_model)
        .build()
        .map_err(|e| anyhow::anyhow!("VFS 构建失败：{}", e))?;

    use tianyan::vfs::VfsCore;
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { vfs.initialize().await })
    })?;

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async { tianyan::vfs::ensure_vfs_structure(&vfs).await })
    })?;

    Ok(Arc::new(vfs))
}

/// Create the application router with configuration
///
/// 支持无配置启动，用于配置向导模式
///
/// 返回 (Router, AppState) 元组，以便在关闭时访问 AppState
fn create_app(config: tianyan::config::TianyanConfig) -> anyhow::Result<(Router, Arc<AppState>)> {
    // 在应用层初始化 VFS（单一实例）
    let vfs = initialize_vfs_for_app(&config)?;

    // Create shared application state（传入 VFS）
    let state = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { AppState::new(config, vfs).await })
    })?;

    info!("Application state initialized successfully");
    let state = Arc::new(state);

    // Configure CORS - 仅允许本地来源访问
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:3000".parse().unwrap(),
            "http://localhost:1420".parse().unwrap(),
            "http://127.0.0.1:3000".parse().unwrap(),
            "http://127.0.0.1:1420".parse().unwrap(),
            "tauri://localhost".parse().unwrap(),
        ])
        .allow_methods(Any)
        .allow_headers(Any);

    // Create API router
    let api_router = create_api_router(state.clone());

    // Serve static files from gui/dist directory
    let static_dir = get_static_dir();
    info!("Serving static files from: {}", static_dir.display());

    let static_service = ServeDir::new(static_dir).append_index_html_on_directories(true);

    Ok((
        Router::new()
            .route("/health", get(health_check))
            .merge(api_router)
            .fallback_service(static_service)
            .layer(cors)
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024)), // 50MB 请求体限制
        state,
    ))
}

/// Start the HTTP server
///
/// # Arguments
///
/// * `config` - Server configuration (port, host, etc.)
/// * `tianyan_config` - Tianyan core configuration
///
/// # Errors
///
/// Returns an error if the server fails to start or bind to the address
///
/// # Example
///
/// ```no_run
/// use tianyan_server::{start_server, ServerConfig};
/// use tianyan::config::get_config;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let server_config = ServerConfig::with_port(3000);
///     let tianyan_config = get_config()?.clone();
///     start_server(server_config, tianyan_config).await
/// }
/// ```
pub async fn start_server(
    config: ServerConfig,
    tianyan_config: tianyan::config::TianyanConfig,
) -> anyhow::Result<()> {
    let (app, state) = create_app(tianyan_config)?;

    // 创建任务调度器
    let task_scheduler = Arc::new(TaskScheduler::new());

    // 创建任务上下文
    let vfs = state.vfs();
    let summary_engine = state.create_summary_engine()?;
    let memory_extractor = state.create_memory_extractor()?;
    let app_config = Arc::new(state.config().read().await.clone());

    let task_ctx = Arc::new(TaskContext::new(
        vfs,
        summary_engine,
        memory_extractor,
        app_config,
    ));

    // 注册摘要任务（每 5 分钟）
    task_scheduler
        .register_task(TaskDefinition::new(
            "summary_generation",
            "摘要生成",
            "0 */5 * * * *",
            Arc::new(SummaryTask::new()),
        ))
        .await?;

    // 注册记忆任务（每 10 分钟）
    task_scheduler
        .register_task(TaskDefinition::new(
            "memory_extraction",
            "记忆提取",
            "0 */10 * * * *",
            Arc::new(MemoryTask::new()),
        ))
        .await?;

    // 启动任务调度器
    TaskScheduler::start_with_scheduler(task_scheduler.clone(), task_ctx.clone()).await?;
    info!("任务调度器已启动");

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?;

    info!("Starting Tianyan server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        error!("Failed to bind to address {}: {}", addr, e);
        anyhow::anyhow!("Failed to bind to address: {}", e)
    })?;

    info!("Server is ready to accept connections");

    // 创建 shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // 克隆 shutdown receiver 用于服务器优雅关闭
    let mut shutdown_rx_server = shutdown_rx.clone();

    // 启动服务器，设置优雅关闭
    let server_handle = tokio::spawn(async move {
        let server = axum::serve(listener, app);
        let shutdown_signal = async move {
            // 等待 shutdown 信号
            let _ = shutdown_rx_server.changed().await;
            info!("收到关闭信号，服务器停止接受新连接");
        };

        server.with_graceful_shutdown(shutdown_signal).await
    });

    // 监听关闭信号
    #[cfg(unix)]
    let shutdown_result = tokio::select! {
        ctrl_c_result = tokio::signal::ctrl_c() => {
            match ctrl_c_result {
                Ok(()) => {
                    info!("捕获到 Ctrl+C 信号");
                    "ctrl_c"
                }
                Err(e) => {
                    error!("监听 Ctrl+C 失败：{}", e);
                    "error"
                }
            }
        },
        sigterm_result = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) => {
            match sigterm_result {
                Ok(mut signal) => {
                    if signal.recv().await.is_some() {
                        info!("捕获到 SIGTERM 信号");
                        "sigterm"
                    } else {
                        "sigterm_error"
                    }
                }
                Err(e) => {
                    error!("监听 SIGTERM 失败：{}", e);
                    "error"
                }
            }
        },
        server_result = server_handle => {
            match server_result {
                Ok(Ok(())) => {
                    info!("服务器正常退出");
                    "server_stopped"
                }
                Ok(Err(e)) => {
                    error!("服务器错误：{}", e);
                    "server_error"
                }
                Err(e) => {
                    error!("服务器任务失败：{}", e);
                    "server_task_error"
                }
            }
        }
    };

    #[cfg(not(unix))]
    let shutdown_result = tokio::select! {
        ctrl_c_result = tokio::signal::ctrl_c() => {
            match ctrl_c_result {
                Ok(()) => {
                    info!("捕获到 Ctrl+C 信号");
                    "ctrl_c"
                }
                Err(e) => {
                    error!("监听 Ctrl+C 失败：{}", e);
                    "error"
                }
            }
        },
        server_result = server_handle => {
            match server_result {
                Ok(Ok(())) => {
                    info!("服务器正常退出");
                    "server_stopped"
                }
                Ok(Err(e)) => {
                    error!("服务器错误：{}", e);
                    "server_error"
                }
                Err(e) => {
                    error!("服务器任务失败：{}", e);
                    "server_task_error"
                }
            }
        }
    };

    // 如果是因为信号触发的关闭，发送 shutdown 信号
    if shutdown_result != "server_stopped"
        && shutdown_result != "server_error"
        && shutdown_result != "server_task_error"
    {
        info!("开始优雅关闭流程...");

        // 发送 shutdown 信号
        if let Err(e) = shutdown_tx.send(true) {
            error!("发送 shutdown 信号失败：{}", e);
        }

        // 等待服务器停止接受新连接
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 停止任务调度器
        info!("停止任务调度器...");
        if let Err(e) = task_scheduler.shutdown().await {
            error!("停止任务调度器失败：{}", e);
        }

        // 调用 AppState::shutdown() 等待所有 pending 任务完成
        info!("等待待处理任务完成...");

        use tokio::time::{timeout, Duration};
        let max_wait = Duration::from_secs(30);

        match timeout(max_wait, state.shutdown()).await {
            Ok(Ok(())) => {
                info!("应用已优雅关闭");
            }
            Ok(Err(e)) => {
                error!("关闭过程中发生错误：{}", e);
            }
            Err(_) => {
                warn!("关闭超时 ({}s)，强制退出", max_wait.as_secs());
            }
        }
    }

    Ok(())
}

/// Start the server with default configuration
///
/// This is a convenience function that starts the server with default settings
/// (host: 127.0.0.1, port: 3000)
pub async fn start_server_default() -> anyhow::Result<()> {
    let tianyan_config = tianyan::config::get_config()
        .map_err(|e| anyhow::anyhow!("加载配置失败：{}", e))?
        .clone();
    start_server(ServerConfig::default(), tianyan_config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.port, 3000);
        assert_eq!(config.host, "127.0.0.1");
    }

    #[test]
    fn test_server_config_with_port() {
        let config = ServerConfig::with_port(8080);
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "127.0.0.1");
    }

    #[test]
    fn test_server_config_new() {
        let config = ServerConfig::new("0.0.0.0", 9000);
        assert_eq!(config.port, 9000);
        assert_eq!(config.host, "0.0.0.0");
    }

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        assert_eq!(response.0.status, "ok");
    }
}
