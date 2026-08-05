//! Tianyan Tauri Desktop Application
//!
//! This crate provides a Tauri-based desktop application that:
//! - Starts an Axum HTTP server in a background thread
//! - Waits for the server to be ready via health checks
//! - Launches a Tauri window to display the Yew-based GUI

pub mod server;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub use server::start_axum_server;

use server::{find_available_port, PREFERRED_PORT, SERVER_HOST};

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// 检测首选端口上是否已有 Tianyan 实例在服务（健康检查可达）。
///
/// 在启动内嵌服务器之前调用：第二实例直接退出，避免短暂打开共享数据文件。
fn existing_instance_running(rt: &tokio::runtime::Runtime) -> bool {
    instance_running_on(rt, PREFERRED_PORT)
}

/// 探测指定端口上的 /health 是否可达。
fn instance_running_on(rt: &tokio::runtime::Runtime, port: u16) -> bool {
    let url = format!("http://{}:{}/health", SERVER_HOST, port);
    rt.block_on(async {
        tokio::time::timeout(HEALTH_CHECK_INTERVAL * 3, async {
            reqwest::Client::new()
                .get(&url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    })
}

/// 服务器重启退避：初始间隔（首次重启等待时间）。
const SERVER_RESTART_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
/// 服务器重启退避：最大间隔（崩溃风暴保护，指数增长封顶）。
const SERVER_RESTART_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// 服务器监督循环：监控内嵌服务器任务，异常结束后自动重启。
///
/// 重启流程：退避等待 → 重新绑定端口（原端口已释放自然拿回）→
/// 创建新关闭信号通道（watch 版本变化后旧 receiver 不可复用）→
/// 启动新任务 → 等待就绪 → 重新注入 API base 端口（前端动态读取，自动生效）。
///
/// 退出流程（[`crate::run`] 的 ExitRequested 钩子置位 `exiting` 后）不再重启，
/// 并正常结束以便退出钩子 join。
async fn run_server_supervisor(
    server_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    exiting: Arc<AtomicBool>,
    shutdown_tx: Arc<Mutex<Option<tokio::sync::watch::Sender<bool>>>>,
    app_handle_rx: tokio::sync::oneshot::Receiver<tauri::AppHandle>,
    config: tianyan::config::TianyanConfig,
) {
    // 等待 setup 完成（AppHandle 就绪后才能注入端口）
    let app_handle = match app_handle_rx.await {
        Ok(handle) => handle,
        Err(_) => {
            warn!("AppHandle 通道关闭，监督循环退出");
            return;
        }
    };

    let mut backoff = SERVER_RESTART_BACKOFF_INITIAL;

    loop {
        // 取出当前任务句柄并等待其结束
        let handle = match server_task.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(handle) = handle else {
            info!("服务器任务句柄已不存在，监督循环退出");
            return;
        };
        if let Err(e) = handle.await {
            error!("内嵌服务器任务异常结束：{}", e);
        } else {
            info!("内嵌服务器任务已结束");
        }

        // 退出流程中（用户关闭应用）不再重启
        if exiting.load(Ordering::SeqCst) {
            info!("应用退出流程中，监督循环退出");
            return;
        }

        // 退避后重启（崩溃风暴保护：1s → 2s → 4s → ... → 30s 封顶）
        warn!("内嵌服务器已停止，{:.1}s 后尝试重启", backoff.as_secs_f32());
        sleep(backoff).await;
        backoff = next_backoff(backoff, SERVER_RESTART_BACKOFF_MAX);

        // 退出流程可能在退避期间触发
        if exiting.load(Ordering::SeqCst) {
            return;
        }

        // 重新绑定端口：原端口已释放（任务结束），自然拿回；被外部抢占则顺延
        let Some(listener) = find_available_port(PREFERRED_PORT) else {
            error!("重启失败：未找到可用端口，监督循环退出");
            return;
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(e) => {
                error!("重启失败：读取监听地址失败：{}", e);
                continue;
            }
        };

        // 新关闭信号通道（watch 版本已变化，不可复用旧 receiver）
        let (tx, rx) = tokio::sync::watch::channel(false);
        *shutdown_tx.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);

        let config_clone = config.clone();
        let handle = tokio::spawn(async move {
            info!("Restarting Axum server on port {}...", port);
            start_axum_server(config_clone, listener, rx).await;
        });
        *server_task.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);

        // 等待重启就绪并重新注入端口（前端 getApiBase() 动态读取，自动生效）
        match wait_for_server_ready(port).await {
            Ok(()) => {
                backoff = SERVER_RESTART_BACKOFF_INITIAL;
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.eval(format!(
                        "window.__TIANYAN_API_BASE__ = 'http://{}:{}';",
                        SERVER_HOST, port
                    ));
                }
                info!("内嵌服务器重启成功，端口 {}", port);
            }
            Err(e) => {
                error!("重启后服务器未就绪（{}），将进入下一轮重试", e);
                // 任务可能仍在运行：发关闭信号强制退出，
                // 下一轮循环 take 该句柄并等待其结束
                if let Some(tx) = shutdown_tx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone()
                {
                    let _ = tx.send(true);
                }
            }
        }
    }
}

