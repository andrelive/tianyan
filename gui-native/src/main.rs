//! 天演原生桌面客户端（GPUI）。
//!
//! 与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌 axum server，
//! 走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前进度：装配内核 → 起内嵌 server → 拉会话列表 → **点会话看消息**。
//! 未完成：Markdown 渲染消息体、SSE 流式发送、设置页/workspace、托盘/通知/updater。

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{IndexPath, Root};
use gpui_kit::*;
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

const SERVER_HOST: &str = "127.0.0.1";
const PREFERRED_PORT: u16 = 3000;

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

fn find_available_port(preferred: u16) -> Option<TcpListener> {
    for port in preferred..=preferred.saturating_add(100) {
        if let Ok(listener) = TcpListener::bind((SERVER_HOST, port)) {
            return Some(listener);
        }
    }
    None
}

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

/// 在 tokio runtime 上取一段 JSON（GPUI 的 executor 是 smol 系，跨 runtime await
/// `JoinHandle` 是安全的）。
async fn fetch_json(url: String) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
            resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

// ===== 会话列表 =====

#[derive(Clone, Debug)]
struct SessionRow {
    id: String,
    title: String,
}

type AppRef = Arc<Mutex<Option<WeakEntity<ClientApp>>>>;

struct SessionsDelegate {
    items: Vec<SessionRow>,
    app: AppRef,
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
        let row = self.items.get(ix.row)?.clone();
        let title = if row.title.is_empty() {
            "(无标题)".to_string()
        } else {
            row.title.clone()
        };
        let app = self.app.clone();
        let id = row.id.clone();
        Some(
            ListItem::new(ix.row)
                .h(px(46.))
                .child(div().px_3().child(title))
                .on_click(move |_, _window, cx| {
                    let weak = app.lock().ok().and_then(|g| g.clone());
                    if let Some(w) = weak {
                        w.update(cx, |this, cx| this.load_messages(id.clone(), cx));
                    }
                }),
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

// ===== 消息列表 =====

#[derive(Clone, Debug)]
struct MsgRow {
    role: String,
    text: String,
}

struct MessagesDelegate {
    items: Vec<MsgRow>,
}

impl ListDelegate for MessagesDelegate {
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
        let m = self.items.get(ix.row)?.clone();
        let (tag, tag_color) = match m.role.as_str() {
            "user" => ("你", rgb(0x7aa2f7)),
            "assistant" => ("天演", rgb(0x9ece6a)),
            "tool" => ("工具", rgb(0xe0af68)),
            _ => ("系统", rgb(0x888888)),
        };
        Some(
            ListItem::new(ix.row).h(px(24.)).child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(tag_color).child(tag))
                    .child(div().child(m.text)),
            ),
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

/// 从消息 JSON 的 segments 里拼接正文（ADR-002：正文单一事实源是 segments，
/// 不是 content——后者只在请求方向携带）
fn segments_text(msg: &serde_json::Value) -> String {
    let Some(arr) = msg.get("segments").and_then(|v| v.as_array()) else {
        return msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
    };
    let mut out = String::new();
    for seg in arr {
        let ty = seg.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or_default();
        match ty {
            "text" => {
                out.push_str(text);
                out.push('\n');
            }
            "thinking" | "reasoning" => {
                if !text.is_empty() {
                    out.push_str("[思考] ");
                    out.push_str(text);
                    out.push('\n');
                }
            }
            "tool" => {
                let name = seg
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                out.push_str(&format!("[调用工具 {name}]\n"));
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push_str("(空消息)");
    }
    out
}

struct ClientApp {
    sessions: Entity<ListState<SessionsDelegate>>,
    messages: Entity<ListState<MessagesDelegate>>,
    status: String,
    port: u16,
    session_count: usize,
    current_title: String,
}

impl ClientApp {
    fn new(window: &mut Window, cx: &mut Context<Self>, port: u16) -> Self {
        let app_ref: AppRef = Arc::new(Mutex::new(None));
        let sessions = cx.new(|cx| {
            ListState::new(
                SessionsDelegate {
                    items: Vec::new(),
                    app: app_ref.clone(),
                },
                window,
                cx,
            )
        });
        let messages = cx.new(|cx| {
            ListState::new(
                MessagesDelegate {
                    items: Vec::new(),
                },
                window,
                cx,
            )
        });
        // 回填自身弱引用，供列表项点击回调使用
        if let Ok(mut g) = app_ref.lock() {
            *g = Some(cx.entity().downgrade());
        }

        let app = Self {
            sessions,
            messages,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
            current_title: "选择左侧会话查看消息".to_string(),
        };
        app.load_sessions(cx);
        app
    }

    fn load_sessions(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let sessions = self.sessions.clone();
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/sessions")).await {
                Ok(body) => {
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
                    println!("[会话] 拉到 {n} 条");
                    cx.update(|app| {
                        sessions.update(app, |state, cx| {
                            state.delegate_mut().items = rows;
                            cx.notify();
                        });
                        this.update(app, |this, cx| {
                            this.session_count = n;
                            this.status = format!("127.0.0.1:{port} · 会话 {n} 条");
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

    fn load_messages(&mut self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        let messages = self.messages.clone();
        println!("[消息] 加载会话 {session_id}");
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages?limit=200"
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let arr = body
                        .get("messages")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let rows: Vec<MsgRow> = arr
                        .iter()
                        .map(|m| MsgRow {
                            role: m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            text: segments_text(m),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[消息] 拉到 {n} 条");
                    cx.update(|app| {
                        messages.update(app, |state, cx| {
                            state.delegate_mut().items = rows;
                            cx.notify();
                        });
                        this.update(app, |this, cx| {
                            this.current_title = format!("会话消息（{n} 条）");
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[消息] 拉取失败: {e}");
                    cx.update(|app| {
                        this.update(app, |this, cx| {
                            this.current_title = format!("加载失败：{e}");
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
            // 左：会话列表
            .child(
                div()
                    .w(px(300.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .text_sm()
                            .child(self.status.clone()),
                    )
                    .child(div().flex_1().min_h_0().child(List::new(&self.sessions))),
            )
            // 右：消息列表
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .text_sm()
                            .border_b_1()
                            .child(self.current_title.clone()),
                    )
                    .child(div().flex_1().min_h_0().child(List::new(&self.messages))),
            )
    }
}

fn main() {
    let config = tianyan_service_config::load_and_install();
    if let Err(e) = tianyan::common::logging::init_logging(&config.logging) {
        eprintln!("日志初始化失败：{e}");
    }
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

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

    println!("[启动] 等待 server 就绪…");
    if !runtime().block_on(wait_for_server(port)) {
        eprintln!("server 30 秒内未就绪，退出");
        std::process::exit(1);
    }
    println!("[启动] server 就绪");

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
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

    let _ = shutdown_tx.send(true);
    println!("[退出] 已发送关停信号");
}
