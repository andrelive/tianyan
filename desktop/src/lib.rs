//! 天演桌面端共享启动编排：内嵌 server 的**单实例发现与复用**。
//!
//! `tauri/`（Tauri + React）与 `gui-native/`（GPUI）是同一产品的两个桌面壳，
//! 各有自己的内嵌 axum server。此前两者的启动编排各写一遍
//! （`find_available_port` 与 `start_axum_server` 逐字两份），而**端口探测只能
//! 避免端口冲突，不能避免数据目录双写**：
//!
//! ```text
//! desktop 版先起 → 占 3000、跑定时任务、写 D:\tianyan
//! gui-native 后起 → 3000 被占 → 顺延 3001 → 也跑定时任务、也写 D:\tianyan   ← 双写
//! ```
//!
//! 实测就撞上过这个场景（3000 与 3001 同时有 server 在跑），后果是定时任务被
//! 两个进程各跑一遍、SQLite 出现并发写。本 crate 把「先看有没有可复用的 server，
//! 没有才自己起」收敛成**一处实现**，两个桌面壳共用：
//!
//! - [`discover_existing`]：扫描 `preferred..=preferred+100`，对每个 `/health`
//!   校验 `service == "tianyan"` **且** `data_dir` 与本进程配置一致；命中即复用。
//! - [`ensure_server`]：命中就返回 `reused = true`（本进程**不**启动 server），
//!   否则绑定端口 + 装配 + 启动 + 等就绪，返回 `reused = false`。
//!
//! ## 为什么要校验 `data_dir`
//!
//! 天演的数据目录是**可配置**的（ADR-023：配置固定、数据可搬）。一台机器上完全
//! 可能同时存在「主数据」与「测试数据」两个目录，各自跑着一个 server。只靠
//! `status=ok` 复用会把客户端连到**别人的数据**上——这是不可接受的静默错误，
//! 所以判据是「服务标识 + 数据目录」两条都要过，且空 `data_dir`（旧版 server
//! 不报该字段）一律判为不匹配。
//!
//! ## 装配边界（ADR-049）
//!
//! 本 crate 通过 `tianyan_assembly::full()` 取插件清单，**不直接认识任何插件 crate**，
//! 因此没有引入第二份装配清单；它只是「装配器的启动编排段」。

// 与全仓其余 crate 一致：测试里 `unwrap`/`expect` 是断言手段，不是生产代码的
// 错误处理方式。缺了这一行，`cargo clippy --all-targets -- -D warnings`
// （`scripts/test.ps1 lint` 与 CI 的同一道门）会把 9 处测试断言升成 error。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::watch;
use tracing::{error, info, warn};

/// 服务器监听地址（仅本机回环，不对外暴露）。
pub const SERVER_HOST: &str = "127.0.0.1";

/// 首选端口（被占用时由 [`find_available_port`] 动态选择替代端口）。
pub const PREFERRED_PORT: u16 = 3000;

/// 端口扫描跨度：与 [`find_available_port`] 的顺延区间**必须一致**。
///
/// 只看首选端口会漏判（实测：桌面版占 3000、gui-native 顺延到 3001，
/// 只探 3000 的实现会判定「无实例」并再起一个 server → 双写）。
pub const PORT_SCAN_SPAN: u16 = 100;

/// 内嵌 server 就绪等待上限。
const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// 单次 `/health` 探测超时。
///
/// 本地回环上未监听的端口会立即 `ECONNREFUSED`（不会吃满超时），所以扫描 101 个
/// 端口在正常情况下是毫秒级；这个上限只为兜住「端口被防火墙 DROP」这类异常。
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 单端口 TCP 探活超时（在 HTTP 之前先探一次：未监听端口会立刻 RST，
/// 避免每个端口都吃满 [`PROBE_TIMEOUT`]）。
const TCP_PROBE_TIMEOUT: Duration = Duration::from_millis(150);

/// 内嵌 server 的端点：端口 + 是否复用（`true` = 本进程没有启动 server）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEndpoint {
    /// 可用的服务端口。
    pub port: u16,
    /// `true` 表示复用了本机已有 server（本进程**没有**启动 server，退出时也**不该**关停它）。
    pub reused: bool,
}

/// `/health` 的最小反序列化形状（只取判定需要的字段，多出的字段忽略）。
#[derive(Debug, Deserialize)]
struct HealthProbe {
    /// 服务标识（`"tianyan"`）；旧版 server 无此字段 → 空串 → 不匹配。
    #[serde(default)]
    service: String,
    /// 服务使用的数据目录（`common::path::normalize_dir` 口径）；旧版 server 无此字段。
    #[serde(default)]
    data_dir: String,
}