/// 重启退避计算：指数增长，封顶 `max`。
fn next_backoff(current: Duration, max: Duration) -> Duration {
    let doubled = current * 2;
    if doubled >= max {
        max
    } else {
        doubled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("创建测试 runtime")
    }

    #[test]
    fn test_instance_running_on_detects_live_server() {
        // 起一个真实健康检查服务，探测应返回 true
        let rt = test_runtime();
        let listener = std::net::TcpListener::bind((SERVER_HOST, 0)).expect("绑定临时端口");
        let port = listener.local_addr().expect("读取端口").port();
        listener.set_nonblocking(true).expect("非阻塞");

        // 启动测试服务并等待就绪（同一 block_on 内：from_std/spawn 需要 runtime 上下文，
        // 轮询驱动 server 任务；最多 2s）
        let ready = rt.block_on(async {
            let tokio_listener =
                tokio::net::TcpListener::from_std(listener).expect("转 tokio listener");
            let app = axum::Router::new().route(
                "/health",
                axum::routing::get(|| async {
                    axum::response::Json(serde_json::json!({"status": "ok"}))
                }),
            );
            rt.spawn(async move {
                let _ = axum::serve(tokio_listener, app).await;
            });
            for _ in 0..20 {
                if reqwest::Client::new()
                    .get(format!("http://{}:{}/health", SERVER_HOST, port))
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false)
                {
                    return true;
                }
                tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
            }
            false
        });
        assert!(ready, "测试服务应就绪");
        assert!(instance_running_on(&rt, port), "服务在线时应探测到实例");
    }

    #[test]
    fn test_instance_running_on_returns_false_for_idle_port() {
        // 未启动服务的随机端口，探测应返回 false（无并发占用，结果确定）
        let rt = test_runtime();
        let probe = std::net::TcpListener::bind((SERVER_HOST, 0)).expect("绑定临时端口");
        let port = probe.local_addr().expect("读取端口").port();
        drop(probe);
        assert!(!instance_running_on(&rt, port), "空闲端口不应探测到实例");
    }

    #[test]
    fn test_next_backoff_doubles_exponentially() {
        assert_eq!(
            next_backoff(Duration::from_secs(1), Duration::from_secs(30)),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(4), Duration::from_secs(30)),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn test_next_backoff_caps_at_max() {
        assert_eq!(
            next_backoff(Duration::from_secs(16), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }
}

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
            .with_writer(Arc::new(file))
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
pub async fn wait_for_server_ready(port: u16) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("http://{}:{}/health", SERVER_HOST, port);

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

    // 应用层单实例检查（先于内嵌服务器启动）：
    // 首选端口健康检查可达 = 已有实例在服务 → 直接退出，
    // 避免第二实例短暂启动 server 并打开共享 SQLite/LanceDB 数据文件。
    // 与 single-instance 插件构成双保险（插件兜底此处检查与启动之间的竞态窗口）。
    if existing_instance_running(&rt) {
        info!(
            "检测到已有实例运行（端口 {} 服务可达），退出当前进程",
            PREFERRED_PORT
        );
        return;
    }

    // 选择并绑定监听端口：首选 3000，被占用时动态递增（避免与其他本地服务冲突）。
    // listener 提前绑定，探测与监听原子化，消除竞态窗口
    let listener = match find_available_port(PREFERRED_PORT) {
        Some(l) => l,
        None => {
            error!(
                "未找到可用端口（{} 起 100 个端口均被占用），退出",
                PREFERRED_PORT
            );
            std::process::exit(1);
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            error!("读取监听端口失败：{}", e);
            std::process::exit(1);
        }
    };
    if port != PREFERRED_PORT {
        info!("端口 {} 已被占用，动态选择端口 {}", PREFERRED_PORT, port);
    }

    // ===== 服务器监督基础设施 =====
    // 当前 server 任务句柄（监督循环与退出钩子共享）
    let server_task = Arc::new(Mutex::new(None::<tokio::task::JoinHandle<()>>));
    // 退出流程标志：置位后监督循环不再重启服务器
    let exiting = Arc::new(AtomicBool::new(false));
    // 当前活跃的关闭信号 sender（跨重启替换：watch 版本变化后旧 receiver 不可复用）
    let shutdown_tx = Arc::new(Mutex::new(None::<tokio::sync::watch::Sender<bool>>));
    // AppHandle 通道：setup 完成后交给监督循环（重启时注入端口用）
    let (app_handle_tx, app_handle_rx) = tokio::sync::oneshot::channel();

    // 启动初始 server 任务（listener 已绑定，见上方 find_available_port）
    {
        let (tx, rx) = tokio::sync::watch::channel(false);
        *shutdown_tx.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
        let config_clone = tianyan_config.clone();
        let handle = rt.spawn(async move {
            info!(
                "Starting Axum server in background task on port {}...",
                port
            );
            start_axum_server(config_clone, listener, rx).await;
        });
        *server_task.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    }

    // 等待服务器启动完成（初始启动失败是环境问题，直接退出）
    info!("Waiting for server to be ready...");
    rt.block_on(async {
        if let Err(e) = wait_for_server_ready(port).await {
            error!("Server failed to start: {}", e);
            std::process::exit(1);
        }
        info!("Axum server is ready, proceeding to start Tauri...");
    });

    // 监督循环：server 任务异常结束后自动重启（退避重试 + 重新注入端口）
    let supervisor_task = rt.spawn(run_server_supervisor(
        server_task.clone(),
        exiting.clone(),
        shutdown_tx.clone(),
        app_handle_rx,
        tianyan_config,
    ));

    info!("Initializing Tauri application...");

    // 构建并运行 Tauri 应用
    // Tauri 会自动加载 frontendDist 中配置的静态文件 (gui/dist)
    // 前端通过 HTTP 调用 Axum 后端 API
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        // 单实例保护：双开时第二实例的启动参数转发到第一实例并聚焦主窗口，
        // 避免两个进程共享同一 SQLite/LanceDB 数据文件
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            info!("检测到已有实例运行，聚焦主窗口");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .setup(move |app| {
            info!("Tauri setup completed, frontend loaded from gui/dist");
            // 注入实际监听端口：前端 getApiBase() 优先读取该变量
            // （动态端口时前端无法从默认值得知，必须显式注入）
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval(format!(
                    "window.__TIANYAN_API_BASE__ = 'http://{}:{}';",
                    SERVER_HOST, port
                ));
            }
            // 仅在 debug 构建时打开开发者工具
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            // 将 AppHandle 交给监督循环（服务器重启后注入新端口用）
            let _ = app_handle_tx.send(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(e) => {
            error!("Tauri application build error: {}", e);
            std::process::exit(1);
        }
    };

    // 退出防重入：app.exit(0) 可能再次触发 ExitRequested
    let exiting_cb = exiting.clone();
    let shutdown_tx_cb = shutdown_tx;
    let mut supervisor_task = Some(supervisor_task);

    app.run(move |app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            // 第二次进入（app.exit 重入）：不再拦截，让退出流程继续
            if exiting_cb.swap(true, Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            info!("收到退出请求，开始优雅关闭内嵌服务器...");

            // 触发 server 的优雅关停链（停止接收连接 → 停调度器 → 等 pending 任务，30s 超时）
            if let Some(tx) = shutdown_tx_cb
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
            {
                if tx.send(true).is_err() {
                    warn!("服务器可能已不在运行（关闭信号发送失败）");
                }
            }

            // 监督循环在服务器任务结束后（检查退出标志后）退出
            if let Some(task) = supervisor_task.take() {
                if let Err(e) = rt.block_on(task) {
                    error!("服务器监督循环异常：{}", e);
                }
            }
            info!("内嵌服务器已优雅关闭，退出应用");
            app_handle.exit(0);
        }
    });
}
