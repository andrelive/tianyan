//! GUI 自动化端口（ADR-052）。
//!
//! 让 agent 用 HTTP 直接读界面状态、触发界面语义动作，替代「DPI 抓屏 + 坐标点击」
//! 那套进程外方案。三条边界（ADR-052 原文）：
//!
//! - **默认关闭**：只有 `TIANYAN_GUI_AUTOMATION_PORT` 能解析成端口时才监听，
//!   生产 MSI 不带这个环境变量 → 端口不存在，代码路径也不执行。
//! - **只绑回环**：`127.0.0.1`，不对外暴露（与内嵌 server 同口径）。
//! - **不是存储**：这是 IPC seam，不碰 VFS，不新增任何持久化（AGENTS.md 硬块约束）。
//!
//! ## 线程模型
//!
//! GPUI 的 `App`/`View` 只能在 UI 线程碰，而 HTTP 在 tokio 线程。做法是**主线程独占**：
//! 主线程跑一个 `mpsc` 消费循环，每次指令回传一个 `oneshot`；HTTP handler 只做
//! 「投递 + 等结果」。这样不需要 `AsyncApp::update` 的 park 机制参与跨线程借用，
//! 也天然把「动作 → 读状态」串行化（`POST /act` 带回的状态一定在该动作之后）。
//!
//! ## 为什么不走 MCP
//!
//! 调用方是**能执行命令的 agent**（`curl` 已是最薄客户端），MCP 只在它之上多一层
//! JSON-RPC 编解码；而 MCP 的 request/response 模型表达不了「等一帧/等一等再读」，
//! UI 自动化恰恰离不开这个（本文件用 `settle_ms` 表达）。
//!
//! ## 纪律
//!
//! `automation_act` 的每个动作都**转发现有方法**（`open_session` / `load_*` /
//! `confirm_*`），不新造逻辑。否则自动化路径会和真实点击路径行为分叉，那么
//! 「用它验证过 = 用户点过也一样」这个前提就不成立，验证结论随即作废。

use std::sync::Arc;

use axum::{
    extract::{RawQuery, State as AxumState},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gpui_kit::{App, AsyncApp, WeakEntity};
use tokio::sync::{mpsc, oneshot};

use crate::{
    tokens_per_sec, ClientApp, Dialog, PickerTarget, SessionListRow, View, SERVER_HOST,
};

/// 自动化端口的环境变量名：存在且能解析成端口才监听。
const PORT_ENV: &str = "TIANYAN_GUI_AUTOMATION_PORT";

/// `settle_ms` 的上限：单次请求最多等这么久（防止一个请求把端口占住）。
const MAX_SETTLE_MS: u64 = 10_000;

/// 主线程要执行的一条指令。
enum Cmd {
    /// 读界面状态（`ClientApp::automation_state`）
    State,
    /// 触发一个动作（`{action, ...}`，交给 `ClientApp::automation_act`）
    Act(serde_json::Value),
}

/// 一次调用：指令 + 回传通道（`Err` = 主线程侧拿不到状态，由 HTTP 层转 503）。
struct Envelope {
    cmd: Cmd,
    reply: oneshot::Sender<Result<serde_json::Value, String>>,
}

/// HTTP 层 → UI 主线程的单向桥。
#[derive(Clone)]
struct Bridge {
    tx: mpsc::UnboundedSender<Envelope>,
}

impl Bridge {
    /// 投递一条指令并等主线程回结果；UI 已退出（接收端 drop）时报错。
    async fn call(&self, cmd: Cmd) -> Result<serde_json::Value, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Envelope { cmd, reply })
            .map_err(|_| "UI 主线程已退出".to_string())?;
        rx.await.map_err(|_| "UI 主线程已退出".to_string())?
    }
}