/// 探测并绑定可用端口：优先首选端口，被占用时递增扫描（最多 [`PORT_SCAN_SPAN`] 个）。
///
/// 返回**已绑定**的 listener：探测与监听原子化，消除
/// "探测端口 → 释放 → 重新绑定"之间的竞态窗口。
///
/// 返回 `None` 表示 `preferred..=preferred+PORT_SCAN_SPAN` 全部被占用。
pub fn find_available_port(preferred: u16) -> Option<std::net::TcpListener> {
    for port in preferred..=preferred.saturating_add(PORT_SCAN_SPAN) {
        if let Ok(listener) = std::net::TcpListener::bind((SERVER_HOST, port)) {
            return Some(listener);
        }
    }
    None
}

/// 在给定端口上探测「是否已有同数据目录的天演 server」。
///
/// 命中返回端口号；未命中返回 `None`（含：端口空闲、端口上是别的服务、端口上是天演但
/// 数据目录不同）。三种「未命中」都不应当作错误——调用方据此决定「自己起一个」。
pub async fn discover_existing(data_dir: &Path, preferred: u16) -> Option<u16> {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            warn!("创建探测客户端失败，跳过复用检查：{e}");
            return None;
        }
    };

    // 快路径：绝大多数情况下实例就在首选端口上，一次探测即可（毫秒级）
    if probe_port(&client, preferred, data_dir).await {
        return Some(preferred);
    }

    // 慢路径：**并发**扫其余端口。
    //
    // 串行扫 101 个端口在「端口被防火墙 DROP」时会退化成 101 × 500ms ≈ 50 秒
    // （实测：`cargo test -p tianyan-desktop` 曾因这一个函数耗时 51 秒）。启动时
    // 白等 50 秒不可接受，故并发，最坏耗时 = 单端口超时上限。
    let mut set = tokio::task::JoinSet::new();
    for port in (preferred.saturating_add(1))..=preferred.saturating_add(PORT_SCAN_SPAN) {
        let client = client.clone();
        let data_dir = data_dir.to_path_buf();
        set.spawn(async move {
            if probe_port(&client, port, &data_dir).await {
                Some(port)
            } else {
                None
            }
        });
    }
    let mut hits: Vec<u16> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(port)) = joined {
            hits.push(port);
        }
    }
    // 多个同目录实例并存（历史遗留 / 手工起的）时取最小端口，结果稳定可复现
    hits.sort_unstable();
    hits.first().copied()
}

