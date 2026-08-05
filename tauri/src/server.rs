//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.

use tianyan_server::{start_server, ServerConfig};
use tracing::{error, info};

/// 服务器监听地址（仅本机回环，不对外暴露）。
pub const SERVER_HOST: &str = "127.0.0.1";
/// 首选端口（被占用时由 [`find_available_port`] 动态选择替代端口）。
pub const PREFERRED_PORT: u16 = 3000;

/// 探测可用端口：优先使用首选端口，被占用时递增扫描（最多 100 个）。
///
/// 桌面端启动时调用：避免与其他本地服务（如其他应用占用 3000）
/// 冲突导致启动失败。
pub fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..=preferred.saturating_add(100) {
        if std::net::TcpListener::bind((SERVER_HOST, port)).is_ok() {
            return port;
        }
    }
    preferred
}

/// 启动 Axum 服务
///
/// 此函数在后台线程中启动 Axum HTTP 服务器，
/// 服务器将在 `127.0.0.1:{port}` 上监听请求。
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
/// * `port` - 实际监听端口（由调用方通过 [`find_available_port`] 选定）
pub async fn start_axum_server(tianyan_config: tianyan::config::TianyanConfig, port: u16) {
    let config = ServerConfig::new(SERVER_HOST, port);
    info!("Starting Axum server on {}:{}", SERVER_HOST, port);

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
        start_axum_server(tianyan_config, PREFERRED_PORT).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_available_port_preferred_when_free() {
        // 首选端口空闲时（CI 环境无占用）应返回首选端口
        if std::net::TcpListener::bind((SERVER_HOST, PREFERRED_PORT)).is_ok() {
            assert_eq!(find_available_port(PREFERRED_PORT), PREFERRED_PORT);
        }
    }

    #[test]
    fn test_find_available_port_skips_occupied() {
        // 首选端口被占用时应选择其他可用端口
        let Ok(listener) = std::net::TcpListener::bind((SERVER_HOST, PREFERRED_PORT)) else {
            return; // 端口被并发测试占用，跳过
        };
        let port = find_available_port(PREFERRED_PORT);
        assert_ne!(port, PREFERRED_PORT, "首选端口被占用时应选择其他端口");
        drop(listener);
    }
}
