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
    decode_data_url, tokens_per_sec, ClientApp, Dialog, FilePickPurpose, PickerTarget,
    SessionListRow, View, SERVER_HOST,
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
            // 当前外观（graphite / parchment / midnight）
            "theme": crate::current_theme().key(),
            "themes": crate::ThemeKind::ALL.iter().map(|t| t.key()).collect::<Vec<_>>(),
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
                if let Dialog::FilePicker { purpose } = d {
                    v["purpose"] = serde_json::json!(purpose.key());
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
            "tools": {
                "count": self.tools.len(),
                "query": &self.tools_query,
                "selected": &self.tools_selected,
                "names": self.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                "items": self.tools.iter().map(|t| serde_json::json!({
                    "name": &t.name,
                    "description": &t.description,
                    // 参数名清单：断言「这个工具带什么参数」不必解析整个 schema
                    "params": t.parameters
                        .get("properties")
                        .and_then(|v| v.as_object())
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                })).collect::<Vec<_>>(),
            },
            "skills": {
                "count": self.skills.len(),
                "total_calls": self.skills_total_calls,
                "query": &self.skills_query,
                "selected": &self.skills_selected,
                "status": &self.skills_status,
                "content_chars": self.skill_content.as_ref().map(|c| c.chars().count()),
                "content_head": self
                    .skill_content
                    .as_ref()
                    .map(|c| c.chars().take(160).collect::<String>()),
                "names": self.skills.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                "items": self.skills.iter().map(|s| serde_json::json!({
                    "id": &s.id, "name": &s.name, "description": &s.description,
                    "version": &s.version, "updated_at": &s.updated_at,
                })).collect::<Vec<_>>(),
                "stats": self.skill_stats.iter().map(|st| serde_json::json!({
                    "skill_id": &st.skill_id, "total_calls": st.total_calls,
                    "success_calls": st.success_calls, "avg_time_ms": st.avg_time_ms,
                })).collect::<Vec<_>>(),
            },
            "scheduled": {
                "count": self.sched_tasks.len(),
                "status": &self.sched_status,
                "form_open": self.sched_form_open,
                "preset": self.sched_preset,
                "items": self.sched_tasks.iter().map(|t| serde_json::json!({
                    "id": &t.id, "name": &t.name, "interval_secs": t.interval_secs,
                    "workspace": &t.workspace, "prompt": &t.prompt, "enabled": t.enabled,
                    "last_run_at": t.last_run_at, "next_run_at": t.next_run_at,
                    // 页面实际显示的那行字（与渲染同一函数生成）：把「时间戳 → 相对时间」
                    // 整条链也纳入可断言范围，否则只能靠看图确认
                    "last_run_display": t
                        .last_run_at
                        .as_deref()
                        .and_then(crate::parse_rfc3339_secs)
                        .map(|ts| format!("上次: {}", crate::relative_from_now(ts)))
                        .unwrap_or_default(),
                    "next_run_display": t
                        .next_run_at
                        .as_deref()
                        .and_then(crate::parse_rfc3339_secs)
                        .map(crate::relative_from_now)
                        .unwrap_or_else(|| "待定".to_string()),
                    "last_result": &t.last_result,
                })).collect::<Vec<_>>(),
            },
            "memory": {
                "count": self.memories.len(),
                "status": &self.memories_status,
                "selected": &self.memory_selected,
                "detail_open": self.memory_detail_open,
                "items": self.memories.iter().map(|m| serde_json::json!({
                    "uri": &m.uri, "name": &m.name, "relative_path": &m.relative_path,
                    "importance": m.importance, "updated_at": &m.updated_at,
                    "tags": &m.tags,
                    // 三层各自是否有内容（断言「记忆有 L1/L2」不必拉全文）
                    "has_l0": m.abstract_text.is_some(),
                    "has_l1": m.overview_text.is_some(),
                    "has_l2": m.detail_text.is_some(),
                })).collect::<Vec<_>>(),
            },
            "insights": {
                "status": &self.insights_status,
                "usage_days": self.usage_days,
                "scheduler": {
                    "running": self.scheduler_running,
                    "executing": &self.scheduler_executing,
                    "task_count": self.sched_status_tasks.len(),
                    "tasks": self.sched_status_tasks.iter().map(|t| serde_json::json!({
                        "id": &t.id, "name": &t.name, "interval_secs": t.interval_secs,
                        "run_count": t.run_count, "next_due_in_secs": t.next_due_in_secs,
                    })).collect::<Vec<_>>(),
                },
                "stats": self.insight_stats.as_ref().map(|s| serde_json::json!({
                    "skills_tracked": s.total_skills_tracked,
                    "skill_calls": s.total_skill_calls,
                    "tools_tracked": s.total_tools_tracked,
                    "tool_calls": s.total_tool_calls,
                    "docs_tracked": s.total_docs_tracked,
                    "searches": s.total_searches,
                })),
                "usage": self.usage_total.as_ref().map(|t| serde_json::json!({
                    "calls": t.calls, "total_tokens": t.total_tokens,
                    "cache_hit_rate": t.cache_hit_rate,
                    "uncached_input": t.uncached_input,
                    "cached_input": t.cached_input,
                    "completion_tokens": t.completion_tokens,
                })),
                "usage_groups": self.usage_grouped.iter().map(|g| serde_json::json!({
                    "group": &g.group, "calls": g.calls,
                    "total_tokens": g.total_tokens, "cache_hit_rate": g.cache_hit_rate,
                })).collect::<Vec<_>>(),
            },
            "approvals_page": {
                "mode": &self.approval_mode,
                "auto": self.approval_auto,
                "timeout_secs": self.approval_timeout,
                "max_pending": self.approval_max_pending,
                "prompt_commands": &self.approval_prompt_cmds,
                "rule_count": self.approval_rules.len(),
                "rules": self.approval_rules.iter().map(|r| serde_json::json!({
                    "name": &r.name, "decision": &r.decision, "enabled": r.enabled,
                })).collect::<Vec<_>>(),
                "pending_confirmations": &self.pending_confirmations,
                "confirmed_action_count": self.confirmed_action_count,
                "history_count": self.approval_history.len(),
                "history": self.approval_history.iter().take(20).map(|h| serde_json::json!({
                    "description": &h.description, "risk": &h.risk,
                    "decision": &h.decision, "session_id": &h.session_id,
                    "result": h.result,
                })).collect::<Vec<_>>(),
                "status": &self.approvals_page_status,
            },
            "knowledge": {
                "tab": self.kb_tab,
                "status": &self.kb_status,
                "path": &self.kb_path,
                "entries": self.kb_entries.iter().map(|e| serde_json::json!({
                    "uri": &e.uri, "name": &e.name, "is_dir": e.is_dir,
                    "has_l0": e.has_abstract, "has_l1": e.has_overview, "has_l2": e.has_detail,
                })).collect::<Vec<_>>(),
                "selected": &self.kb_selected,
                "content_level": self.kb_content.as_ref().map(|(l, _)| l.clone()),
                "content_chars": self.kb_content.as_ref().map(|(_, c)| c.chars().count()),
                "ingest_results": self.kb_ingest_results.iter().map(|(n, s)| serde_json::json!({
                    "filename": n, "status": s,
                })).collect::<Vec<_>>(),
                // 内嵌文件浏览器（不是 OS 对话框 → 可自动化）
                "browse": {
                    "dir": &self.kb_browse_dir,
                    "status": &self.kb_browse_status,
                    "show_hidden": self.kb_show_hidden,
                    "entries": self.kb_browse_entries.iter().map(|e| serde_json::json!({
                        "name": &e.name, "path": &e.path,
                        "is_dir": e.is_dir, "size": e.size,
                    })).collect::<Vec<_>>(),
                    "picked": &self.kb_picked,
                },
                "results": self.kb_results.iter().map(|r| serde_json::json!({
                    "id": &r.id, "source": &r.source, "score": r.score,
                    "content_head": r.content.chars().take(120).collect::<String>(),
                })).collect::<Vec<_>>(),
            },
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
                // 上下文环口径：分子 = 最近一轮请求的 `prompt_tokens`（不是
                // `total_tokens`），分母 = 模型声明的窗口（缺失时才用流式上报值）
                "ctx_used": self.ctx_usage().map(|u| u.prompt_tokens),
                "ctx_window": self.ctx_window(),
                "ctx_pct": self.ctx_pct(),
                "ctx_cache_read": self.ctx_usage().map(|u| u.cache_read),
                "ctx_completion": self.ctx_usage().map(|u| u.completion_tokens),
                "ctx_panel_open": self.ctx_panel_open,
                "ctx_compressing": self.ctx_compressing,
                // 图片放大遮罩是否打开（点击用户消息图片）
                "zoomed_image": self.zoomed_image.is_some(),
                // 待发送附件数（图片）
                "composer_images": self.composer_images.len(),
                // 输入框正文（@ 引用插入后断言它含 `@file:"..."`；
                // 走 `read(cx)` 拿实体真值，不是副本——副本会和实体漂移）
                "composer_text": self.input.read(cx).value(),
                "ctx_compress": self.ctx_compress.as_ref().map(|(ok, m)| serde_json::json!({
                    "ok": ok, "message": m,
                })),
                // 思考强度：档位集来自当前模型声明；`options` 为 None = 该模型不支持思考
                "thinking": self.thinking.clone(),
                "thinking_label": self.thinking_label(),
                "thinking_options": self.thinking_options(),
                "thinking_menu_open": self.thinking_menu_open,
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
        let action = cmd
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
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
            // 上下文详情面板：`open` 显式指定，不带才取反（同上面两处的口径）
            "ctx_panel" => {
                self.ctx_panel_open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.ctx_panel_open);
                if !self.ctx_panel_open {
                    self.ctx_compress = None;
                }
                // 两个浮层同一位置：开一收一
                self.model_menu_open = false;
                cx.notify();
            }
            // 压缩当前会话：只转既有方法（D8：动作不新造逻辑）
            "compress_session" => self.compress_session(cx),
            // 思考强度下拉：`open` 显式指定，不带才取反（同其余浮层口径）
            "thinking_menu" => {
                self.thinking_menu_open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.thinking_menu_open);
                // 三个浮层互斥：开一收二
                self.model_menu_open = false;
                self.ctx_panel_open = false;
                cx.notify();
            }
            // 选择思考强度：`effort` 缺省/`off` = default（不附加思考参数）
            "select_thinking" => {
                let effort = arg_opt(cmd, "effort").filter(|v| v != "off");
                self.select_thinking(effort, cx);
            }
            // 消息区滚到顶部 / 底部：验证「用户气泡」这类靠上内容时必需
            // （列表默认 `ListAlignment::Bottom`，打开会话即贴底）
            "scroll_messages" => {
                match arg_str(cmd, "to")?.as_str() {
                    "top" => {
                        // 先退出「贴底跟随」，否则 scroll_to 会被 follow 逻辑拉回底部
                        self.msg_list_state
                            .set_follow_mode(gpui_kit::FollowMode::Normal);
                        self.msg_list_state.scroll_to(gpui_kit::ListOffset {
                            item_ix: 0,
                            offset_in_item: gpui_kit::px(0.),
                        });
                    }
                    "end" | "bottom" => {
                        // 恢复跟随（用户回到对话尾部的正常行为）
                        self.msg_list_state
                            .set_follow_mode(gpui_kit::FollowMode::Tail);
                        self.msg_list_state.scroll_to_end();
                    }
                    other => return Err(format!("to 只能是 top / bottom：{other}")),
                }
                // ⚠️ 必须触发重绘：`scroll_to` 只改内部状态，不 notify 界面不会动
                // （此前漏了这一句，导致「滚到顶部」从没生效，验证一直在看底部）
                cx.notify();
            }
            // 模型下拉：`open` 显式指定，不带才取反（三个浮层同口径、互斥）
            // 附加图片附件（OS 文件对话框不可自动化，这里直接给路径）
            "attach_image" => {
                let path = arg_str(cmd, "path")?;
                match crate::image_path_to_data_url(std::path::Path::new(&path)) {
                    Some(url) => {
                        self.composer_images.push(url);
                        cx.notify();
                    }
                    None => return Err(format!("不是受支持的图片或读取失败：{path}")),
                }
            }
            // 图片放大（对齐 vite `ZoomableImage`）：不带 `open` 时取反；
            // 打开时取当前消息里**最近一条含图消息的第一张**（验证用）
            "zoom_image" => {
                let open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.zoomed_image.is_none());
                if open {
                    let src = self
                        .messages
                        .iter()
                        .rev()
                        .flat_map(|m| m.images.iter())
                        .next()
                        .cloned();
                    self.zoomed_image =
                        src.as_deref()
                            .and_then(decode_data_url)
                            .map(|(format, bytes)| {
                                Arc::new(gpui_kit::Image::from_bytes(format, bytes))
                            });
                } else {
                    self.zoomed_image = None;
                }
                cx.notify();
            }
            "model_menu" => {
                self.model_menu_open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.model_menu_open);
                self.thinking_menu_open = false;
                self.ctx_panel_open = false;
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
            // ---- 工具 ----
            "select_tool" => {
                let name = arg_str(cmd, "name")?;
                if !self.tools.iter().any(|t| t.name == name) {
                    return Err(format!("没有该工具：{name}"));
                }
                self.tools_selected = Some(name);
                cx.notify();
            }
            // ---- 技能 ----
            "select_skill" => {
                let key = arg_str(cmd, "name")?;
                let Some(skill) = self
                    .skills
                    .iter()
                    .find(|s| s.name == key || s.id == key)
                    .cloned()
                else {
                    return Err(format!("没有该技能：{key}"));
                };
                self.skills_selected = Some(skill.id.clone());
                self.skill_content = None;
                self.skills_status = "正在加载技能内容…".to_string();
                self.load_skill_detail(skill.id, cx);
                cx.notify();
            }
            "reload_skills" => self.load_skills(cx),
            // ---- 定时任务 ----
            "sched_form" => {
                self.sched_form_open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.sched_form_open);
                cx.notify();
            }
            "sched_preset" => {
                let secs = cmd
                    .get("secs")
                    .and_then(|v| v.as_u64())
                    .ok_or("缺少参数 secs（秒）")?;
                if !crate::SCHED_PRESETS.iter().any(|(s, _)| *s == secs) {
                    return Err(format!(
                        "非法预设：{secs}（可选 1800/3600/21600/43200/86400/604800/0）"
                    ));
                }
                self.sched_preset = secs;
                cx.notify();
            }
            "sched_create" => {
                let name = arg_str(cmd, "name")?;
                let prompt = arg_str(cmd, "prompt")?;
                let secs = cmd
                    .get("interval_secs")
                    .and_then(|v| v.as_u64())
                    .ok_or("缺少参数 interval_secs（秒，最小 60）")?;
                if secs < 60 {
                    return Err("间隔至少 60 秒".to_string());
                }
                let workspace = arg_opt(cmd, "workspace").unwrap_or_default();
                self.create_sched_task_direct(name, secs, workspace, prompt, cx);
            }
            "sched_delete" => self.delete_sched_task(arg_str(cmd, "id")?, cx),
            // ---- 外观 ----
            "set_theme" => {
                let key = arg_str(cmd, "theme")?;
                let kind = crate::ThemeKind::from_key(&key).ok_or_else(|| {
                    format!("未知主题：{key}（可选 graphite / parchment / midnight）")
                })?;
                // 与设置页点击共用同一份（要同步组件库 ThemeMode + 持久化）；
                // 自动化上下文没有 `&mut Window`，传 None 只改全局主题
                self.apply_theme_with(kind, None, cx);
            }
            "reload_scheduled" => self.load_sched_tasks(cx),
            // ---- 记忆 ----
            "select_memory" => {
                let key = arg_str(cmd, "name")?;
                let Some(m) = self
                    .memories
                    .iter()
                    .find(|m| m.name == key || m.uri == key)
                    .cloned()
                else {
                    return Err(format!("没有该记忆：{key}"));
                };
                self.memory_selected = Some(m.uri);
                self.memory_detail_open = false;
                cx.notify();
            }
            "memory_detail" => {
                self.memory_detail_open = cmd
                    .get("open")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.memory_detail_open);
                cx.notify();
            }
            "reload_memory" => self.load_memories(cx),
            // ---- 洞察 ----
            "insights_days" => {
                let d = cmd
                    .get("days")
                    .and_then(|v| v.as_i64())
                    .ok_or("缺少参数 days")?;
                if !matches!(d, 1 | 3 | 7 | 30) {
                    return Err(format!("days 只能是 1 / 3 / 7 / 30：{d}"));
                }
                self.usage_days = d;
                self.load_insights(cx);
            }
            "reload_insights" => self.load_insights(cx),
            // ---- 审批页 ----
            "reload_approvals_page" => self.load_approval_page(cx),
            // ---- 知识页 ----
            "kb_tab" => {
                let t = cmd
                    .get("tab")
                    .and_then(|v| v.as_u64())
                    .ok_or("缺少参数 tab（0 浏览 / 1 导入 / 2 搜索）")?;
                if t > 2 {
                    return Err(format!("tab 只能是 0 / 1 / 2：{t}"));
                }
                self.kb_tab = t as u8;
                if t == 0 && self.kb_entries.is_empty() {
                    self.load_kb_entries(String::new(), cx);
                }
                if t == 1 && self.kb_browse_entries.is_empty() {
                    self.load_kb_browse(String::new(), cx);
                }
                cx.notify();
            }
            // ---- 知识页·内嵌文件浏览器（非 OS 对话框 → 可自动化）----
            "kb_browse" => {
                let path = arg_opt(cmd, "path").unwrap_or_default();
                self.load_kb_browse(path, cx);
                cx.notify();
            }
            "kb_open_picker" => {
                // 打开文件选择弹窗（GPUI 自绘、非 OS 对话框）：默认定位到会话工作目录
                self.open_kb_picker(cx);
            }
            // @ 文件引用：同一个弹窗，用途 = Mention（点中文件即插入输入框）
            "mention_file" => {
                self.open_file_picker(FilePickPurpose::Mention, cx);
            }
            // 点中一个文件（真实点击路径是弹窗里的文件行 → `mention_file`；
            // 自动化没有 window，走 `mention_file_auto`，插入 API 同一个）
            "mention_pick" => {
                self.mention_file_auto(arg_str(cmd, "path")?, cx)?;
            }
            // 清空输入框（测试收尾；真实路径里发送成功后也会清）
            "composer_clear" => {
                self.clear_composer(cx)?;
            }
            "kb_close_picker" => {
                self.dialog = None;
                cx.notify();
            }
            "kb_toggle_hidden" => {
                self.kb_show_hidden = cmd
                    .get("show")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!self.kb_show_hidden);
                cx.notify();
            }
            "kb_pick" => {
                let path = arg_str(cmd, "path")?;
                if let Some(pos) = self.kb_picked.iter().position(|p| p == &path) {
                    self.kb_picked.remove(pos);
                } else {
                    self.kb_picked.push(path);
                }
                cx.notify();
            }
            "kb_pick_clear" => {
                self.kb_picked.clear();
                cx.notify();
            }
            "kb_import_picked" => self.import_kb_picked(cx),
            "kb_enter" => {
                let path = arg_opt(cmd, "path").unwrap_or_default();
                self.load_kb_entries(path, cx);
                cx.notify();
            }
            "kb_open" => {
                let uri = arg_str(cmd, "uri")?;
                let level = arg_opt(cmd, "level").unwrap_or_else(|| "detail".to_string());
                self.kb_selected = Some(uri.clone());
                self.read_kb_entry(uri, level, cx);
                cx.notify();
            }
            "kb_delete" => self.delete_kb_entry(arg_str(cmd, "uri")?, cx),
            "kb_search" => self.search_kb(arg_str(cmd, "q")?, cx),
            "kb_ingest" => {
                // 自动化路径：直接给路径与分类，不经过输入框（InputState 没有 set_value）
                let path = arg_str(cmd, "path")?;
                let category = arg_opt(cmd, "category").unwrap_or_default();
                self.kb_ingest_direct(path, category, cx);
            }
            "reset_role" => self.confirm_reset_role(arg_str(cmd, "name")?, cx),
            "delete_role" => self.confirm_delete_role(arg_str(cmd, "name")?, cx),
            other => return Err(format!("未知动作：{other}")),
        }
        Ok(())
    }
}
