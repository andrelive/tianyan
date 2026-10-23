//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.
//!
//! 装配清单（11 个工厂 + `full()` / `policies()` 两份清单，ADR-049 期 4）的唯一实现
//! 在 `tianyan-assembly`：本模块只负责「绑定端口 → 转换 listener → 启动 server」。
//! 改造前本文件与 `app/src/main.rs` 各写了一遍那 11 个工厂（逐字同形，靠注释
//! 「与 `app/src/main.rs::xxx_factory` 同形同源」互相提醒）——两份副本的漂移是
//! **静默的**：插槽签名加一个参数时先改的那份能编、后改的报错，而跨插件顺序纪律
//! （工具 fs→code→todo/goal→web、任务 evolution 排第 2）在某份被单独调整时不会
//! 编译失败，只会让桌面端聊天前缀缓存命中率悄悄下降。

use tianyan_server::start_server_with_shutdown_signal_and_assembly;
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

    // 完整装配（ADR-049 期 2 序 3/4/5a/6、期 3-T1/T2/T3/T5、期 4）：
    // 工具清单 + 向量存储 + 结构化存储 + 内置任务 + 审批门控 + 上下文策略 + 两个评审插槽
    // + 聊天/视觉 provider + 嵌入 provider + 工具管线——清单在 `tianyan-assembly`，
    // 与独立 bin 共用同一份实现（顺序纪律也只有那里一份）。
    let assembly = tianyan_assembly::full();
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
