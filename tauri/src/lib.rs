//! Tianyan Tauri Desktop Application
//!
//! This crate provides a Tauri-based desktop application that:
//! - Starts an Axum HTTP server in a background thread
//! - Waits for the server to be ready via health checks
//! - Launches a Tauri window to display the Yew-based GUI

pub mod server;

use std::time::Duration;
use tauri::Manager;
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub use server::start_axum_server;

const SERVER_HOST: &str = "127.0.0.1";
const SERVER_PORT: u16 = 3000;
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// 初始化日志系统 - 同时输出到文件和控制台（如果有）
fn init_logging() {
    // 创建日志目录
    let log_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.tianyan.app")
        .join("logs");

    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Failed to create log directory: {}", e);
    }

    let log_file = log_dir.join(format!(
        "tianyan_{}.log",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));

    // 尝试创建文件日志
    let file_appender = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        Ok(file) => {
            eprintln!("Logging to: {:?}", log_file);
            Some(file)
        }
        Err(e) => {
            eprintln!("Failed to create log file {:?}: {}", log_file, e);
            None
        }
    };

    // 设置日志格式和级别
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tianyan=debug,tauri=debug"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_file(true);

    if let Some(file) = file_appender {
        let fmt_layer_file = tracing_subscriber::fmt::layer()
            .with_writer(std::sync::Arc::new(file))
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_line_number(true)
            .with_file(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(fmt_layer_file)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
    }

    info!("=== Tianyan Application Started ===");
    info!("Log file: {:?}", log_file);
}

/// 等待服务就绪（健康检查）
pub async fn wait_for_server_ready() -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("http://{}:{}/health", SERVER_HOST, SERVER_PORT);

    info!("Waiting for server at {}...", health_url);
    let start_time = std::time::Instant::now();

    while start_time.elapsed() < HEALTH_CHECK_TIMEOUT {
        match client
            .get(&health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    info!("Server is ready at {}", health_url);
                    return Ok(());
                } else {
                    warn!("Health check returned status: {}", response.status());
                }
            }
            Err(e) => {
                warn!("Health check failed ({}), retrying...", e);
            }
        }

        sleep(HEALTH_CHECK_INTERVAL).await;
    }

    Err(anyhow::anyhow!(
        "Server failed to become ready within {:?}",
        HEALTH_CHECK_TIMEOUT
    ))
}

/// 运行 Tauri 应用程序
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志（输出到文件）
    init_logging();

    info!("Starting Tianyan Tauri application...");

    // 检查配置状态
    let configured = tianyan::config::TianyanConfig::config_exists();
    info!("Configuration status: configured={}", configured);

    // 尝试加载配置，如果失败则使用默认配置
    let tianyan_config = match tianyan::config::TianyanConfig::load() {
        Ok(config) => {
            info!("Tianyan configuration loaded successfully");
            config
        }
        Err(e) => {
            warn!("Failed to load configuration: {}. Using default config.", e);
            // 使用默认配置，让前端显示配置向导
            tianyan::config::TianyanConfig::default()
        }
    };

    // 创建 Tokio runtime 用于后台服务
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            error!("Failed to create Tokio runtime: {}", e);
            std::process::exit(1);
        }
    };

    // 启动 Axum 服务在后台线程，传入配置
    // 即使配置无效，也启动服务以支持配置向导 API
    let config_clone = tianyan_config.clone();
    rt.spawn(async move {
        info!("Starting Axum server in background task...");
        start_axum_server(config_clone).await;
    });

    // 等待服务器启动完成
    info!("Waiting for server to be ready...");
    rt.block_on(async {
        if let Err(e) = wait_for_server_ready().await {
            error!("Server failed to start: {}", e);
            std::process::exit(1);
        }
        info!("Axum server is ready, proceeding to start Tauri...");
    });

    info!("Initializing Tauri application...");

    // 构建并运行 Tauri 应用
    // Tauri 会自动加载 frontendDist 中配置的静态文件 (gui/dist)
    // 前端通过 HTTP 调用 Axum 后端 API
    if let Err(e) = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|_app| {
            info!("Tauri setup completed, frontend loaded from gui/dist");
            // 仅在 debug 构建时打开开发者工具
            #[cfg(debug_assertions)]
            if let Some(window) = _app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
    {
        error!("Tauri application error: {}", e);
        std::process::exit(1);
    }
}
