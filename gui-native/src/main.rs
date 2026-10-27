//! 天演原生桌面客户端（GPUI）。
//!
//! 与 `gui-vite`（Tauri + React）**并存**的第二个客户端，连同一个内嵌 axum server，
//! 走同一套 HTTP/SSE 协议。gui-vite 退役前 `tauri/` 必须保留。
//!
//! 当前进度：装配内核 → 起内嵌 server → 拉会话列表 → 点会话看消息（Markdown 渲染）。
//! 未完成：SSE 流式发送、设置页/workspace/审批面板、托盘/通知/快捷键/updater、单实例保护。

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{IndexPath, Root};
use gpui_kit::{
    list as gpui_list, AppContext as _, IntoElement as _, ListAlignment, ListState as GpuiListState,
};
use gpui_kit::*;
use futures::StreamExt as _;
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

const SERVER_HOST: &str = "127.0.0.1";
const PREFERRED_PORT: u16 = 3000;
/// 单屏渲染的消息条数上限（无虚拟化时控制 Markdown 布局量，否则滚动卡顿）
const RENDER_LIMIT: usize = 60;

/// 本地生成会话 id：服务端 `resolve_or_create_session` 接受前端传入的 id
/// 并在不存在时按它新建（`server/src/api/chat/services.rs`），因此新会话
/// 不必等 `POST /chat/stream` 的响应——事件按 `session_id` 路由，id 必须
/// 在**发起前**就确定，否则首批流事件会被当成别的会话丢掉。
fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("session-{nanos:x}")
}

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