/// 按环境变量决定是否启动自动化端口（未设置 → 直接返回，零副作用）。
pub fn maybe_start(cx: &AsyncApp, view: WeakEntity<ClientApp>) {
    let Some(port) = std::env::var(PORT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
    else {
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<Envelope>();
    // 主线程串行消费（`cx.spawn` 的 future 由 GPUI 的 foreground executor 驱动）
    cx.spawn(async move |cx| {
        while let Some(env) = rx.recv().await {
            // `WeakEntity::update` 返回 `Result`：实体已释放（窗口关掉）时如实上抛
            let out = match env.cmd {
                Cmd::State => view.update(cx, |this, cx| this.automation_state(cx)),
                Cmd::Act(body) => view.update(cx, |this, cx| this.automation_act(&body, cx)),
            }
            .map_err(|e| format!("UI 实体已释放：{e}"));
            // 请求方已断开（超时/取消）不算错误，结果丢掉即可
            let _ = env.reply.send(out);
        }
    })
    .detach();

    let bridge = Arc::new(Bridge { tx });
    crate::runtime().spawn(async move {
        let app = Router::new()
            .route("/health", get(health))
            .route("/state", get(state))
            .route("/act", post(act))
            .with_state(bridge);
        match tokio::net::TcpListener::bind((SERVER_HOST, port)).await {
            Ok(listener) => {
                println!(
                    "[自动化] 已监听 http://127.0.0.1:{port}  \
                     (GET /state · POST /act · GET /health)"
                );
                if let Err(e) = axum::serve(listener, app).await {
                    eprintln!("[自动化] 服务异常退出：{e}");
                }
            }
            Err(e) => eprintln!("[自动化] 端口 {port} 绑定失败：{e}"),
        }
    });
}

/// `GET /health`：探活（agent 用它确认端口上跑的确实是 GUI，而不是别的服务）。
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "service": "tianyan-gui", "pid": std::process::id() }))
}

/// `GET /state[?settle_ms=N]`：界面状态快照。
///
/// `settle_ms` 在 **tokio 侧**睡（不阻塞 UI），用于「刚点完、等异步数据落地」的场景。
async fn state(AxumState(bridge): AxumState<Arc<Bridge>>, RawQuery(raw): RawQuery) -> Response {
    settle_sleep(parse_settle(raw.as_deref())).await;
    match bridge.call(Cmd::State).await {
        Ok(state) => Json(serde_json::json!({ "ok": true, "state": state })).into_response(),
        Err(e) => unavailable(&e),
    }
}

/// `POST /act`：执行一个语义动作，返回**动作之后**的状态（省一次往返）。
///
/// 载荷：`{"action": "...", ...参数, "settle_ms": 300}`
async fn act(
    AxumState(bridge): AxumState<Arc<Bridge>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let settle = body
        .get("settle_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(MAX_SETTLE_MS);
    let acted = match bridge.call(Cmd::Act(body)).await {
        Ok(v) => v,
        Err(e) => return unavailable(&e),
    };
    settle_sleep(settle).await;
    let after = bridge.call(Cmd::State).await.ok();
    let ok = acted.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut out = serde_json::json!({ "ok": ok, "state": after });
    if let Some(err) = acted.get("error") {
        out["error"] = err.clone();
    }
    Json(out).into_response()
}

/// UI 主线程已经不在了（窗口关闭 / 进程退出中）。
fn unavailable(reason: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "ok": false, "error": reason })),
    )
        .into_response()
}

/// 从原始 query 串里取 `settle_ms`（不引 `serde::Deserialize`：只有一个可选整型）。
fn parse_settle(raw: Option<&str>) -> u64 {
    raw.unwrap_or_default()
        .split('&')
        .find_map(|kv| kv.strip_prefix("settle_ms="))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(MAX_SETTLE_MS)
}

/// 等 `settle_ms` 毫秒（0 = 不睡）。
async fn settle_sleep(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

// ===== 视图侧：状态快照与语义动作 =====

/// 取必填字符串参数（缺失即报错，报错文本直接回给调用方）。
fn arg_str(cmd: &serde_json::Value, key: &str) -> Result<String, String> {
    cmd.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("缺少参数 {key}（字符串）"))
}

