//! 应用主体：生命周期、全局事件、一次发送的完整链路。
//!
//! `struct ClientApp` 的**定义**在这里（字段见 `ClientApp`），其余文件用
//! `impl ClientApp` 扩展各自域的方法——Rust 允许同一类型在本 crate 内多个模块
//! 各有 `impl` 块，这是把 1.1 万行 impl 拆开而不引入中间层的唯一办法。

use crate::*;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::msg::AppRef;
use crate::theme::colors;
use crate::types::View;
use crate::types::*;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::{ListAlignment, ListState as GpuiListState};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

impl ClientApp {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        port: u16,
        tray_rx: tokio::sync::mpsc::UnboundedReceiver<tray::TrayCommand>,
    ) -> Self {
        let expand: ExpandRef = Rc::new(std::cell::RefCell::new(ExpandState::default()));
        let app_ref: AppRef = Arc::new(Mutex::new(None));
        let messages: Rc<Vec<MsgRow>> = Rc::new(Vec::new());
        let total: usize = 0;

        let input = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("输入消息 · Enter 发送 / Shift+Enter 换行")
        });
        // 子智能体页的搜索框：改动即过滤（订阅 Change 事件）
        let roles_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索子智能体..."));
        cx.subscribe_in(
            &roles_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.roles_query = this.roles_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 工具页的搜索框：改动即过滤（同 roles_search 的口径）
        let tools_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索工具..."));
        cx.subscribe_in(
            &tools_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.tools_query = this.tools_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 技能页的搜索框（同口径）
        let skills_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索技能..."));
        cx.subscribe_in(
            &skills_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.skills_query = this.skills_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 文件选择弹窗的过滤框：改动即过滤（同 roles/tools 搜索框的口径）
        let kb_pick_search = cx.new(|cx| InputState::new(window, cx).placeholder("按名字过滤…"));
        cx.subscribe_in(
            &kb_pick_search,
            window,
            |this, _input, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.kb_pick_query = this.kb_pick_search.read(cx).value().to_string();
                    cx.notify();
                }
            },
        )
        .detach();
        // 知识页的三个输入框（导入路径 / 导入分类 / 搜索词）
        let kb_ingest_path =
            cx.new(|cx| InputState::new(window, cx).placeholder("文件或目录的绝对路径"));
        let kb_ingest_category = cx
            .new(|cx| InputState::new(window, cx).placeholder("分类（可选，如 technical / docs）"));
        let kb_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索知识库..."));
        // 定时任务页的四个表单输入框（名称 / 自定义秒 / 工作目录 / 指令）。
        // 不订阅 Change：值只在「创建」时读一次，没必要每敲一个字就刷视图。
        let sched_name = cx.new(|cx| InputState::new(window, cx).placeholder("任务名称"));
        let sched_custom_secs =
            cx.new(|cx| InputState::new(window, cx).placeholder("自定义间隔（秒，最小 60）"));
        let sched_workspace = cx
            .new(|cx| InputState::new(window, cx).placeholder("工作目录（绝对路径，留空 = 默认）"));
        let sched_prompt = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("给智能体的指令（每次触发让它做的事）")
        });
        // 设置页的两个编辑框：endpoint 与 api_key（打开编辑时从 `GET /config` 回填）
        let endpoint_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("API 端点（http(s)://…）"));
        let key_input = cx.new(|cx| TextareaState::new(window, cx).placeholder("API 密钥"));
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
            session_rows: Vec::new(),
            sessions_data: Vec::new(),
            collapsed_groups: std::collections::HashSet::new(),
            expanded_groups: std::collections::HashMap::new(),
            hovered_session: None,
            all_tasks: Vec::new(),
            expanded_task: None,
            task_messages: std::collections::HashMap::new(),
            expand,
            app_ref: app_ref.clone(),
            roles: Vec::new(),
            roles_stats: RolesStats::default(),
            roles_query: String::new(),
            roles_search,
            roles_selected: None,
            role_detail: None,
            roles_status: "正在加载子智能体…".to_string(),
            roles_action_msg: None,
            tools: Vec::new(),
            tools_query: String::new(),
            tools_search,
            tools_selected: None,
            skills: Vec::new(),
            skill_stats: Vec::new(),
            skills_total_calls: 0,
            skills_query: String::new(),
            skills_search,
            skills_selected: None,
            skill_content: None,
            skills_status: "正在加载技能…".to_string(),
            sched_tasks: Vec::new(),
            sched_form_open: false,
            sched_preset: 21600,
            sched_name,
            sched_custom_secs,
            sched_workspace,
            sched_prompt,
            sched_status: String::new(),
            memories: Vec::new(),
            memories_status: "正在加载记忆…".to_string(),
            memory_selected: None,
            memory_detail_open: false,
            insight_stats: None,
            usage_total: None,
            usage_grouped: Vec::new(),
            usage_days: 7,
            scheduler_running: false,
            scheduler_executing: None,
            sched_status_tasks: Vec::new(),
            insights_status: "正在加载洞察…".to_string(),
            approval_mode: String::new(),
            approval_auto: false,
            approval_timeout: 0,
            approval_max_pending: 0,
            approval_prompt_cmds: Vec::new(),
            approval_rules: Vec::new(),
            pending_confirmations: Vec::new(),
            confirmed_action_count: 0,
            approval_history: Vec::new(),
            approvals_page_status: "正在加载审批状态…".to_string(),
            kb_tab: 0,
            kb_path: String::new(),
            kb_entries: Vec::new(),
            kb_selected: None,
            kb_content: None,
            kb_status: "正在加载知识库…".to_string(),
            kb_ingest_path,
            kb_ingest_category,
            kb_ingest_results: Vec::new(),
            kb_browse_dir: String::new(),
            kb_browse_entries: Vec::new(),
            kb_browse_status: String::new(),
            kb_show_hidden: false,
            kb_pick_query: String::new(),
            kb_pick_search,
            kb_picked: Vec::new(),
            kb_search,
            kb_results: Vec::new(),
            picker_current: String::new(),
            picker_parent: None,
            picker_entries: Vec::new(),
            picker_status: String::new(),
            ws_root: String::new(),
            ws_path: String::new(),
            ws_entries: Vec::new(),
            ws_file: None,
            ws_status: String::new(),
            ws_diff_open: false,
            ws_diff_files: Vec::new(),
            ws_diff: None,
            ws_diff_status: String::new(),
            dialog: None,
            zoomed_image: None,
            composer_images: Vec::new(),
            inputs: Vec::new(),
            running_sessions: std::collections::HashSet::new(),
            draft_restore_path: String::new(),
            parked: Vec::new(),
            oldest_seq: None,
            has_more_history: false,
            loading_older: false,
            needs_setup: false,
            setup_dismissed: false,
            notices: Vec::new(),
            open_tabs: Vec::new(),
            new_session_workspace: None,
            dialog_input: None,
            messages,
            msg_list_state: GpuiListState::new(0, ListAlignment::Bottom, px(120.)),
            message_total: total,
            status: "正在连接后端…".to_string(),
            port,
            session_count: 0,
            current_title: "选择左侧会话查看消息".to_string(),
            current_session: None,
            input,
            streaming: false,
            view: View::Chat,
            models: Vec::new(),
            settings_status: "正在加载模型列表…".to_string(),
            settings_error: false,
            settings_tab: SettingsTab::Models,
            raw_config: None,
            session_model: None,
            model_menu_open: false,
            thinking: None,
            thinking_menu_open: false,
            ctx_live: None,
            ctx_window_live: 0,
            ctx_panel_open: false,
            ctx_compressing: false,
            ctx_compress: None,
            session_usage: None,
            request_perf: None,
            tasks: Vec::new(),
            tasks_collapsed: false,
            todos: Vec::new(),
            goals: Vec::new(),
            todo_bar_collapsed: false,
            pending_approvals: Vec::new(),
            clarification: None,
            clarify_answers: Vec::new(),
            providers: Vec::new(),
            editing_provider: None,
            edit_endpoint: endpoint_input,
            edit_api_key: key_input,
            settings_editors: ClientApp::create_settings_editors(window, cx),
            settings_dirty: std::collections::HashMap::new(),
            settings_msg: String::new(),
            settings_msg_err: false,
            settings_backfilled: false,
            soul_editor: cx
                .new(|cx| TextareaState::new(window, cx).placeholder("智能体核心人设（Markdown）")),
            soul_default: String::new(),
            soul_loaded: false,
            soul_msg: String::new(),
            soul_msg_err: false,
            mcp_servers: Vec::new(),
            mcp_test: std::collections::HashMap::new(),
            mcp_msg: String::new(),
            mcp_msg_err: false,
            mcp_form_open: false,
            mcp_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("名称（唯一，如 filesystem）")),
            mcp_command: cx
                .new(|cx| InputState::new(window, cx).placeholder("命令（如 npx / uvx / python）")),
            mcp_args: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("参数（空格分隔，如 -y @modelcontextprotocol/server-filesystem）")
            }),
            mcp_desc: cx.new(|cx| InputState::new(window, cx).placeholder("说明（可选）")),
            storage_move_target: cx.new(|cx| {
                InputState::new(window, cx).placeholder("目标数据目录（绝对路径，须为空或不存在）")
            }),
            storage_msg: String::new(),
            storage_msg_err: false,
            usage_group_by: "model",
            usage_msg: String::new(),
            health_info: None,
            health_msg: String::new(),
            models_msg: String::new(),
            models_msg_err: false,
            model_form_open: false,
            model_form_provider: None,
            model_form_name: cx.new(|cx| {
                InputState::new(window, cx).placeholder("模型名（与上游一致，如 gpt-4o）")
            }),
            model_form_caps: Vec::new(),
            prov_form_open: false,
            prov_form_name: cx.new(|cx| {
                InputState::new(window, cx).placeholder("提供商名称（如 openai / ollama）")
            }),
            prov_form_endpoint: cx.new(|cx| {
                InputState::new(window, cx).placeholder("端点（可留空：命中预置时自动补齐）")
            }),
            prov_form_key: cx.new(|cx| InputState::new(window, cx).placeholder("密钥（可选）")),
            prov_form_model: cx
                .new(|cx| InputState::new(window, cx).placeholder("首个模型名（必填，如 gpt-4o）")),
            prov_form_caps: Vec::new(),
            delete_confirm: None,
            window_handle: window.window_handle(),
            last_screenshot: None,
        };
        // 回填自身弱引用：消息区折叠块的点击回调要用它 update 回 view
        if let Ok(mut g) = app_ref.lock() {
            *g = Some(cx.entity().downgrade());
        }
        // 关闭按钮 → 藏到托盘（不是退出）。托盘「退出天演」会先 `begin_quit()`
        // 置位，这里的回调据此放行——否则用户永远关不掉这个程序。
        window.on_window_should_close(cx, |_window, _cx| {
            if tray::is_quitting() {
                true
            } else {
                tray::hide_main_window();
                false
            }
        });
        // 托盘菜单动作（主线程执行）：显隐窗口 / 退出。
        // 窗口操作走 `tray::*` 的 Win32 自由函数而不是 GPUI API —— 后者只有
        // minimize/activate，没有 hide（而托盘语义要求窗口从任务栏消失）。
        cx.spawn(async move |this, cx| {
            let mut tray_rx = tray_rx;
            while let Some(cmd) = tray_rx.recv().await {
                match cmd {
                    tray::TrayCommand::Show => tray::show_main_window(),
                    tray::TrayCommand::Hide => tray::hide_main_window(),
                    tray::TrayCommand::Quit => {
                        tray::begin_quit();
                        let _ = this.update(cx, |this, cx| {
                            let handle = this.window_handle;
                            let _ = handle.update(cx, |_, window, _cx| window.remove_window());
                        });
                        break;
                    }
                }
            }
        })
        .detach();
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
        app.load_models(cx);
        // 子智能体页数据预加载：等到切页时才拉会让用户先看到空白再「跳」出内容
        app.load_roles(cx);
        app.load_roles_stats(cx);
        // 后台任务面板轮询（3s，对齐 Tauri 版 `AgentTasksPanel.POLL_INTERVAL_MS`）。
        // 面板是观测面：只在有选中会话时拉，失败静默（`load_tasks` 内部处理）。
        cx.spawn(async move |this, cx| loop {
            sleep_secs(3).await;
            let alive = this.update(cx, |this, cx| {
                // 任务列表**无条件**轮询：左栏会话角标要按 `parent_session_id`
                // 聚合所有会话，未选中会话时也得更新；面板那三个只在有会话时拉。
                this.load_tasks(cx);
                // 顺手做过期提示清理（同一个 3s tick，不额外起定时器）
                this.expire_notices();
                if this.current_session.is_some() {
                    this.load_todos_goals(cx);
                    this.load_approvals(cx);
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        app
    }

    /// 全局一级导航栏（64px 图标 rail）。
    ///
    /// 与 Tauri 版 `Sidebar.tsx` 同构：**固定宽度、不做展开/收起**——会话列表在会话页
    /// 左栏，不在 rail 里；rail 只放 11 个一级入口。未实现的入口点进去是占位页。
    pub(crate) fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rail = div()
            .w(px(colors::RAIL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_3()
            .bg(rgb(colors::BG_SECONDARY()))
            .border_r_1()
            .border_color(rgb(colors::BORDER()))
            // 品牌块（Tauri 版是「天」字方块）
            .child(
                div()
                    .w(px(36.))
                    .h(px(36.))
                    .mb_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(rgb(colors::BG_TERTIARY()))
                    .text_color(rgb(colors::ACCENT()))
                    .child("天"),
            );
        for (ix, entry) in NAV_ITEMS.iter().enumerate() {
            let (icon, _label, view) = *entry;
            let tint = if self.view == view {
                colors::ACCENT()
            } else {
                colors::TEXT_SECONDARY()
            };
            let mut item = div()
                .id(("nav", ix))
                .w(px(40.))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_sm();
            if self.view == view {
                item = item.bg(rgb(colors::BG_TERTIARY())).text_color(rgb(tint));
            } else {
                item = item.text_color(rgb(tint));
            }
            // Lucide SVG（stroke 用 currentColor → 由 text_color 上色，可随选中态换色）
            rail = rail.child(
                item.child(svg().path(icon.path()).size(px(18.)).text_color(rgb(tint)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.view = view;
                        if view == View::Settings {
                            this.load_models(cx);
                        }
                        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
                        // 避免每次切页都发请求、页面先空一帧再填
                        if view == View::Roles && this.roles.is_empty() {
                            this.load_roles(cx);
                            this.load_roles_stats(cx);
                        }
                        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
                        if view == View::Workspace && this.ws_root.is_empty() {
                            this.load_workspace_tree(String::new(), cx);
                        }
                        cx.notify();
                    })),
            );
        }
        rail
    }

    pub(crate) fn switch_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        if view == View::Settings {
            self.load_models(cx);
        }
        // 数据已在启动时预加载：只在空列表（首次失败/未拉到）时补拉，
        // 避免每次切页都发请求、页面先空一帧再填
        if view == View::Roles && self.roles.is_empty() {
            self.load_roles(cx);
            self.load_roles_stats(cx);
        }
        // 文件页首次进入才拉目录树（工作区根 = 会话绑定目录）
        if view == View::Workspace && self.ws_root.is_empty() {
            self.load_workspace_tree(String::new(), cx);
        }
        // 工具页首次进入才拉工具目录（与 roles 同一懒加载口径）
        if view == View::Tools && self.tools.is_empty() {
            self.load_tools(cx);
        }
        // 技能页首次进入才拉（列表 + 使用统计）
        if view == View::Skills && self.skills.is_empty() {
            self.load_skills(cx);
        }
        // 定时任务页每次进入都刷新：agent 可能刚用 schedule_task 建过任务
        if view == View::Scheduled {
            self.load_sched_tasks(cx);
        }
        // 记忆页每次进入都刷新：演化任务会在会话结束后写入
        if view == View::Memory {
            self.load_memories(cx);
        }
        // 洞察页每次进入都刷新（用量/调度状态随时在变）
        if view == View::Insights {
            self.load_insights(cx);
        }
        // 审批页每次进入都刷新（挂起请求随时可能出现）
        if view == View::Approval {
            self.load_approval_page(cx);
        }
        // 知识页首次进入才拉根目录（后续导航自己发请求）
        if view == View::Knowledge && self.kb_entries.is_empty() {
            self.load_kb_entries(String::new(), cx);
        }
        cx.notify();
    }

    /// 处理一条统一事件。
    ///
    /// 只认 `type=chat_stream` 且只认当前会话（子代理事件的 `session_id` 是
    /// `task_id`，属于任务面板，后续接入）。`chunk_type` 语义见
    /// `docs/architecture/event-protocol.md` §4。
    pub(crate) fn handle_event(&mut self, ev: &serde_json::Value, cx: &mut Context<Self>) {
        if ev.get("type").and_then(|v| v.as_str()) != Some("chat_stream") {
            return;
        }
        let sid = ev
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        // 轮状态是**会话级全局**事实：即使不是当前会话也要记账，否则「切走时
        // 轮还在跑、切回后按钮却回到发送」——按钮态不能只活在当前会话的渲染里。
        // 这里只维护集合（不刷新界面），当前会话的渲染仍走下面 match 的分支。
        if ev.get("chunk_type").and_then(|v| v.as_str()) == Some("turn_state") {
            let running = ev
                .get("turn_state")
                .and_then(|t| t.get("state"))
                .and_then(|v| v.as_str())
                == Some("running");
            if running {
                self.running_sessions.insert(sid.to_string());
            } else {
                self.running_sessions.remove(sid);
            }
        }
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
                // 工具步收尾后的第一段正文 = 新的一步（新消息），与库里的「每步一条」对齐
                self.start_new_step_after_tools();
                self.append_delta(delta);
                // 上下文占用（`ContextRing` 的数据源）：usage 随 answer 块下发。
                // 分子取 `prompt_tokens`（本轮上下文）——`total_tokens` 含输出，
                // 当占用会偏高；分母优先用模型声明的窗口，这里只记流式兜底值。
                if let Some(u) = ev.get("usage") {
                    let prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    if prompt > 0 {
                        self.ctx_live = Some(MsgUsage {
                            prompt_tokens: prompt,
                            completion_tokens: u
                                .get("completion_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            cache_read: u.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                    if let Some(win) = u.get("context_window").and_then(|v| v.as_u64()) {
                        self.ctx_window_live = win;
                    }
                }
            }
            // 思考增量：取 **`thinking` 字段**（不是 `delta`）——vite
            // `chat-slice.appendThinking` 同口径：思考与正文分两个字段下发、分开渲染。
            //
            // 这行曾经是 `"thought" => {}`（注释「待独立渲染区」），于是流式期间
            // 完全看不到思考过程，只能在流结束后靠 `text` 里的 Markdown 引用块近似
            // ——那条路会踩 lazy continuation（见 `segments_body_text` 的注释）。
            "thought" => {
                let delta = ev
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                // 上一步已产出内容（正文 / 工具都出结果）→ 这是新一步的思考，新开消息
                self.start_new_step();
                self.append_thinking_delta(delta);
            }
            "tool_call" => {
                // 结构化工具卡（对齐 vite `appendToolCalls`）：**不再**把工具调用塞成正文
                // 文本——那样没有参数/结果/耗时，而且会和正文混在同一个 Markdown 段里。
                let Some(tc) = ev.get("tool_call") else {
                    return;
                };
                let name = tc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("工具")
                    .to_string();
                self.start_new_step_after_tools();
                self.append_tool_call(
                    tc.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name.clone(),
                    tc.get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tc.get("presentation")
                        .and_then(|v| v.as_str())
                        .unwrap_or("generic")
                        .to_string(),
                );
                // ask_user 是**同步工具**：工具执行挂起等回答，输入区要被追问接管
                if name == "ask_user" {
                    if let Some(args) = ev.get("tool_call").and_then(|v| v.get("arguments")) {
                        let parsed = match args.as_str() {
                            Some(s) => serde_json::from_str::<serde_json::Value>(s)
                                .unwrap_or(serde_json::Value::Null),
                            None => args.clone(),
                        };
                        let questions = parse_ask_user(&parsed);
                        if !questions.is_empty() {
                            self.clarify_answers = vec![String::new(); questions.len()];
                            self.clarification = Some(questions);
                        }
                    }
                }
            }
            "observation" => {
                // 工具执行结果：按 `tool_call_id` 回填调用卡片（耗时 / 成败 / 结果内容）。
                // 早期这里是空实现 —— 流式期与流结束后都看不到工具结果，只能等历史重载。
                if let Some(tr) = ev.get("tool_result") {
                    let id = tr
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    if !id.is_empty() {
                        self.apply_tool_result(
                            id,
                            tr.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(-1),
                            tr.get("success").and_then(|v| v.as_bool()).unwrap_or(true),
                            tr.get("error")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            tr.get("content")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        );
                    }
                }
            }
            // 消息边界：流末尾的 assistant 完整消息（含工具段）为权威全文，替换占位
            "message" => {
                if let Some(msg) = ev.get("message") {
                    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or_default();
                    let is_marker = msg
                        .get("compression_marker")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    match classify_boundary_message(role, is_marker) {
                        BoundaryMessageAction::ReplaceAssistant => {
                            self.apply_authoritative_message(msg)
                        }
                        // 早先这里只处理 assistant，其余一律丢弃 —— 于是后端「落库即
                        // 推送」（ADR-028 门控 `System || compression_marker`）推出的
                        // 压缩点和后台任务完成通知，在界面上一律没有。用户实测：
                        // 「上下文占比从 58% 掉到 2%，一个压缩点都没显示」。
                        BoundaryMessageAction::AppendBoundary => self.append_boundary_message(msg),
                        BoundaryMessageAction::Ignore => {}
                    }
                }
            }
            "error" => {
                let msg = ev
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                self.streaming = false;
                // 流式错误必须进 toast：否则下一次轮询写状态行就把它盖掉了
                self.set_error(format!("出错：{msg}"));
                self.append_delta(&format!("\n\n> ⚠️ {msg}\n"));
            }
            // ADR-031：用户消息落库确认 —— 把服务端真实 id 补到本地那条乐观行上。
            // 这个 chunk_type 早先没处理（落进 `_ => {}`），于是用户消息的 id 永远
            // 是空，`msg.rs` 里「仅已落库消息才给回退入口」的判断一直为假：
            // 发完消息想「回退到此」得重开会话才行。
            "user_message_id" => {
                let real = ev
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !real.is_empty() {
                    let mut next: Vec<MsgRow> = (*self.messages).clone();
                    // 从末尾找第一条还没 id 的 user 行（就是那条乐观渲染的占位）
                    if let Some(m) = next
                        .iter_mut()
                        .rev()
                        .find(|m| m.role == "user" && m.id.is_empty())
                    {
                        m.id = real.to_string();
                        self.messages = Rc::new(next);
                    }
                }
            }
            "turn_state" => {
                let state = ev
                    .get("turn_state")
                    .and_then(|v| v.get("state"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                self.streaming = state == "running";
                if !self.streaming {
                    self.set_status(format!("空闲 · {sid}"));
                    // 本轮结束：刷新底部统计行（token 汇总 + 最近一次请求性能）
                    self.load_session_usage(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// 推入一条空的 assistant 占位（本步骤的开始）。
    ///
    /// 为什么需要新建而不是“全部堆一条”：后端一轮对话是**多步**的（LLM 响应→工具→
    /// LLM 响应…），库里每个 assistant 响应是**独立一条消息**（实测四步就是 4 条）。
    /// 早期前端把整轮所有步堆在同一条占位里，于是：工具调用只能当正文塞成
    /// `` `🔧 name` `` 文本（没有参数/结果/耗时），而一旦任何地方把这条消息换成
    /// 后端的权威消息，前面几步就一起消失了（用户报的「流式结束后工具调用都不见了」）。
    fn push_assistant_row(&mut self) -> usize {
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
            usage: None,
            images: Vec::new(),
            ts: Some(now_clock()),
            id: String::new(),
            seq: 0,
        });
        self.trim_loaded_front(&mut next);
        let idx = next.len() - 1;
        self.messages = Rc::new(next);
        // ⚠️ 这里**不能用 `reset()`**：`ListState::reset` 会把
        // `logical_scroll_top` 清成 `None`，而它的「默认值」是**最后一项**
        // （`StateInner::logical_scroll_top` 的 `unwrap_or_else`）—— 等于把
        // 用户的滚动锚点悄悄挪到底部。于是即便随后不调 `scroll_to_end`，
        // 流式新开一步时用户的上滚位置也已经被抹掉了。
        // 改用 `splice` 在末尾**插入一项**：项数变了、现有锚点保持。
        let n = self.messages.len();
        self.msg_list_state.splice(n.saturating_sub(1)..n.saturating_sub(1), 1);
        self.scroll_to_end_if_following();
        self.message_total = self.messages.len();
        idx
    }

    /// 确保最后一条是 assistant（否则新建）。返回是否新建。
    fn ensure_assistant_tail(&mut self) -> bool {
        if self.messages.last().map(|m| m.role.as_str()) == Some("assistant") {
            return false;
        }
        self.push_assistant_row();
        true
    }

    /// 上一条 assistant 是否已经「收尾」（该给它后面开新消息了）。
    ///
    /// 判据（对齐 vite `chat-stream.ts` 的 `hasPrevTurn`，并补上「工具都已出结果」）：
    /// - 已有正文段 → 这一步的产出完成了；
    /// - 工具调用全部拿到结果 → 工具步收尾了（下一步 LLM 响应属于新消息）。
    ///
    /// 只有思考段、或有未出结果的工具 → 仍属当前步，继续往这条上追加。
    fn step_closed(m: &MsgRow) -> bool {
        if m.segs
            .iter()
            .any(|s| matches!(s, Seg::Text(t) if !t.trim().is_empty()))
        {
            return true;
        }
        !m.tool_calls.is_empty() && m.tool_calls.iter().all(|t| t.result.is_some())
    }

    /// 步边界：上一条已收尾（或根本不是 assistant）→ 新开一条 assistant 消息。
    fn start_new_step(&mut self) {
        let need = match self.messages.last() {
            None => true,
            Some(m) if m.role != "assistant" => true,
            Some(m) => Self::step_closed(m),
        };
        if need {
            self.push_assistant_row();
        }
    }

    /// `answer` / `tool_call` 用的步边界判据：只有「工具步已收尾」时才新开。
    ///
    /// 为什么不复用 [`Self::start_new_step`]：正文是**逐块**流下来的，若按
    /// 「已有正文段就新开」判定，第二块 delta 就会另起一条消息——一句回复被劈成几十条。
    fn start_new_step_after_tools(&mut self) {
        let need = match self.messages.last() {
            None => true,
            Some(m) if m.role != "assistant" => true,
            Some(m) => !m.tool_calls.is_empty() && m.tool_calls.iter().all(|t| t.result.is_some()),
        };
        if need {
            self.push_assistant_row();
        }
    }

    /// 追加一条**结构化**工具调用（`tool_call` 事件）→ `Seg::Tool` + `ToolCall` 占位。
    ///
    /// 结果由 `apply_tool_result` 按 id 回填。**不要**再退回「往正文里塞
    /// `` `🔧 name` `` 文本」——那条路上工具没有参数/结果/耗时，而且会和正文
    /// 混在同一个 Markdown 段里把渲染搞乱。
    pub(crate) fn append_tool_call(
        &mut self,
        id: String,
        name: String,
        arguments: String,
        presentation: String,
    ) {
        self.ensure_assistant_tail();
        if self.messages.last().is_none() {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].segs.push(Seg::Tool {
            id: id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            presentation: presentation.clone(),
        });
        next[idx].tool_calls.push(ToolCall {
            id,
            name,
            arguments,
            presentation,
            result: None,
            duration_ms: None,
            error: None,
        });
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.scroll_to_end_if_following();
    }

    /// 填充工具执行结果（`observation` / `tool_result` 事件）：按 id 回填卡片状态。
    ///
    /// 从最后一条往前找：工具结果总是来自**已收尾**的那一步，而当前可能已经新开了
    /// 一条（下一步的思考先到、结果后到），只认最后一条会丢。
    pub(crate) fn apply_tool_result(
        &mut self,
        tool_call_id: &str,
        duration_ms: i64,
        success: bool,
        error: Option<String>,
        content: Option<String>,
    ) {
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let mut hit: Option<usize> = None;
        'outer: for (mi, m) in next.iter_mut().enumerate().rev() {
            for t in m.tool_calls.iter_mut() {
                if t.id == tool_call_id {
                    t.duration_ms = Some(duration_ms);
                    if content.is_some() {
                        t.result = content.clone();
                    }
                    t.error = if success { None } else { error.clone() };
                    hit = Some(mi);
                    break 'outer;
                }
            }
        }
        if let Some(mi) = hit {
            self.messages = Rc::new(next);
            self.msg_list_state.remeasure_items(mi..mi + 1);
        }
    }

    /// 往最后一条 assistant 消息追加**正文**增量。
    ///
    /// 两份表示必须同步更新，否则渲染会漏内容：
    /// - `text`：纯正文（宽度估算 / 纯文本回退用）；
    /// - `segs` 末尾的 `Seg::Text`：**渲染真相源**——只更新 `text` 的话，
    ///   一旦 `segs` 里已经有思考段（`thought` 增量建的），渲染就走 segs 分支，
    ///   正文根本不显示。
    /// 流式更新时「跟随尾部」：**只有视图本来就在底部**才滚到底。
    ///
    /// 为什么：早先每个流式增量都无条件 `scroll_to_end()` —— 用户往上看历史时，
    /// 下一个 delta 就把他拖回底部，输出过程中根本没法上滚（用户实测）。
    ///
    /// 有意**不**在这里做「回到底部」按钮：用户上滚后新内容还在长，先保证不打扰；
    /// 需要提示时再加（那是纯增量）。
    pub(crate) fn scroll_to_end_if_following(&self) {
        if self.follows_tail() {
            self.msg_list_state.scroll_to_end();
        }
    }

    /// 视图此刻是否「跟着尾部」。
    ///
    /// ⚠️ **不能只用 `is_scrolled_to_end()`**（实测踩过）：它依赖
    /// `last_layout_bounds` + `summary.has_unknown_height` —— 虚拟列表在
    /// 滚到顶部/中部时，下方未渲染项的高度是未知的，于是它返回 `None`；
    /// 隐藏窗口（无布局）同样返回 `None`。把它当「在底部」处理，结果就是
    /// **永远跟随、上滚立刻被拖回**（正是用户报的现象）。
    ///
    /// 所以用两条判据：
    /// 1. `is_scrolled_to_end() == Some(true)` → 明确在底部；
    /// 2. 兜底：`logical_scroll_top().item_ix` 已经是最后一条（或更后）。
    ///    `item_ix` 只反映滚动锚点、**不依赖测量**，任何状态下都有效。
    pub(crate) fn follows_tail(&self) -> bool {
        if self.msg_list_state.is_scrolled_to_end() == Some(true) {
            return true;
        }
        let top = self.msg_list_state.logical_scroll_top().item_ix;
        top + 1 >= self.messages.len()
    }

    pub(crate) fn append_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        // 最后一条不是 assistant（刚发的用户消息 / system 通知）→ 新开一条，
        // 而不是把正文丢掉（早期直接 `return`，表现为「模型说话了但界面什么都不显示」）
        self.ensure_assistant_tail();
        if self.messages.last().is_none() {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].text = format!("{}{}", next[idx].text, delta).into();
        push_seg_delta(&mut next[idx].segs, delta, false);
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.scroll_to_end_if_following();
    }

    /// 往最后一条 assistant 消息追加**思考**增量（实时渲染可折叠思考块）。
    ///
    /// 思考**不进** `text`：那是正文口径（宽度估算按正文算，思考不该撑宽气泡）。
    pub(crate) fn append_thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.ensure_assistant_tail();
        if self.messages.last().is_none() {
            return;
        }
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        push_seg_delta(&mut next[idx].segs, delta, true);
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.scroll_to_end_if_following();
    }

    /// 用流末尾的**权威消息**替换占位（ADR-002：正文真相源是 `segments`）。
    ///
    /// 必须同时换掉 `segs` / `tool_calls` / `id` / `usage`——早期只换 `text`，
    /// 于是渲染继续走「纯文本兜底」路径：思考与正文的分块信息全丢，思考块只能
    /// 靠 Markdown 引用块近似，而引用块会把紧随其后的正文第一段并进去
    /// （lazy continuation）。顺带把 `id` 补上，否则流式结束后的消息仍然没有
    /// 回退入口（占位消息的 id 是空串）。
    pub(crate) fn apply_authoritative_message(&mut self, msg: &serde_json::Value) {
        if self.messages.last().map(|m| m.role.as_str()) != Some("assistant") {
            return;
        }
        let (segs, tool_calls) = parse_segments(msg);
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        let idx = next.len() - 1;
        next[idx].text = segments_body_text(msg).into();
        next[idx].segs = segs;
        next[idx].tool_calls = tool_calls;
        if let Some(u) = parse_msg_usage(msg) {
            next[idx].usage = Some(u);
        }
        if let Some(id) = msg
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            next[idx].id = id.to_string();
        }
        if let Some(ts) = parse_msg_ts(msg) {
            next[idx].ts = Some(ts);
        }
        self.messages = Rc::new(next);
        self.msg_list_state.remeasure_items(idx..idx + 1);
        self.scroll_to_end_if_following();
    }

    /// 边界消息（压缩点 / System 通知）→ **追加成新行**（按 id 幂等）。
    ///
    /// 为什么不复用 `apply_authoritative_message`：那个只把内容塞进「最后一条
    /// assistant 占位」，且前提是最后一条确实是 assistant。压缩点在用户刚发消息
    /// 之后到达（此时最后一条是 user），于是被整条丢弃——用户实测「上下文占比从
    /// 58% 掉到 2%，一个压缩点都没显示」。System 通知（后台任务完成）同理。
    pub(crate) fn append_boundary_message(&mut self, msg: &serde_json::Value) {
        let id = msg
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        // 幂等：推送可能重放（重连快照 + 实时），同 id 不重复追加
        if !id.is_empty() && self.messages.iter().any(|m| m.id == id) {
            return;
        }
        let row = parse_msg_row(msg);
        let mut next: Vec<MsgRow> = (*self.messages).clone();
        next.push(row);
        // 与流式追加同一口径：超窗口丢最早的（压缩点的补齐由 `render_window` 负责）
        self.trim_loaded_front(&mut next);
        self.messages = Rc::new(next);
        self.msg_list_state.reset(self.messages.len());
        self.scroll_to_end_if_following();
        self.message_total = self.messages.len();
    }

    /// 发送输入框内容（Enter 或「发送」按钮）
    pub(crate) fn send_current_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        // 发送后输入框已清空（上面 set_value("")），不再需要删草稿条目：
        // 文本随实体走（每会话一个 `Entity<TextareaState>`），图片被 `send_text`
        // 里的 `mem::take(&mut self.composer_images)` 取走。
        self.send_text(text, cx);
    }

    /// 发送一段文本：`POST /chat/stream` 只是开关（ADR-028），输出走常驻 SSE。
    pub(crate) fn send_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        // 待发送图片随本条消息一起走（发送即清空草稿）
        let pending_images = std::mem::take(&mut self.composer_images);
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
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
            usage: None,
            // 本地占位直接带图：发送后立刻能看到自己贴的图（历史重载后仍可见）
            images: pending_images.clone(),
            // 用户消息发送即打点（vite 同样在本地取时间；后端 `timestamp` 恒空）
            ts: Some(now_clock()),
            // 本地占位：尚未落库，无服务端 id（回退入口据此隐藏）
            id: String::new(),
            seq: 0,
        });
        next.push(MsgRow {
            role: "assistant".to_string(),
            text: "".into(),
            segs: Vec::new(),
            tool_calls: Vec::new(),
            compression: false,
            usage: None,
            images: Vec::new(),
            // 与 vite `startNewAssistantTurn` 同口径：本轮 assistant 开始时打点
            ts: Some(now_clock()),
            id: String::new(),
            seq: 0,
        });
        self.trim_loaded_front(&mut next);
        self.messages = Rc::new(next);
        self.msg_list_state.reset(self.messages.len());
        self.msg_list_state.scroll_to_end();
        self.message_total = self.messages.len();
        self.streaming = true;
        // 与 `turn_state` 事件同一份记账：服务端的 running 事件到达前按钮就该是
        // 「停止」（否则这一小段窗口里用户点按钮会重复发送）
        self.running_sessions.insert(session_id.clone());
        // `body` 会把 `session_id` move 走，失败分支清理集合需要自己留一份
        let sid_for_cleanup = session_id.clone();
        self.set_status(format!("生成中… · {session_id}"));

        let port = self.port;
        // 图片走 `message.images`（data URL 数组；后端 `CoreMessage::user_with_images`）
        // ADR-031 乐观渲染定位键：后端落库后经 `user_message_id` 确认事件回显**真实
        // id**（回退/删除按它定位）。早先没带这个字段，后端就不发确认事件 →
        // 用户消息的 id 永远是空 → 「回退到此」要重开会话才出现（实测：发完消息
        // 想立刻撤销做不到）。
        let local_umid = format!("umid_{}", chrono::Utc::now().timestamp_millis());
        let mut message = serde_json::json!({
            "role": "user",
            "content": text,
            "user_message_id": local_umid,
        });
        if !pending_images.is_empty() {
            message["images"] = serde_json::json!(pending_images);
        }
        let mut body = serde_json::json!({
            "session_id": session_id,
            "message": message,
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 2048
        });
        // 会话级模型：随首条消息一起下发——服务端建会话时就固化，不必等会话 id 生成后再补 PUT
        if let Some((provider, model)) = &self.session_model {
            body["model"] = serde_json::Value::String(model.clone());
            body["provider"] = serde_json::Value::String(provider.clone());
        }
        // 「＋新目录」设置的工作目录：只作用于这一条消息——服务端建会话时固化，
        // 之后的消息再带就没意义了（所以 `take()` 掉）。
        if let Some(dir) = self.new_session_workspace.take() {
            body["working_directory"] = serde_json::Value::String(dir);
        }
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
                        this.running_sessions.remove(&sid_for_cleanup);
                        this.set_error(format!("发送失败：{e}"));
                        this.append_delta(&format!("\n\n> ⚠️ 发送失败：{e}\n"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 重算「是否该显示首次启动引导」。
    ///
    /// 判据是**有没有可用对话模型**（`models` 里 `is_chat=true` 的那个），不是
    /// `GET /config/status` 的 `configured`：
    /// - 内嵌 server 起来后配置文件往往已存在（`config_exists()` 恒真）；
    /// - 自定义 `TIANYAN_CONFIG` 路径时那个判据根本看不到这份配置（实测：
    ///   指向不存在的路径启动，`configured` 仍是 true）；
    /// - 而「一个对话模型都没有」是用户真正不能用的状态。
    ///
    /// `setup_dismissed` 是逃生舱：用户点过「仍然进入应用」就不再拦。
    pub(crate) fn recompute_setup(&mut self) {
        self.needs_setup = !self.setup_dismissed && !self.models.iter().any(|m| m.is_chat);
    }

    pub(crate) fn load_messages(&mut self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        println!("[消息] 加载会话 {session_id}");
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages?limit={MESSAGE_PAGE}"
            );
            match fetch_json(url).await {
                Ok(body) => {
                    let arr = body
                        .get("messages")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    // 上滚游标与「还有更早」都由服务端随页下发（ADR-035 §8）。
                    let cursor = body.get("next_before_seq").and_then(|v| v.as_i64());
                    let more = body
                        .get("has_more")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let rows: Vec<MsgRow> = arr
                        .iter()
                        .map(parse_msg_row)
                        .collect();
                    let n = rows.len();
                    println!("[消息] 首屏 {n} 条（has_more={more}）");
                    let total = n;
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            // 请求期间切走了 → 丢弃（否则上一个会话的消息会顶掉当前会话的）
                            if this.is_stale_for(&session_id) {
                                return;
                            }
                            this.messages = Rc::new(rows);
                            // 条数变了必须重建 list state（item_count 在构造时固定）
                            this.msg_list_state = GpuiListState::new(
                                this.messages.len(),
                                ListAlignment::Bottom,
                                px(120.),
                            );
                            this.message_total = total;
                            this.oldest_seq = cursor;
                            this.has_more_history = more;
                            this.loading_older = false;
                            // 标题要如实说明「只是已加载的一段」：分页后 total 是当前
                            // 窗口条数，不是会话总条数（早先写「会话消息（50 条）」会
                            // 让人以为整个会话只有 50 条）
                            this.current_title = if more {
                                format!("会话消息（已加载 {total} 条 · 还有更早）")
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

    /// 上滚加载更早一页（`?before_seq=<oldest_seq>`），前插并保持阅读位置。
    ///
    /// 与 Web 版 `use-session-history.loadOlder` 同口径：页长 [`MESSAGE_PAGE`]、
    /// 游标用服务端下发的 `next_before_seq`、`has_more=false` 就不再请求。
    /// 早先的做法是「一次拉 200 条、只渲染 60 条」——更早的历史永远看不到，
    /// 拉回来的 140 条还白费。
    pub(crate) fn load_older_messages(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || !self.has_more_history {
            return;
        }
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let Some(cursor) = self.oldest_seq else {
            return;
        };
        if self.messages.len() >= MAX_LOADED_MESSAGES {
            self.set_status(format!("已加载 {MAX_LOADED_MESSAGES} 条，不再往前加载"));
            cx.notify();
            return;
        }
        self.loading_older = true;
        let port = self.port;
        self.set_status("正在加载更早的消息…".to_string());
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/messages?limit={MESSAGE_PAGE}&before_seq={cursor}"
            );
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_older = false;
                // 加载期间切走了 → 丢弃这一页（否则会把上一个会话的更早消息前插进来）
                if this.is_stale_for(&sid) {
                    return;
                }
                let body = match res {
                    Ok(b) => b,
                    Err(e) => {
                        this.set_error(format!("加载更早的消息失败：{e}"));
                        cx.notify();
                        return;
                    }
                };
                let arr = body
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let older: Vec<MsgRow> = arr.iter().map(parse_msg_row).collect();
                // 按 id 去重（服务端页边界可能重叠）：保留前插的那份
                let known: std::collections::HashSet<String> = this
                    .messages
                    .iter()
                    .map(|m| m.id.clone())
                    .filter(|id| !id.is_empty())
                    .collect();
                let mut merged: Vec<MsgRow> = older
                    .into_iter()
                    .filter(|m| m.id.is_empty() || !known.contains(&m.id))
                    .collect();
                let added = merged.len();
                merged.extend((*this.messages).iter().cloned());
                this.oldest_seq = body.get("next_before_seq").and_then(|v| v.as_i64());
                this.has_more_history = body
                    .get("has_more")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                this.messages = Rc::new(merged);
                // 重建列表（item_count 构造时固定），并把视线留在**原来那条**上：
                // 前插了 added 条，原来的第一条现在在索引 added
                this.msg_list_state =
                    GpuiListState::new(this.messages.len(), ListAlignment::Bottom, px(120.));
                if added > 0 {
                    this.msg_list_state.scroll_to(gpui_kit::ListOffset {
                        item_ix: added,
                        offset_in_item: px(0.),
                    });
                }
                this.message_total = this.messages.len();
                // 标题跟着走（「还有更早」随 has_more 出现/消失）
                this.current_title = if this.has_more_history {
                    format!(
                        "会话消息（已加载 {} 条 · 还有更早）",
                        this.messages.len()
                    )
                } else {
                    format!("会话消息（{} 条）", this.messages.len())
                };
                this.set_status(if added > 0 {
                    format!("已加载 {added} 条更早的消息")
                } else {
                    "没有更早的消息了".to_string()
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// 显式停止当前轮：`POST /chat/streams/{id}/cancel`（ADR-036 的结构性取消
    /// 在服务端置位，客户端断线**不**取消——本地助手跑完再取）。
    pub(crate) fn stop_current_turn(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        self.set_status("正在停止…".to_string());
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/streams/{sid}/cancel");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => this.set_status(format!("已请求停止 · {sid}")),
                    Err(e) => this.set_error(format!("停止失败：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 切页：**侧栏点击与自动化 `switch_tab` 共用这一份**。
    ///
    /// 「首次进入才补拉」的逻辑必须住在这里。早先它内联在侧栏的 `on_click` 闭包
    /// 里，于是点侧栏会拉数据、而自动化 `switch_tab` 只改 `view` 不拉 —— 两条
    /// 路径分叉，验证结论随即失真（ADR-052 D8 的实例，实测踩过：文件页树一直空）。
    /// 切换外观：改全局色板 + 同步组件库 `ThemeMode` + 持久化 + 重绘。
    ///
    /// 组件库控件（`Input` / `Textarea` / 按钮）走它自己的 `Theme`，不同步切就会
    /// 出现「天演是羊皮纸、输入框还是深灰」的割裂。
    ///
    /// `window` 为 `None` 时（自动化端口调用）只改全局主题，不碰窗口局部状态。
    pub(crate) fn apply_theme_with(
        &mut self,
        kind: ThemeKind,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        set_theme(kind);
        Theme::change(
            match kind {
                ThemeKind::Parchment => ThemeMode::Light,
                _ => ThemeMode::Dark,
            },
            window,
            cx,
        );
        save_theme_pref(kind);
        cx.notify();
    }

    // 内建截图依赖 gpui 的 `test-support`（`Window::render_to_image` 是 cfg 门控的），
    // 所以本方法也必须同样门控 —— 否则默认构建找不到那个方法。
    #[cfg(feature = "screenshot")]
    pub(crate) fn screenshot_to(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let handle = self.window_handle;
        let target = path.clone();
        let size = handle
            .update(cx, move |_, window, _cx| {
                let img = window
                    .render_to_image()
                    .map_err(|e| format!("渲染当前帧失败：{e}"))?;
                let (w, h) = (img.width(), img.height());
                img.save(&target)
                    .map_err(|e| format!("写 PNG 失败（{target}）：{e}"))?;
                Ok::<(u32, u32), String>((w, h))
            })
            .map_err(|e| format!("窗口已关闭，无法截图：{e}"))??;

        self.last_screenshot = Some((path, size.0, size.1));
        cx.notify();
        Ok(())
    }

    /// 拉本会话的 token 汇总与最近一次请求性能（输入卡片底部统计行）。
    ///
    /// 对应 Web 版两个接口：
    /// - `/usage/stats?days=0&session_id=` 按会话聚合 `usage_logs`（全时段，与消息分页无关）
    /// - `/usage/recent?limit=1&session_id=` 最近一条样本（ttft / 时长 / 输出 token）
    ///
    /// 失败**静默保留旧值**：统计条是观测面，不能因一次抖动把已有数字清空。
    /// 异步回调落地前的会话守卫。
    ///
    /// 请求发出后用户可能已经切走——此时**必须丢弃**这份结果，否则上一个会话的
    /// 数据会写进当前会话（实测场景：快速连点两个会话，第一个的响应后到，
    /// 消息列表 / 待办 / 审批 / 用量就会短暂串台，看到的是别人的内容）。
    /// 只认 `current_session`，不看 `open_tabs`：Tab 里留着不等于「还看着它」。
    pub(crate) fn is_stale_for(&self, session: &str) -> bool {
        self.current_session.as_deref() != Some(session)
    }

    /// 写状态行（**唯一入口**）。
    ///
    /// 为什么收成一个方法：早先 28 处直接 `self.status = …` 散落各处，其中错误路径
    /// 常被下一次轮询覆盖掉。收敛之后语义清晰——**普通状态走这里**，
    /// **错误走 [`Self::set_error`]**（同时进 toast，粘住 10s）。
    pub(crate) fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    /// 推一条瞬时提示（右上角 toast）。
    ///
    /// 同文本去重：3s 轮询会把同一个错误反复写进来，不去重就是刷屏。
    /// 最多留 5 条（超出丢最早的）。
    pub(crate) fn push_notice(&mut self, text: impl Into<String>, is_err: bool) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        if self
            .notices
            .iter()
            .any(|n| n.text == text && n.is_err == is_err)
        {
            return;
        }
        self.notices.push(Notice {
            text,
            is_err,
            at: std::time::Instant::now(),
        });
        if self.notices.len() > 5 {
            self.notices.remove(0);
        }
    }

    /// 错误：状态行**照旧更新**（既有断言/状态栏兼容）+ 进 toast 粘住 10s。
    ///
    /// 为什么两者都要：状态行会被下一次轮询覆盖，toast 不会——错误必须留得住。
    pub(crate) fn set_error(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        self.set_status(m.clone());
        self.push_notice(m, true);
    }

    /// 清掉过期提示（由 3s 轮询 tick 调用）。
    pub(crate) fn expire_notices(&mut self) {
        self.notices.retain(|n| {
            let ttl = if n.is_err { 10 } else { 4 };
            n.at.elapsed() < std::time::Duration::from_secs(ttl)
        });
    }

    /// 裁掉超上限的**最早**消息，并把上滚游标同步到新的首条。
    ///
    /// 为什么必须同步游标：`oldest_seq` 是「加载更早」的 `before_seq`。头部被裁掉
    /// 后若不更新，下一次加载会用失效的老游标再拉一遍**已经被丢掉**的那段（被 id
    /// 去重挡住，但白跑一次请求，用户点「加载更早」看起来没反应）。
    /// 游标取 `MsgRow.seq`（与数据同源），而不是响应里的 `next_before_seq`
    /// —— 后者在裁剪之后就失准了。
    fn trim_loaded_front(&mut self, next: &mut Vec<MsgRow>) {
        if next.len() <= crate::MAX_LOADED_MESSAGES {
            return;
        }
        let drop_n = next.len() - crate::MAX_LOADED_MESSAGES;
        next.drain(..drop_n);
        if let Some(first) = next.first() {
            if first.seq > 0 {
                self.oldest_seq = Some(first.seq);
                // 前面确实还有（我们刚把它裁掉了），上滚入口保持可见
                self.has_more_history = true;
            }
        }
    }

    pub(crate) fn load_session_usage(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let stats = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days=0&session_id={sid}"
            ))
            .await
            .ok();
            let recent = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/recent?limit=1&session_id={sid}"
            ))
            .await
            .ok();
            let usage = stats
                .as_ref()
                .and_then(|b| b.get("total"))
                .map(|t| {
                    (
                        t.get("uncached_input")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        t.get("cached_input").and_then(|v| v.as_u64()).unwrap_or(0),
                        t.get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    )
                })
                .filter(|(a, b, c)| a + b + c > 0);
            let perf = recent
                .as_ref()
                .and_then(|b| b.get("samples"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|s| {
                    let duration = s.get("duration_ms").and_then(|v| v.as_i64())?;
                    if duration < 0 {
                        return None;
                    }
                    let ttft = s
                        .get("ttft_ms")
                        .and_then(|v| v.as_i64())
                        .filter(|v| *v >= 0);
                    let completion = s
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    Some((ttft, duration, completion))
                });
            let _ = this.update(cx, |this, cx| {
                // 切走了就丢弃（否则上一个会话的用量 / 性能写进当前会话）
                if this.is_stale_for(&sid) {
                    return;
                }
                this.session_usage = usage;
                this.request_perf = perf;
                cx.notify();
            });
        })
        .detach();
    }
}

/// 把增量并入 `segs` 末尾同类段；类型不同就新起一段。
///
/// 段合并规则与 Web 版一致（`MessageSegments.tsx`：连续 `thinking` 合并成一个
/// 折叠块、连续 `text` 合并成一个 Markdown 块）——流式增量天然是「一段连续
/// 同类型」，所以这里只需看最后一段。
fn push_seg_delta(segs: &mut Vec<Seg>, delta: &str, thinking: bool) {
    match (thinking, segs.last_mut()) {
        (true, Some(Seg::Thinking(t))) => t.push_str(delta),
        (false, Some(Seg::Text(t))) => t.push_str(delta),
        _ => segs.push(if thinking {
            Seg::Thinking(delta.to_string())
        } else {
            Seg::Text(delta.to_string())
        }),
    }
}

/// 一条服务端消息 → 界面行。
///
/// 历史首屏、上滚分页、边界事件（压缩点 / System 通知）**共用这一份**口径——
/// 三处各写一遍字段映射，迟早漂移出「历史里有、推送来的没有」这类问题。
pub(crate) fn parse_msg_row(m: &serde_json::Value) -> MsgRow {
    let (segs, tool_calls) = parse_segments(m);
    MsgRow {
        id: m
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        seq: m.get("seq").and_then(|v| v.as_i64()).unwrap_or(0),
        role: m
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        text: segments_body_text(m).into(),
        segs,
        tool_calls,
        compression: m
            .get("compression_marker")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        usage: parse_msg_usage(m),
        images: parse_msg_images(m),
        ts: parse_msg_ts(m),
    }
}

/// `message` 边界事件该走哪条路径（结构判据，单测钉住）。
///
/// 三种消息混在同一个事件名里，早先只处理了第一种，于是压缩点和 System 通知
/// 被静默丢弃（用户实测「58% → 2%，一个压缩点都没有」）。
#[derive(Debug, PartialEq)]
pub(crate) enum BoundaryMessageAction {
    /// 流末尾的 assistant 全文 → 替换最后一条占位（内容与工具段以它为准）
    ReplaceAssistant,
    /// 压缩点（user 锚定但带 `compression_marker`）/ System 通知 → **追加新行**
    AppendBoundary,
    /// 普通 user 等 → 不动（乐观渲染已经显示过一次，重复插入就是两条）
    Ignore,
}

/// 判定一条 `message` 事件的落点。
pub(crate) fn classify_boundary_message(role: &str, is_marker: bool) -> BoundaryMessageAction {
    if role == "assistant" {
        BoundaryMessageAction::ReplaceAssistant
    } else if is_marker || role == "system" {
        BoundaryMessageAction::AppendBoundary
    } else {
        BoundaryMessageAction::Ignore
    }
}

#[cfg(test)]
mod step_boundary_tests {
    //! 「一步一条消息」判据的回归（用户报的「流式结束后工具调用全不见」的根因）。
    //!
    //! 为什么用单测而不是 UI 断言：这条逻辑只看数据结构（segs / tool_calls 的形态），
    //! 而真实流式要等 LLM 返回、还要截图比对边界，成本高且不稳定。判据本身是纯函数，
    //! 单测能钉住四类形态。
    // ⚠️ 不要写 `use super::*`：bin crate 根是 `main.rs`，glob 会拉进整棵模块树，
    // `#[test]` 宏展开时直接撞 `recursion_limit`（实测报
    // “recursion limit reached while expanding `#[test]`”）。只显式导入需要的类型。
    use crate::app::{classify_boundary_message, BoundaryMessageAction};
    use crate::types::{MsgRow, Seg, ToolCall};
    use crate::ClientApp;

    fn tool(id: &str, done: bool) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "read_file".to_string(),
            arguments: "{}".to_string(),
            presentation: "read".to_string(),
            result: if done { Some("ok".to_string()) } else { None },
            duration_ms: if done { Some(12) } else { None },
            error: None,
        }
    }

    fn row(segs: Vec<Seg>, tools: Vec<ToolCall>) -> MsgRow {
        MsgRow {
            id: String::new(),
            seq: 0,
            role: "assistant".to_string(),
            text: "".into(),
            segs,
            tool_calls: tools,
            compression: false,
            usage: None,
            images: Vec::new(),
            ts: None,
        }
    }

    #[test]
    fn empty_placeholder_is_not_closed() {
        assert!(!ClientApp::step_closed(&row(Vec::new(), Vec::new())));
    }

    #[test]
    fn boundary_event_assistant_replaces_placeholder() {
        assert_eq!(
            classify_boundary_message("assistant", false),
            BoundaryMessageAction::ReplaceAssistant
        );
    }

    #[test]
    fn boundary_event_compression_marker_is_appended() {
        // 压缩点是 user 锚定的——**不能**因为它 role=user 就当成乐观渲染过的用户消息
        assert_eq!(
            classify_boundary_message("user", true),
            BoundaryMessageAction::AppendBoundary,
            "压缩点必须追加成新行，否则界面永远看不到它"
        );
    }

    #[test]
    fn boundary_event_system_notice_is_appended() {
        assert_eq!(
            classify_boundary_message("system", false),
            BoundaryMessageAction::AppendBoundary,
            "后台任务完成通知也要显示"
        );
    }

    #[test]
    fn boundary_event_plain_user_is_ignored() {
        // 普通用户消息已由乐观渲染显示过，再追加就是两条
        assert_eq!(
            classify_boundary_message("user", false),
            BoundaryMessageAction::Ignore
        );
    }

    #[test]
    fn thinking_only_is_not_closed() {
        let m = row(vec![Seg::Thinking("想一想".into())], Vec::new());
        assert!(!ClientApp::step_closed(&m), "思考还在流，不能开新消息");
    }

    #[test]
    fn text_closes_step() {
        let m = row(vec![Seg::Text("答案".into())], Vec::new());
        assert!(ClientApp::step_closed(&m), "已有正文 → 下一步该是新消息");
    }

    #[test]
    fn pending_tool_keeps_step_open() {
        let m = row(
            vec![Seg::Tool {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
                presentation: "read".into(),
            }],
            vec![tool("c1", false)],
        );
        assert!(
            !ClientApp::step_closed(&m),
            "工具还没出结果 → 同一批并行工具不该被拆开"
        );
    }

    #[test]
    fn finished_tools_close_step() {
        let m = row(
            vec![Seg::Tool {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
                presentation: "read".into(),
            }],
            vec![tool("c1", true)],
        );
        assert!(
            ClientApp::step_closed(&m),
            "工具出结果 → 下一步是新的 LLM 响应"
        );
    }
}