/// POST 一段 JSON 并取回 JSON（同样跑在 tokio runtime 上，跨 runtime await 安全）。
async fn post_json(url: String, body: serde_json::Value) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// 在字节缓冲里找子串（SSE 分帧用；`str::find` 在增量缓冲上有 char 边界风险，
/// 且 UTF-8 多字节字符可能被 chunk 切断——必须**按字节**分帧后再解码）
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 后台读统一事件流（`GET /api/v1/events`，ADR-028 常驻水管）。
///
/// `POST /api/v1/chat/stream` 只是开关，流式输出全部经这条常驻 SSE 下发
/// （`server/src/api/chat/handlers.rs`），所以订阅必须**独立于发送**常驻。
/// 断线（server 未起 / 重启）按 1s 间隔重连；`tx` 关闭即退出（窗口销毁）。
fn spawn_event_reader(port: u16, tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>) {
    let _ = runtime().spawn(async move {
        let client = reqwest::Client::new();
        let url = format!("http://{SERVER_HOST}:{port}/api/v1/events");
        loop {
            if tx.is_closed() {
                return;
            }
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    println!("[事件] 订阅失败（1s 后重试）：{e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            println!("[事件] 已订阅 {}（HTTP {}）", url, resp.status());
            let mut stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        println!("[事件] 读取中断：{e}");
                        break;
                    }
                };
                buf.extend_from_slice(&bytes);
                while let Some(pos) = find_subslice(&buf, b"\n\n") {
                    let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                    let frame = String::from_utf8_lossy(&frame);
                    // SSE 多行 data 按规范用 \n 拼接（axum 的 Event::data 目前是单行）
                    let mut data = String::new();
                    for line in frame.lines() {
                        if let Some(rest) = line.strip_prefix("data:") {
                            if !data.is_empty() {
                                data.push('\n');
                            }
                            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(v) => {
                            if tx.send(v).is_err() {
                                return;
                            }
                        }
                        Err(e) => println!("[事件] 解析失败：{e}"),
                    }
                }
            }
            println!("[事件] 连接结束，1s 后重连");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// 单行省略（ListItem 硬约束：每项必须等高，多行会溢出重叠）
fn ellipsis(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
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
            ellipsis(&row.title, 22)
        };
        let app = self.app.clone();
        let id = row.id.clone();
        Some(
            ListItem::new(ix.row)
                .h(px(40.))
                .child(
                    div()
                        .px_3()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(title),
                )
                .on_click(move |_, _window, cx| {
                    let weak = app.lock().ok().and_then(|g| g.clone());
                    if let Some(w) = weak {
                        let _ = w.update(cx, |this, cx| this.open_session(id.clone(), cx));
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

// ===== 消息（不再用虚拟列表：高度不定，几十到 200 条直接布局更简单可靠）=====

#[derive(Clone, Debug)]
struct MsgRow {
    role: String,
    /// SharedString（Arc 语义）：clone 是 O(1)，避免每帧复制正文
    text: gpui_kit::SharedString,
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
        match ty {
            "text" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                    out.push('\n');
                }
            }
            "thinking" | "reasoning" => {
                if let Some(t) = seg.get("text").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        out.push_str("> 🧠 思考：");
                        out.push_str(&t.replace('\n', "\n> "));
                        out.push('\n');
                    }
                }
            }
            "tool" => {
                // 真实结构：{ type: "tool", tool_call: { name, arguments, .. } }
                let tc = seg.get("tool_call");
                let name = tc
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                out.push_str(&format!("`🔧 {name}`\n"));
            }
            _ => {}
        }
    }
    if out.trim().is_empty() {
        out.push_str("(空消息)");
    }
    out
}

struct ClientApp {
    sessions: Entity<ListState<SessionsDelegate>>,
    /// 消息用 Rc 包一层：闭包驱动的虚拟列表需要把数据带进 'static 闭包，
    /// Rc::clone 是 O(1)，比每帧 clone Vec<MsgRow> 便宜得多
    messages: Rc<Vec<MsgRow>>,
    /// gpui 原生变高虚拟列表的状态（只有可见项会被 render）
    msg_list_state: GpuiListState,
    /// 会话消息总数（messages 可能只保留最近 N 条用于渲染）
    message_total: usize,
    status: String,
    port: u16,
    session_count: usize,
    current_title: String,
    /// 当前选中会话 id —— 事件路由键：只渲染本会话的 `chat_stream` 增量
    current_session: Option<String>,
    /// 输入框（gpui-component 的 `Textarea`：IME / 选区 / 撤销都由它管，不自造）
    input: Entity<TextareaState>,
    /// 是否有轮在跑（`turn_state` 驱动，同时闸住重复发送）
    streaming: bool,
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
        let messages: Rc<Vec<MsgRow>> = Rc::new(Vec::new());
        let total: usize = 0;
        // 回填自身弱引用，供列表项点击回调使用
        if let Ok(mut g) = app_ref.lock() {
            *g = Some(cx.entity().downgrade());
        }

        let input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("输入消息 · Enter 发送 / Shift+Enter 换行")
        });
        // Enter 发送：Textarea 已按多行语义插入换行，发送成功后再清空即无残留
        cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    if !shift {
                        this.send_current_input(window, cx);
                    }
                }
            },
        )
        .detach();

        let app = Self {
            sessions,
            messages,
            msg_list_state: GpuiListState::new(0, ListAlignment::Top, px(120.)),
            message_total: total,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
            current_title: "选择左侧会话查看消息".to_string(),
            current_session: None,
            input,
            streaming: false,
        };
        // 统一事件常驻订阅（ADR-028）：订阅在窗口创建时建立，与「发送」解耦
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        spawn_event_reader(port, tx);
        cx.spawn(async move |this, cx| loop {
            let Some(ev) = rx.recv().await else {
                break;
            };
            if this
                .update(cx, |this, cx| this.handle_event(&ev, cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
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
                        let _ = this.update(app, |this, cx| {
                            this.session_count = n;
                            this.status = format!("127.0.0.1:{port} · 会话 {n} 条");
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[会话] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.status = format!("拉取会话失败：{e}");
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }

    /// 显式停止当前轮：`POST /chat/streams/{id}/cancel`（ADR-036 的结构性取消
    /// 在服务端置位，客户端断线**不**取消——本地助手跑完再取）。
    fn stop_current_turn(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        self.status = "正在停止…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/streams/{sid}/cancel");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("已请求停止 · {sid}"),
                    Err(e) => format!("停止失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 选中会话：切换路由键并拉消息
    fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.current_session = Some(id.clone());
        // 切会话即放弃上一会话的流式状态（本客户端只渲染当前会话的增量）
        self.streaming = false;
        self.load_messages(id, cx);
    }

    /// 处理一条统一事件。
    ///
    /// 只认 `type=chat_stream` 且只认当前会话（子代理事件的 `session_id` 是
    /// `task_id`，属于任务面板，后续接入）。`chunk_type` 语义见
    /// `docs/architecture/event-protocol.md` §4。
    fn handle_event(&mut self, ev: &serde_json::Value, cx: &mut Context<Self>) {
        if ev.get("type").and_then(|v| v.as_str()) != Some("chat_stream") {
            return;
        }
        let sid = ev
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if Some(sid) != self.current_session.as_deref() {
            return;
        }
        match ev
            .get("chunk_type")
            .and_then(|v| v.as_str())
            .unwrap_or("answer")
        {
            "answer" => {
                let delta = ev.get("delta").and_then(|v| v.as_str()).unwrap_or_default();
                self.append_delta(delta);
            }
            // 思考增量暂不并入正文（会污染 Markdown 结构），待独立渲染区
            "thought" => {}
            "tool_call" => {
                let name = ev
                    .get("tool_call")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具");
                self.append_delta(&format!("\n`🔧 {name}`\n"));
            }
            "observation" => {}
            // 消息边界：流末尾的 assistant 完整消息（含工具段）为权威全文，替换占位
            "message" => {
                if let Some(msg) = ev.get("message") {
                    if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                        let full = segments_text(msg);
                        self.replace_last(&full);
                    }
                }
            }
            "error" => {
                let msg = ev.get("delta").and_then(|v| v.as_str()).unwrap_or("未知错误");
                self.streaming = false;
                self.status = format!("出错：{msg}");
                self.append_delta(&format!("\n\n> ⚠️ {msg}\n"));
            }
            "turn_state" => {
                let state = ev
                    .get("turn_state")
                    .and_then(|v| v.get("state"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                self.streaming = state == "running";
                if !self.streaming {
                    self.status = format!("空闲 · {sid}");
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// 往最后一条 assistant 消息追加增量（虚拟列表 count 不变，只重测末项）
    fn append_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let Some(last) = self.messages.last() else {
            return;
        };
        if last.role != "assistant" {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        let merged = format!("{}{}", next[idx].text, delta);
        next[idx].text = merged.into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 用权威全文替换最后一条 assistant 消息
    fn replace_last(&mut self, full: &str) {
        if self.messages.last().map(|m| m.role.as_str()) != Some("assistant") {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].text = full.to_string().into();
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.msg_list_state.scroll_to_end();
    }

    /// 发送输入框内容（Enter 或「发送」按钮）
    fn send_current_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.send_text(text, cx);
    }

    /// 发送一段文本：`POST /chat/stream` 只是开关（ADR-028），输出走常驻 SSE。
    fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        // 新会话：id 必须在发起前确定（事件按 session_id 路由）
        let session_id = match self.current_session.clone() {
            Some(sid) => sid,
            None => {
                let sid = new_session_id();
                self.current_session = Some(sid.clone());
                sid
            }
        };

        let mut next: Vec<MsgRow> = (*self.messages).clone();
        next.push(MsgRow {
            role: "user".to_string(),
            text: text.clone().into(),
        });
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
        });
        if next.len() > RENDER_LIMIT {
            let drop_n = next.len() - RENDER_LIMIT;
            next.drain(..drop_n);
        }
        self.messages = Rc::new(next);
        self.msg_list_state.reset(self.messages.len());
        self.msg_list_state.scroll_to_end();
        self.message_total = self.messages.len();
        self.streaming = true;
        self.status = format!("生成中… · {session_id}");

        let port = self.port;
        let body = serde_json::json!({
            "session_id": session_id,
            "message": { "role": "user", "content": text },
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 2048
        });
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/stream");
            match post_json(url, body).await {
                Ok(_) => {
                    // 新会话首轮：刷新左侧列表把新会话显示出来
                    let _ = this.update(cx, |this, cx| {
                        this.load_sessions(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.streaming = false;
                        this.status = format!("发送失败：{e}");
                        this.append_delta(&format!("\n\n> ⚠️ 发送失败：{e}\n"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_messages(&mut self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
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
                            text: segments_text(m).into(),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[消息] 拉到 {n} 条");
                    // 性能：只渲染最近 RENDER_LIMIT 条（Markdown 布局成本高，
                    // 全量渲染 90+ 条会明显卡顿）。历史加载留待后续做上滚分页。
                    let total = n;
                    let kept: Vec<MsgRow> = if n > RENDER_LIMIT {
                        rows.into_iter().skip(n - RENDER_LIMIT).collect()
                    } else {
                        rows
                    };
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.messages = Rc::new(kept);
                            // 条数变了必须重建 list state（item_count 在构造时固定）
                            this.msg_list_state = GpuiListState::new(
                                this.messages.len(),
                                ListAlignment::Top,
                                px(120.),
                            );
                            this.message_total = total;
                            this.current_title = if total > RENDER_LIMIT {
                                format!("会话消息（共 {total} 条，显示最近 {RENDER_LIMIT} 条）")
                            } else {
                                format!("会话消息（{total} 条）")
                            };
                            cx.notify();
                        });
                    });
                }
                Err(e) => {
                    println!("[消息] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(
                        div().flex_1().min_h_0().child({
                            let items = self.messages.clone();
                            gpui_list(self.msg_list_state.clone(), move |ix, _window, _cx| {
                                let Some(m) = items.get(ix) else {
                                    return div().into_any_element();
                                };
                                let (tag, tag_color) = match m.role.as_str() {
                                    "user" => ("你", rgb(0x7aa2f7)),
                                    "assistant" => ("天演", rgb(0x9ece6a)),
                                    "tool" => ("工具", rgb(0xe0af68)),
                                    _ => ("系统", rgb(0x888888)),
                                };
                                div()
                                    .px_4()
                                    .py_2()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_xs().text_color(tag_color).child(tag))
                                    .child(TextView::markdown(("msg", ix), m.text.clone()))
                                    .into_any_element()
                            })
                        }),
                    )
                    // 底部输入区：Textarea（多行 + IME）与发送按钮
                    .child(
                        div()
                            .border_t_1()
                            .p_2()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_end()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Textarea::new(&self.input).h(px(76.))),
                            )
                            .child(
                                div()
                                    .id("send")
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(0x3b4252))
                                    .text_color(rgb(0xeceff4))
                                    .cursor_pointer()
                                    .child(if self.streaming { "停止" } else { "发送" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.streaming {
                                            this.stop_current_turn(cx);
                                        } else {
                                            this.send_current_input(window, cx);
                                        }
                                    })),
                            ),
                    ),
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
    let server_config = config.clone();
    // JoinHandle 丢掉不取消任务（tokio 默认 detached），与“窗口关闭后再发关停信号”的收尾顺序无关
    let _ = runtime().spawn(async move {
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
            server_config,
            shutdown_rx,
            tianyan_assembly::full(),
        )
        .await
        {
            eprintln!("server 退出：{e}");
        }
    });

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
