//! 天演原生桌面客户端（GPUI）。
//!
//! 定位：与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌
//! axum server，走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前阶段：真实骨架——装配内核 → 起内嵌 server → 拉真实会话列表 → 渲染。
//! 尚未迁移：聊天流、设置页、工作区、托盘/通知/updater（见 README 的未验证清单）。
//!
//! 启动编排与 `tauri/src/lib.rs` 同源同形（配置加载 → 日志 → runtime → 端口 →
//! server → 健康检查）。**这两份将来要抽成共享 crate**，否则第三份副本迟早出现。

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{IndexPath, Root};
use gpui_kit::*;
use std::net::TcpListener;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;

/// 服务器监听地址（仅本机回环）。
const SERVER_HOST: &str = "127.0.0.1";
/// 首选端口（被占用时递增扫描）。
const PREFERRED_PORT: u16 = 3000;

/// 全局 tokio runtime：server 与 HTTP 客户端共用。GPUI 的 executor 是 smol 系，
/// 跨 runtime 的 `JoinHandle` 是可以 await 的（只要 tokio 在跑）。
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

/// 探测并绑定可用端口：探测与监听原子化，消除竞态窗口。
fn find_available_port(preferred: u16) -> Option<TcpListener> {
    for port in preferred..=preferred.saturating_add(100) {
        if let Ok(listener) = TcpListener::bind((SERVER_HOST, port)) {
            return Some(listener);
        }
    }
    None
}

/// 等 `/health` 就绪（最多 30 秒）。
async fn wait_for_server(port: u16) -> bool {
    let url = format!("http://{SERVER_HOST}:{port}/health");
    let client = reqwest::Client::new();
    for _ in 0..60 {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// 会话列表项（后端 `GET /api/v1/sessions` 的最小子集）
#[derive(Clone, Debug)]
struct SessionRow {
    title: String,
    id: String,
}

struct SessionsDelegate {
    items: Vec<SessionRow>,
}

impl ListDelegate for SessionsDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &gpui_kit::App) -> usize {
        self.items.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let row = self.items.get(ix.row)?;
        let title = if row.title.is_empty() {
            "(无标题)".to_string()
        } else {
            row.title.clone()
        };
        Some(
            ListItem::new(ix.row)
                .h(px(46.))
                .child(div().px_3().child(title)),
        )
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }
}

struct ClientApp {
    sessions: Entity<ListState<SessionsDelegate>>,
    status: String,
    port: u16,
    session_count: usize,
}

impl ClientApp {
    fn new(window: &mut Window, cx: &mut Context<Self>, port: u16) -> Self {
        let sessions = cx.new(|cx| {
            ListState::new(
                SessionsDelegate {
                    items: Vec::new(),
                },
                window,
                cx,
            )
        });

        let app = Self {
            sessions,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
        };
        app.load_sessions(cx);
        app
    }

    /// 拉真实会话列表（与 gui-vite 的 `GET /api/v1/sessions` 同一端点）
    fn load_sessions(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let sessions = self.sessions.clone();
        cx.spawn(async move |this, cx| {
            let fetched = runtime()
                .spawn(async move {
                    let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions");
                    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
                    let body: serde_json::Value =
                        resp.json().await.map_err(|e| e.to_string())?;
                    Ok::<serde_json::Value, String>(body)
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")));

            match fetched {
                Ok(body) => {
                    // 后端返回形如 {"sessions": [...]} 或直接数组，两种都容忍
                    let arr = body
                        .get("sessions")
                        .and_then(|v| v.as_array())
                        .or_else(|| body.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<SessionRow> = arr
                        .iter()
                        .map(|s| SessionRow {
                            id: s
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            title: s
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[会话] 从后端拉到 {n} 条");
                    cx.update(|app| {
                        sessions.update(app, |state, cx| {
                            state.delegate_mut().items = rows;
                            cx.notify();
                        });
                        this.update(app, |this, cx| {
                            this.session_count = n;
                            this.status = format!("已连接 127.0.0.1:{port} · 会话 {n} 条");
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[会话] 拉取失败: {e}");
                    cx.update(|app| {
                        this.update(app, |this, cx| {
                            this.status = format!("拉取会话失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }
}

impl Render for ClientApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                // 左：真实会话列表
                div()
                    .w(px(320.))
                    .h_full()
                    .border_r_1()
                    .child(List::new(&self.sessions)),
            )
            .child(
                // 右：状态区（后续替换为聊天流）
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_xl().child("天演 · 原生客户端"))
                    .child(div().child(self.status.clone()))
                    .child(
                        div()
                            .text_sm()
                            .child(format!("会话数：{}", self.session_count)),
                    ),
            )
    }
}

fn main() {
    // ===== 1. 装配：配置加载 + 注入内核槽位 + 日志（与 app/src/main.rs、tauri/src/lib.rs 同源）=====
    let config = tianyan_service_config::load_and_install();
    if let Err(e) = tianyan::common::logging::init_logging(&config.logging) {
        eprintln!("日志初始化失败：{e}");
    }
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // ===== 2. 端口：探测与绑定原子化 =====
    let listener = match find_available_port(PREFERRED_PORT) {
        Some(l) => l,
        None => {
            eprintln!("{PREFERRED_PORT} 起 100 个端口均被占用，退出");
            std::process::exit(1);
        }
    };
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .unwrap_or(PREFERRED_PORT);
    println!("[启动] 内嵌 server 端口 = {port}");

    // ===== 3. 后台起 server（完整装配：11 插槽，清单在 tianyan-assembly）=====
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    {
        let config = config.clone();
        runtime().spawn(async move {
            let listener = match listener.set_nonblocking(true) {
                Ok(()) => match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("转换监听 socket 失败：{e}");
                        return;
                    }
                },
                Err(e) => {
                    eprintln!("设置非阻塞失败：{e}");
                    return;
                }
            };
            if let Err(e) = tianyan_server::start_server_with_shutdown_signal_and_assembly(
                listener,
                config,
                shutdown_rx,
                tianyan_assembly::full(),
            )
            .await
            {
                eprintln!("server 退出：{e}");
            }
        });
    }

    // ===== 4. 等健康检查（启动期硬前置：server 起不来就没有 UI 可用）=====
    println!("[启动] 等待 server 就绪…");
    let ready = runtime().block_on(wait_for_server(port));
    if !ready {
        eprintln!("server 30 秒内未就绪，退出");
        std::process::exit(1);
    }
    println!("[启动] server 就绪");

    // ===== 5. GPUI 主循环 =====
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("天演".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| ClientApp::new(window, cx, port));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });

    // ===== 6. 窗口关闭 → 关停 server =====
    let _ = shutdown_tx.send(true);
    println!("[退出] 已发送关停信号");
}
