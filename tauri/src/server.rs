//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.

use tianyan_server::start_server_with_listener;
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

/// 启动 Axum 服务
///
/// 此函数在后台任务中启动 Axum HTTP 服务器，
/// 服务器监听调用方传入的（已绑定）listener。
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
/// * `listener` - 已绑定的监听 socket（由 [`find_available_port`] 提供）
pub async fn start_axum_server(
    tianyan_config: tianyan::config::TianyanConfig,
    listener: std::net::TcpListener,
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

    if let Err(e) = start_server_with_listener(listener, tianyan_config).await {
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
        let Some(listener) = find_available_port(PREFERRED_PORT) else {
            error!("未找到可用端口（{} 起 100 个端口均被占用）", PREFERRED_PORT);
            return;
        };
        start_axum_server(tianyan_config, listener).await;
    });
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
