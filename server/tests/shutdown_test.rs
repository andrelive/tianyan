//! 优雅关停集成测试 — 桌面端退出场景：外部关闭信号应触发完整关停链
//!
//! 覆盖 `start_server_with_shutdown_signal`：Tauri 退出时通过 watch channel
//! 发送关闭信号，服务器应停止接收连接、停止调度器、等待 pending 任务后正常返回。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::factory::test_tianyan_config_with_data_dir;
use tempfile::tempdir;

#[tokio::test]
async fn external_shutdown_signal_triggers_graceful_shutdown() {
    // 独立临时数据目录：避免与其他测试的 SQLite/LanceDB 数据互相污染
    let data_dir = tempdir().expect("创建临时数据目录");
    let config = test_tianyan_config_with_data_dir(data_dir.path().to_path_buf());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定临时端口");
    let port = listener.local_addr().expect("读取端口").port();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let handle = tokio::spawn(tianyan_server::start_server_with_shutdown_signal(
        listener,
        config,
        shutdown_rx,
    ));

    // 轮询 /health 直到服务器就绪（create_app + 调度器装配耗时不定）
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{}/health", port);
    let mut ready = false;
    for _ in 0..50 {
        if client
            .get(&health_url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "服务器应在 5 秒内就绪");

    // 发送外部关闭信号（模拟 Tauri 退出）
    shutdown_tx.send(true).expect("发送关闭信号");

    // 服务器应在超时前完成优雅关停并正常返回
    let result = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("优雅关停超时")
        .expect("服务器任务 join 失败");
    assert!(
        result.is_ok(),
        "外部关闭信号应触发正常返回，实际: {result:?}"
    );
}