/// 单端口探测：先 TCP 探活，再校验 `/health` 的服务标识与数据目录。
async fn probe_port(client: &reqwest::Client, port: u16, data_dir: &Path) -> bool {
    let addr = format!("{SERVER_HOST}:{port}");
    let tcp = tokio::time::timeout(TCP_PROBE_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await;
    if !matches!(tcp, Ok(Ok(_))) {
        return false;
    }
    let url = format!("http://{SERVER_HOST}:{port}/health");
    let Ok(resp) = client.get(&url).send().await else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    let Ok(probe) = resp.json::<HealthProbe>().await else {
        return false;
    };
    if probe.service != "tianyan" {
        // 端口上活着别的本地服务（IDE / dev server 等）：静默跳过
        return false;
    }
    if !tianyan::common::path::same_dir(&probe.data_dir, data_dir) {
        // 同一台机器上可能有多个数据目录各跑一个天演（主数据 / 测试数据）——
        // 连过去会读写别人的数据，必须跳过并明确告警。
        warn!(
            port,
            remote = %probe.data_dir,
            local = %data_dir.display(),
            "发现天演 server，但数据目录与本进程不同，跳过复用"
        );
        return false;
    }
    true
}

/// 启动或复用内嵌 server：**本机已有同一数据目录的实例就直接连它**。
///
/// - `reused = true`：`shutdown_rx` 立即失效（本进程没起 server，退出时不要关它）。
/// - `reused = false`：server 已在本进程的 tokio runtime 上 spawn，且 `/health` 已就绪。
///
/// 失败时返回人类可读的错误（端口耗尽 / 监听地址异常 / 30 秒未就绪），
/// 由调用方决定是弹窗退出（桌面端）还是打印退出（CLI）。
pub async fn ensure_server(
    config: tianyan::config::TianyanConfig,
    preferred: u16,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<ServerEndpoint, String> {
    let data_dir = config.storage.data_dir.clone();
    if let Some(port) = discover_existing(&data_dir, preferred).await {
        info!(
            port,
            data_dir = %data_dir.display(),
            "本机已有天演 server 服务同一数据目录，复用之（本进程不启动第二个 server）"
        );
        return Ok(ServerEndpoint { port, reused: true });
    }

    let listener = find_available_port(preferred).ok_or_else(|| {
        format!("{preferred} 起 {PORT_SCAN_SPAN} 个端口均被占用，请释放部分端口后重试")
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取监听地址失败：{e}"))?
        .port();
    info!(port, "未发现可复用的 server，本进程启动内嵌 server");

    let spawned = tokio::spawn(async move {
        start_axum_server(config, listener, shutdown_rx).await;
    });

    // 就绪等待失败时把刚 spawn 的 server 收掉：否则调用方拿不到可用端口，
    // 却留下一个「在跑但没人用」的 server（退出时还会因为优雅关停卡住）。
    if let Err(e) = wait_ready(port).await {
        spawned.abort();
        return Err(e);
    }
    Ok(ServerEndpoint {
        port,
        reused: false,
    })
}

/// 等待指定端口的 `/health` 就绪（[`HEALTH_TIMEOUT`] 上限）。
pub async fn wait_ready(port: u16) -> Result<(), String> {
    wait_ready_within(port, HEALTH_TIMEOUT).await
}

/// 就绪等待的实现（超时可注入，供单测用短超时）。
async fn wait_ready_within(port: u16, timeout: Duration) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| format!("创建探测客户端失败：{e}"))?;
    let url = format!("http://{SERVER_HOST}:{port}/health");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "server 在 {} 秒内未就绪（{url}）",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// 启动 Axum 服务（监听调用方传入的**已绑定** listener）。
///
/// # Arguments
///
/// * `tianyan_config` - Tianyan core 配置
/// * `listener` - 已绑定的监听 socket（由 [`find_available_port`] 提供）
/// * `shutdown_signal` - 外部关闭信号：发送 `true` 触发与 Ctrl+C 相同的优雅关停
pub async fn start_axum_server(
    tianyan_config: tianyan::config::TianyanConfig,
    listener: std::net::TcpListener,
    shutdown_signal: watch::Receiver<bool>,
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

    // 完整装配（ADR-049）：清单在 `tianyan-assembly`，与独立 bin / 两个桌面壳共用
    // 同一份实现（跨插件顺序纪律也只有那里一份）。
    let assembly = tianyan_assembly::full();
    if let Err(e) = tianyan_server::start_server_with_shutdown_signal_and_assembly(
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

    /// 一个几乎肯定空闲的端口段（避开 3000..3100：本机常驻的桌面版/开发服务都在那）。
    const FREE_PROBE_BASE: u16 = 39900;

    #[test]
    fn test_find_available_port_holds_the_port() {
        let preferred = FREE_PROBE_BASE + 1;
        let Ok(occupied) = std::net::TcpListener::bind((SERVER_HOST, preferred)) else {
            return; // 端口被并发测试占用，跳过
        };
        let found = find_available_port(preferred).expect("应找到可用端口");
        let port = found.local_addr().expect("listener 应已绑定").port();
        assert_ne!(port, preferred, "首选端口被占用时应顺延");
        // 已选中的端口不应能被再次绑定（探测与监听原子化）
        assert!(std::net::TcpListener::bind((SERVER_HOST, port)).is_err());
        drop(occupied);
    }

    #[test]
    fn test_find_available_port_prefers_first_free() {
        let preferred = FREE_PROBE_BASE + 20;
        if std::net::TcpListener::bind((SERVER_HOST, preferred)).is_err() {
            return; // 首选端口恰好被占，跳过
        }
        let found = find_available_port(preferred).expect("应找到可用端口");
        assert_eq!(
            found.local_addr().expect("listener 应已绑定").port(),
            preferred
        );
    }

    #[tokio::test]
    async fn test_discover_existing_finds_nothing_when_free() {
        // 无人监听的端口段：必须返回 None（而不是阻塞到超时或错报）
        assert_eq!(
            discover_existing(&std::env::temp_dir(), FREE_PROBE_BASE + 40).await,
            None
        );
    }

    #[tokio::test]
    async fn test_discover_existing_scan_is_bounded() {
        // 扫描耗时上界：串行实现曾因「端口被 DROP」退化到 101 × 500ms ≈ 50 秒
        // （单测总耗时实测 51s）。并发 + TCP 预探后应稳定在秒级，这里钉住。
        let start = std::time::Instant::now();
        assert_eq!(
            discover_existing(&std::env::temp_dir(), FREE_PROBE_BASE + 200).await,
            None
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "扫描 101 个端口应在秒级完成，实际 {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_wait_ready_times_out_fast_on_dead_port() {
        let err = wait_ready_within(FREE_PROBE_BASE + 60, Duration::from_millis(400))
            .await
            .expect_err("无人监听时不得判为就绪");
        assert!(err.contains("未就绪"), "错误应说明未就绪，实际：{err}");
    }

    #[tokio::test]
    async fn test_discover_existing_ignores_foreign_service() {
        // 假服务：端口活着，但 /health 不是天演的形状 → 不得复用
        let listener = match std::net::TcpListener::bind((SERVER_HOST, FREE_PROBE_BASE + 80)) {
            Ok(l) => l,
            Err(_) => return,
        };
        let port = listener.local_addr().expect("listener 应已绑定").port();
        listener.set_nonblocking(true).expect("设为非阻塞");
        let task = tokio::spawn(async move {
            let l = tokio::net::TcpListener::from_std(listener).expect("转换 listener");
            if let Ok((mut stream, _)) = l.accept().await {
                use tokio::io::AsyncWriteExt as _;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}")
                    .await;
            }
        });
        let found = discover_existing(&std::env::temp_dir(), port).await;
        task.abort();
        assert_eq!(found, None, "非天演服务不得被判为可复用");
    }
}
