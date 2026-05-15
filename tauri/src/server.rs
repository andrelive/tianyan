//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.

use tianyan_server::{start_server, ServerConfig};
use tracing::{error, info};

const SERVER_HOST: &str = "127.0.0.1";
const SERVER_PORT: u16 = 3000;

/// 启动 Axum 服务
///
/// 此函数在后台线程中启动 Axum HTTP 服务器，
/// 服务器将在 127.0.0.1:3000 上监听请求。
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
pub async fn start_axum_server(tianyan_config: tianyan::config::TianyanConfig) {
    let config = ServerConfig::new(SERVER_HOST, SERVER_PORT);
    info!("Starting Axum server on {}:{}", SERVER_HOST, SERVER_PORT);

    if let Err(e) = start_server(config, tianyan_config).await {
        error!("Server error: {}", e);
    }
}

/// 启动 Axum 服务（阻塞版本）
///
/// 此函数用于在单独的任务中启动服务器
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
pub fn start_axum_server_blocking(tianyan_config: tianyan::config::TianyanConfig) {
    let rt = tokio::runtime::Handle::current();
    rt.spawn(async move {
        start_axum_server(tianyan_config).await;
    });
}