/// 取可选字符串参数。
fn arg_opt(cmd: &serde_json::Value, key: &str) -> Option<String> {
    cmd.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

impl ClientApp {
    /// 界面状态快照（`GET /state` 的载荷）。**只读**：不触发请求、不改状态。
    ///
    /// 字段名即对外契约。刻意**不含** `api_key`（`settings.providers` 只给名字与
    /// 端点）：自动化端口是把界面摊开给外部读，密钥没有出现在这里的理由。
    fn automation_state(&self, cx: &App) -> serde_json::Value {
        // 左栏混排行按 `kind` 摊平：断言「某组有几条可见」「查看更多还剩几个」
        // 靠它，不必去解析渲染结果。
        let session_rows: Vec<serde_json::Value> = self
            .session_rows
            .iter()
            .map(|row| match row {
                SessionListRow::Group {
                    key,
                    label,
                    count,
                    collapsed,
                } => serde_json::json!({
                    "kind": "group", "key": key, "label": label,
                    "count": count, "collapsed": collapsed,
                }),
                SessionListRow::Session {
                    id,
                    title,
                    meta,
                    tasks,
                    selected,
                } => serde_json::json!({
                    "kind": "session", "id": id, "title": title,
                    "meta": meta, "tasks": tasks, "selected": selected,
                }),
                SessionListRow::More { key, hidden } => serde_json::json!({
                    "kind": "more", "key": key, "hidden": hidden,
                }),
            })
            .collect();

        // 最后一条消息的正文头：「消息内容到底显示出来没有」不必看图即可断言。
        let last_msg = self.messages.last().map(|m| {
            let text: &str = m.text.as_ref();
            serde_json::json!({
                "role": &m.role,
                "chars": text.chars().count(),
                "head": text.chars().take(160).collect::<String>(),
                "segs": m.segs.len(),
                "tool_calls": m.tool_calls.len(),
                "compression": m.compression,
            })
        });

        serde_json::json!({
            "view": self.view.key(),
            "views": View::ALL.iter().map(|v| v.key()).collect::<Vec<_>>(),
            "status": &self.status,
            "port": self.port,
            "streaming": self.streaming,
            "session": {
                "current": &self.current_session,
                "title": &self.current_title,
                "count": self.session_count,
            },
            "messages": {
                "loaded": self.messages.len(),
                "total": self.message_total,
                "last": last_msg,
            },
            "input": self.input.read(cx).value().to_string(),
            "sessions_rows": session_rows,
            "sessions_data": self.sessions_data.iter().map(|s| serde_json::json!({
                "id": &s.id, "title": &s.title,
                "working_directory": &s.working_directory,
                "updated_at": &s.updated_at,
            })).collect::<Vec<_>>(),
            "collapsed_groups": self.collapsed_groups.iter().collect::<Vec<_>>(),
            "expanded_groups": self.expanded_groups.iter().map(|(k, v)| serde_json::json!({
                "key": k, "extra": v,
            })).collect::<Vec<_>>(),
            "dialog": self.dialog.as_ref().map(|d| {
                let mut v = serde_json::json!({ "kind": d.key() });
                if let Dialog::WorkspacePicker { target } = d {
                    v["target"] = serde_json::json!(target.key());
                }
                v
            }),
            "picker": {
                "open": matches!(self.dialog, Some(Dialog::WorkspacePicker { .. })),
                "current": &self.picker_current,
                "parent": &self.picker_parent,
                "names": self.picker_entries.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                "entries": self.picker_entries.iter().map(|(n, p)| serde_json::json!({
                    "name": n, "path": p,
                })).collect::<Vec<_>>(),
                "status": &self.picker_status,
            },
            "workspace": {
                "root": &self.ws_root,
                "path": &self.ws_path,
                "status": &self.ws_status,
                "entries": self.ws_entries.iter().map(|e| serde_json::json!({
                    "name": &e.name, "path": &e.path, "dir": e.is_dir, "size": e.size,
                })).collect::<Vec<_>>(),
                "file": self.ws_file.as_ref().map(|(p, text)| serde_json::json!({
                    "path": p,
                    "lines": text.lines().count(),
                    "chars": text.chars().count(),
                })),
                "diff": {
                    "open": self.ws_diff_open,
                    "status": &self.ws_diff_status,
                    "files": self.ws_diff_files.iter().map(|f| serde_json::json!({
                        "path": &f.path, "status": &f.status,
                        "old_lines": f.old_lines, "new_lines": f.new_lines,
                    })).collect::<Vec<_>>(),
                    "current": self.ws_diff.as_ref().map(|(p, text)| serde_json::json!({
                        "path": p,
                        "lines": text.lines().count(),
                        "added": text.lines()
                            .filter(|l| l.starts_with('+') && !l.starts_with("+++")).count(),
                        "removed": text.lines()
                            .filter(|l| l.starts_with('-') && !l.starts_with("---")).count(),
                    })),
                },
            },
            "tasks": {
                "collapsed": self.tasks_collapsed,
                "expanded": &self.expanded_task,
                "visible": self.tasks.len(),
                "all": self.all_tasks.len(),
                "items": self.tasks.iter().map(|t| serde_json::json!({
                    "id": &t.id, "kind": &t.kind, "status": &t.status,
                    "description": &t.description,
                    "parent_session_id": &t.parent_session_id,
                    "output_tail_chars": t.output_tail.chars().count(),
                    "error": &t.error,
                    // 已加载的子会话消息条数（活动流是否拉到，看这个）
                    "activity": self.task_messages.get(&t.id).map(|m| m.len()),
                })).collect::<Vec<_>>(),
            },
            "todos": self.todos.iter().map(|(s, t)| serde_json::json!({
                "status": s, "title": t,
            })).collect::<Vec<_>>(),
            "goals": self.goals.iter().map(|g| serde_json::json!({
                "title": &g.title, "status": &g.status, "progress": g.progress,
                "todo_total": g.todo_total, "todo_done": g.todo_done,
            })).collect::<Vec<_>>(),
            "todo_bar_collapsed": self.todo_bar_collapsed,
            "approvals": self.pending_approvals.iter().map(|a| serde_json::json!({
                "request_id": &a.request_id,
                "description": &a.description,
                "risk": &a.risk,
            })).collect::<Vec<_>>(),
            "clarification": self.clarification.as_ref().map(|qs| {
                qs.iter().enumerate().map(|(i, (q, opts))| serde_json::json!({
                    "question": q, "options": opts,
                    "answer": self.clarify_answers.get(i),
                })).collect::<Vec<_>>()
            }),
            "roles": {
                "count": self.roles.len(),
                "query": &self.roles_query,
                "selected": &self.roles_selected,
                "status": &self.roles_status,
                "action_msg": &self.roles_action_msg,
                "detail": self.role_detail.as_ref().map(|d| serde_json::json!({
                    "system_prompt_chars": d.system_prompt.chars().count(),
                    "system_prompt_head": d.system_prompt.chars().take(200).collect::<String>(),
                    "tools": &d.tools,
                })),
                "stats": {
                    "total_calls": self.roles_stats.total_calls,
                    "success_rate": self.roles_stats.success_rate,
                },
                "items": self.roles.iter().map(|r| serde_json::json!({
                    "name": &r.name, "source": &r.source, "status": &r.status,
                    "version": r.version, "purpose": &r.purpose,
                    "tool_count": r.tool_count, "model": &r.model, "usage": r.usage,
                })).collect::<Vec<_>>(),
            },
            "settings": {
                "status": &self.settings_status,
                "error": self.settings_error,
                "menu_open": self.model_menu_open,
                "session_model": &self.session_model,
                "current_model": self.current_model_label(),
                "editing_provider": &self.editing_provider,
                "models": self.models.iter().map(|m| serde_json::json!({
                    "provider": &m.provider, "name": &m.name,
                    "capabilities": &m.capabilities, "context": m.context,
                    "is_chat": m.is_chat,
                })).collect::<Vec<_>>(),
                "providers": self.providers.iter().map(|p| serde_json::json!({
                    "name": &p.name, "endpoint": &p.endpoint,
                })).collect::<Vec<_>>(),
            },
            "usage": {
                "ctx_used": self.ctx_used,
                "ctx_window": self.ctx_window,
                "ctx_pct": self.ctx_pct(),
                "session": self.session_usage.map(|(i, c, o)| serde_json::json!({
                    "input_uncached": i, "input_cached": c, "output": o,
                })),
                "request": self.request_perf.map(|(ttft, dur, completion)| serde_json::json!({
                    "ttft_ms": ttft, "duration_ms": dur,
                    "completion_tokens": completion,
                    "tokens_per_sec": tokens_per_sec(ttft, dur, completion),
                })),
            },
        })
    }

    /// 执行一个语义动作（`POST /act` 的载荷），返回 `{ok, action[, error]}`。
    fn automation_act(
        &mut self,
        cmd: &serde_json::Value,
        cx: &mut gpui_kit::Context<Self>,
    ) -> serde_json::Value {
        let action = cmd.get("action").and_then(|v| v.as_str()).unwrap_or_default();
        match self.apply_action(action, cmd, cx) {
            Ok(()) => serde_json::json!({ "ok": true, "action": action }),
            Err(e) => serde_json::json!({ "ok": false, "action": action, "error": e }),
        }
    }

    /// 动作分派。每个分支只调既有方法，不新造逻辑。
    fn apply_action(
        &mut self,
        action: &str,
        cmd: &serde_json::Value,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Result<(), String> {
        match action {
            // ---- 导航 ----
            "switch_tab" => {
                let key = arg_str(cmd, "view")?;
                let view = View::from_key(&key).ok_or_else(|| format!("未知视图：{key}"))?;
                // 走 `switch_view`（与侧栏点击同一份）：页内首次进入的补拉住在那里，
                // 直接赋 `self.view` 会跳过它（实测：文件页树因此一直为空）
                self.switch_view(view, cx);
            }
            "select_session" => self.open_session(arg_str(cmd, "id")?, cx),
            "new_session" => self.start_new_session(cx),
            "reload" => {
                self.load_sessions(cx);
                self.load_tasks(cx);
                if self.current_session.is_some() {
                    self.load_todos_goals(cx);
                    self.load_approvals(cx);
                }
                cx.notify();
            }
            // ---- 对话 ----
            "send_message" => self.send_text(arg_str(cmd, "text")?, cx),
            "stop_turn" => self.stop_current_turn(cx),
            // ---- 左栏 ----
            "toggle_group" => self.toggle_group(arg_str(cmd, "key")?, cx),
            "expand_group" => self.expand_group(arg_str(cmd, "key")?, cx),
            // ---- 目录选择器 ----
            "open_picker" => {
                let target = match arg_str(cmd, "target")?.as_str() {
                    "session" => PickerTarget::Session,
                    "new_session" => PickerTarget::NewSession,
                    other => return Err(format!("target 只能是 session / new_session：{other}")),
                };
                self.open_workspace_picker(target, cx);
            }
            "picker_enter" => {
                // 二选一：`name`（按当前层可见的子目录名）或 `path`（绝对路径）
                let path = match arg_opt(cmd, "path") {
                    Some(p) => p,
                    None => {
                        let name = arg_str(cmd, "name")?;
                        self.picker_entries
                            .iter()
                            .find(|(n, _)| n == &name)
                            .map(|(_, p)| p.clone())
                            .ok_or_else(|| format!("当前层没有目录：{name}"))?
                    }
                };
                self.load_picker_dirs(path, cx);
                cx.notify();
            }
            "picker_up" => {
                let parent = self.picker_parent.clone().ok_or("已在浏览根")?;
                self.load_picker_dirs(parent, cx);
                cx.notify();
            }
            "picker_confirm" => {
                let Some(Dialog::WorkspacePicker { target }) = self.dialog.clone() else {
                    return Err("目录选择器未打开".to_string());
                };
                self.confirm_workspace_picker(target, cx);
            }
            "picker_cancel" => {
                self.dialog = None;
                cx.notify();
            }
            // ---- 文件页 ----
            "ws_enter" => {
                let path = arg_opt(cmd, "path").unwrap_or_default();
                self.load_workspace_tree(path, cx);
                cx.notify();
            }
            "ws_open_file" => self.load_workspace_file(arg_str(cmd, "path")?, cx),
            // `open: true/false` 显式指定，不带才取反。
            // 只给 toggle 的话调用方无法表达「我要它开着」——实测踩过：第一次调用
            // 打开、第二次（想确认仍然开着）反而关掉了。
            "ws_diff" | "ws_diff_toggle" => {
                let open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.ws_diff_open);
                self.ws_diff_open = open;
                if open {
                    self.load_workspace_diff_list(cx);
                }
                cx.notify();
            }
            "ws_diff_open" => self.load_workspace_diff(arg_str(cmd, "path")?, cx),
            // ---- 任务面板 ----
            "expand_task" => {
                let id = arg_opt(cmd, "id");
                self.expanded_task = id.clone();
                if let Some(id) = id {
                    self.load_task_messages(id, cx);
                }
                cx.notify();
            }
            "tasks_collapse" => {
                self.tasks_collapsed = cmd
                    .get("collapsed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                cx.notify();
            }
            "cancel_task" => self.cancel_task(arg_str(cmd, "id")?, cx),
            // ---- 待办 / 审批 / 追问 ----
            // 同上：`collapsed` 显式指定，不带才取反
            "todo_bar" | "toggle_todo_bar" => {
                self.todo_bar_collapsed = cmd
                    .get("collapsed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.todo_bar_collapsed);
                cx.notify();
            }
            "respond_approval" => {
                let decision = arg_str(cmd, "decision")?;
                let request_id = match arg_opt(cmd, "request_id") {
                    Some(id) => id,
                    None => self
                        .pending_approvals
                        .first()
                        .map(|a| a.request_id.clone())
                        .ok_or("没有待审批项")?,
                };
                // `respond_approval` 收 `&'static str`，只能走这两个字面量
                match decision.as_str() {
                    "approve" => self.respond_approval(request_id, "approve", cx),
                    "deny" => self.respond_approval(request_id, "deny", cx),
                    other => return Err(format!("decision 只能是 approve / deny：{other}")),
                }
            }
            "answer_clarification" => {
                let answers: Vec<String> = cmd
                    .get("answers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .ok_or("缺少参数 answers（字符串数组）")?;
                let expect = self.clarification.as_ref().map(|q| q.len()).unwrap_or(0);
                if expect == 0 {
                    return Err("当前没有待回答的追问".to_string());
                }
                if answers.len() != expect {
                    return Err(format!("answers 长度应为 {expect}，实际 {}", answers.len()));
                }
                self.clarify_answers = answers;
                self.submit_clarification(cx);
            }
            // ---- 子智能体 ----
            "select_role" => {
                let name = arg_str(cmd, "name")?;
                self.roles_selected = Some(name.clone());
                self.load_role_detail(name, cx);
                cx.notify();
            }
            "reset_role" => self.confirm_reset_role(arg_str(cmd, "name")?, cx),
            "delete_role" => self.confirm_delete_role(arg_str(cmd, "name")?, cx),
            other => return Err(format!("未知动作：{other}")),
        }
        Ok(())
    }
}
