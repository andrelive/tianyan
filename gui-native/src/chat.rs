//! 会话页：消息列 + 输入卡片 + 上下文环 + 模型/思考选择器。
//!
//! 布局三段平级挂 `flex_row`（左列表 / 右聊天 / 右任务面板），任务面板不是右栏的
//! 子节点——改挂载点前先跑括号栈快照，编译通过不代表挂对了父节点。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::types::View;
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::list as gpui_list;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    /// 最近一轮请求的用量 → 上下文占用的分子。
    ///
    /// 两条来源（与 gui-vite `lib/token-usage.lastMessageUsage` 同口径）：
    /// ① 流式 `usage` 上报（`ctx_live`，最新一轮；切会话即清）；
    /// ② 消息链里每条消息自带的 `usage`——从末尾向前找第一条，让**切会话 /
    ///    重开应用**也能立刻显示占用，而不必等下一轮对话才开始有数。
    ///
    /// 压缩摘要消息特殊处理：它的 `prompt_tokens` 是压缩请求的输入（≈ 压缩前
    /// 上下文），不能当占用；改用「系统前缀基线（会话首条请求 input）+ 摘要输出」
    /// 估算压缩后占用。
    pub(crate) fn ctx_usage(&self) -> Option<MsgUsage> {
        // 流式实时值优先：它比库里最后一条更「当轮」（本轮 tool 循环的最后一跳）
        if let Some(u) = self.ctx_live {
            return Some(u);
        }
        for m in self.messages.iter().rev() {
            let Some(u) = m.usage else { continue };
            if !m.compression {
                return Some(u);
            }
            let baseline = self
                .sessions_data
                .iter()
                .find(|s| Some(&s.id) == self.current_session.as_ref())
                .and_then(|s| s.first_request_tokens)
                .or_else(|| {
                    // 后端没给基线（旧接口）时退回「窗口内第一条 assistant 的
                    // prompt_tokens」——仅当窗口包含会话开头才近似成立
                    self.messages
                        .iter()
                        .find(|x| x.role == "assistant" && x.usage.is_some())
                        .and_then(|x| x.usage)
                        .map(|x| x.prompt_tokens)
                })
                .unwrap_or(0);
            return Some(MsgUsage {
                prompt_tokens: baseline + u.completion_tokens,
                completion_tokens: u.completion_tokens,
                cache_read: u.cache_read,
            });
        }
        None
    }

    /// 上下文窗口（分母）：优先当前 chat 模型声明的 `context_length`（vite 同口径
    /// `chatModels.context_length`），否则用流式上报的窗口兜底。
    pub(crate) fn ctx_window(&self) -> u64 {
        self.current_model_context().unwrap_or(self.ctx_window_live)
    }

    /// 当前 chat 模型的声明上下文窗口：会话级模型优先，回退 `preferences.chat`
    /// （`load_models` 已把默认 chat 模型标成 `is_chat`）。
    pub(crate) fn current_model_context(&self) -> Option<u64> {
        if let Some((provider, model)) = &self.session_model {
            if let Some(row) = self
                .models
                .iter()
                .find(|m| &m.provider == provider && &m.name == model)
            {
                return row.context;
            }
        }
        self.models
            .iter()
            .find(|m| m.is_chat)
            .and_then(|m| m.context)
    }

    /// 上下文占用百分比（0-100；无用量时也是 0，用 `ctx_usage()` 区分「没有数据」）。
    ///
    /// 超过窗口时夹到 100、并按四舍五入取整（与 vite `ContextRing` 的
    /// `Math.round(Math.min(100, …))` 同口径）——截断会把 0.9% 显示成 0%，
    /// 看着像「还没有数据」。
    pub(crate) fn ctx_pct(&self) -> u64 {
        let window = self.ctx_window();
        let Some(used) = self.ctx_usage() else {
            return 0;
        };
        if window == 0 {
            return 0;
        }
        let capped = used
            .prompt_tokens
            .saturating_mul(100)
            .min(window.saturating_mul(100));
        (capped + window / 2) / window
    }

    /// 上下文占用配色（对齐 DSH ContextMeter / vite `ContextRing`）：
    /// 临界 80% → error，高 50% → warning，正常 → success。
    pub(crate) fn ctx_color(&self) -> u32 {
        let pct = self.ctx_pct();
        if pct >= 80 {
            colors::ERROR()
        } else if pct >= 50 {
            colors::WARNING()
        } else {
            colors::SUCCESS()
        }
    }

    /// 手动压缩当前会话（`POST /sessions/{id}/compress`）。
    ///
    /// 与 vite `ChatPanel.handleCompress` 同语义：流式中 / 已压缩中不重入；
    /// 完成后刷新消息链（压缩点消息 + 上下文变短）与会话 token 汇总。
    pub(crate) fn compress_session(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        if self.ctx_compressing || self.streaming {
            return;
        }
        self.ctx_compressing = true;
        self.ctx_compress = None;
        cx.notify();
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/compress");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                this.ctx_compressing = false;
                match res {
                    Ok(body) => {
                        let compressed = body
                            .get("compressed")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        this.ctx_compress = Some((
                            true,
                            if compressed {
                                "已压缩".to_string()
                            } else {
                                "无需压缩（消息太少）".to_string()
                            },
                        ));
                        // 压缩改的是消息链：重拉消息 / 汇总 / 会话列表（首条基线）
                        this.load_messages(sid.clone(), cx);
                        this.load_session_usage(cx);
                        this.load_sessions(cx);
                    }
                    Err(e) => this.ctx_compress = Some((false, format!("压缩失败：{e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 输入卡片底部显示的当前 chat 模型（`模型 · provider`）。
    ///
    /// 口径与 Tauri 版 `ModelSelector` 一致：未选中时回退到 `preferences.chat`
    /// （`load_models` 已把它标成 `is_chat`），再没有就显示占位。
    pub(crate) fn current_model_label(&self) -> String {
        match self.current_model_pair() {
            Some((provider, model)) => format!("{model} · {provider}"),
            None => "未选择模型".to_string(),
        }
    }

    /// 当前会话生效的 `(provider, model)`：会话选择优先，回退默认 chat 模型。
    pub(crate) fn current_model_pair(&self) -> Option<(String, String)> {
        if let Some(pair) = &self.session_model {
            return Some(pair.clone());
        }
        let m = self.models.iter().find(|m| m.is_chat)?;
        Some((m.provider.clone(), m.name.clone()))
    }

    /// 当前 chat 模型行（思考档位 / 上下文窗口都从这里取）。
    pub(crate) fn current_chat_model(&self) -> Option<&ModelRow> {
        if let Some((provider, model)) = &self.session_model {
            if let Some(row) = self
                .models
                .iter()
                .find(|m| &m.provider == provider && &m.name == model)
            {
                return Some(row);
            }
        }
        self.models.iter().find(|m| m.is_chat)
    }

    /// 思考强度候选档位；`None` = 当前模型不支持思考 → 选择器整体隐藏。
    ///
    /// 顺序 = 「default」（值 `off`）+ 模型声明的档位（去掉重复的 `off`）。
    /// 档位值**原样展示不翻译**——厂商可能给 low / high / max / think 等任意值。
    pub(crate) fn thinking_options(&self) -> Option<Vec<String>> {
        let declared: Vec<String> = self
            .current_chat_model()?
            .reasoning_efforts
            .iter()
            .filter(|v| v.as_str() != "off")
            .cloned()
            .collect();
        if declared.is_empty() {
            return None;
        }
        let mut out = vec!["off".to_string()];
        out.extend(declared);
        Some(out)
    }

    /// 思考强度展示名：`off` 如实显示为「default」
    /// （不附加思考参数，是否思考由模型决定——思考模型缺省仍会输出推理）。
    pub(crate) fn thinking_label(&self) -> String {
        self.thinking
            .clone()
            .unwrap_or_else(|| "default".to_string())
    }

    /// 选择思考强度：写**当前会话**的模型选择（与模型选择同一个对象，
    /// `PUT /sessions/{id}/model` 带 `thinking`）。未创建会话时只记本地，
    /// 首条消息的 `ChatRequest` 会带上并固化。
    pub(crate) fn select_thinking(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        self.thinking_menu_open = false;
        self.model_menu_open = false;
        self.thinking = effort.clone();
        let Some(sid) = self.current_session.clone() else {
            cx.notify();
            return;
        };
        let Some((provider, model)) = self.current_model_pair() else {
            cx.notify();
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let mut body = serde_json::json!({ "provider": provider, "model": model });
            // 缺省 = off（不附加思考参数）
            if let Some(e) = effort {
                body["thinking"] = serde_json::Value::String(e);
            }
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("思考强度已设置：{}", this.thinking_label()),
                    Err(e) => format!("设置思考强度失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉会话级模型选择（`GET /sessions/{id}/model`）→ 恢复模型与思考强度。
    ///
    /// 不恢复的话，切到「设过 high」的会话会显示 default——用户一改就把会话里
    /// 的档位覆盖掉（显示与实际不符，比不显示更糟）。
    pub(crate) fn load_session_model(&self, session_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/model");
            let Ok(body) = fetch_json(url).await else {
                return;
            };
            let sel = body.get("selection");
            let provider = sel
                .and_then(|s| s.get("provider"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let model = sel
                .and_then(|s| s.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let thinking = sel
                .and_then(|s| s.get("thinking"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let _ = this.update(cx, |this, cx| {
                // 迟到响应不覆盖：请求在飞时会话可能已切走
                if this.current_session.as_deref() != Some(session_id.as_str()) {
                    return;
                }
                if let (Some(p), Some(m)) = (provider, model) {
                    this.session_model = Some((p, m));
                }
                this.thinking = thinking;
                cx.notify();
            });
        })
        .detach();
    }

    /// 选择模型：写**当前会话**的模型（`PUT /sessions/{id}/model`）。
    ///
    /// 与 Tauri 版 `ModelSelector.selectModel` 同语义——**只改会话，不改全局默认**
    /// （全局默认由设置页的 `POST /config/models/switch` 负责）。
    /// 未创建会话时先记在本地：首条消息的 `ChatRequest` 会带上 `model`/`provider`，
    /// 服务端建会话时即固化，不必等会话 id 生成后再补一次 PUT。
    pub(crate) fn select_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        self.ctx_panel_open = false;
        self.session_model = Some((provider.clone(), model.clone()));
        // 新模型没声明当前档位 → 复位 default（与 vite `ThinkingSelect` 的
        // useEffect 兜底同逻辑；否则会把一个该模型不认识的档位发给后端）
        if let Some(cur) = &self.thinking {
            let supported = self
                .models
                .iter()
                .find(|m| m.provider == provider && m.name == model)
                .map(|m| m.reasoning_efforts.iter().any(|v| v == cur))
                .unwrap_or(false);
            if !supported {
                self.thinking = None;
            }
        }
        let Some(sid) = self.current_session.clone() else {
            self.status = format!("新会话将使用 {model} · {provider}");
            cx.notify();
            return;
        };
        let thinking = self.thinking.clone();
        let port = self.port;
        self.status = format!("正在为会话切换模型到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let mut body = serde_json::json!({ "provider": provider, "model": model });
            if let Some(e) = thinking {
                body["thinking"] = serde_json::Value::String(e);
            }
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("会话模型已切换：{model}"),
                    Err(e) => format!("设置会话模型失败：{e}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 会话视图骨架：左栏会话列表 + 右栏聊天区 + 右侧任务面板。
    ///
    /// 三段**平级**挂在 `flex_row` 根节点上——任务面板是第三列，不是右栏的子节点
    /// （`ed1b8d7` 修过的挂载点；动这里先跑括号栈快照，编译通过不代表挂对了父节点）。
    ///
    /// 拆成多个 render 子函数不是为了整洁：这段曾是 810 行单函数，单帧栈占用把
    /// GPUI 的递归 prepaint 推过栈上限（`has overflowed its stack`）——加一个控件
    /// 就可能崩。新增控件请挂进对应的子函数，不要往这里堆。
    /// 布局对齐 `gui-vite` 的 `SessionPage`（左栏 280px + 右栏 ChatPanel 撑满）。
    pub(crate) fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(self.render_chat_sidebar(cx))
            .child(self.render_chat_main(cx))
            .child(self.render_tasks_panel(cx))
            .into_any_element()
    }

    /// 左栏：头部（状态行 + 新建会话 / 新建目录）+ 分组列表 + 底部文件视图入口。
    pub(crate) fn render_chat_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(colors::LIST_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors::BG_SECONDARY()))
            .border_r_1()
            .border_color(rgb(colors::BORDER()))
            // 头部：状态行 + 新建会话（对齐 Tauri 版 `SessionList` 头部；
            // 「＋ 新目录」要目录选择器弹窗，留到 workspace 页一起做）
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .id("new-session")
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::Plus.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_new_session(cx);
                            })),
                    )
                    // 「＋新目录」：新建一个绑定指定工作目录的会话（目录在
                    // 首条消息时随 `working_directory` 下发并固化）
                    .child(
                        div()
                            .id("new-workspace")
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::FolderPlus.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = window;
                                this.open_workspace_picker(PickerTarget::NewSession, cx);
                            })),
                    ),
            )
            // 分组列表（gpui-component `List`：自带虚拟化与滚动）
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        (0..self.session_rows.len())
                            .map(|ix| self.render_session_row(ix, cx))
                            .collect::<Vec<_>>(),
                    )
                    .overflow_y_scrollbar(),
            )
            // 底部：文件视图入口（占位页；与 Tauri 版同位置）
            .child(
                div()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .px_2()
                    .py_2()
                    .child(
                        div()
                            .id("open-workspace")
                            .w_full()
                            .px_2()
                            .py_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(
                                svg()
                                    .path(IconName::FileText.path())
                                    .size(px(14.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child("文件视图")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = View::Workspace;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    /// 右栏：会话 Tab 条 + 消息列 + 三条横幅 + 输入卡片 + 统计行。
    pub(crate) fn render_chat_main(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    // 顶部：打开的会话 Tab 条（浏览器式；无 tab 时退回单行标题）
                    .child(self.render_session_tabs(cx))
                    .child(self.render_message_column())
                    .child(self.render_banners(cx))
                    .child(self.render_composer(cx))
                    .child(self.render_usage_line())
            )
            .into_any_element()
    }

    /// 消息列：限宽 896 居中（与输入卡片同宽、同左边界）。
    ///
    /// 父级用 `flex_col + items_center` 让限宽块水平居中；限宽块自身
    /// `w_full + max_w` 才会在宽窗口下取 896。
    pub(crate) fn render_message_column(&self) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child({
                        let items = self.messages.clone();
                        let expand = self.expand.clone();
                        let app = self.app_ref.clone();
                        // 回退需要会话 id（消息操作按钮用）
                        let sid = self
                            .current_session
                            .clone()
                            .unwrap_or_default();
                        gpui_list(
                            self.msg_list_state.clone(),
                            move |ix, _window, _cx| {
                                let Some(m) = items.get(ix) else {
                                    return div().into_any_element();
                                };
                                // 段渲染口径见 `render_msg_body`（思考块 / 工具卡可折叠，
                                // 结果按 `tool_calls` 的 id 关联）
                                render_msg_body(m, ix, &expand, &app, &sid)
                            },
                        )
                        .size_full()
                    }),
            )
            .into_any_element()
    }

    /// 审批横幅 / 追问卡片 / 待办窄条：与消息列、输入卡片同宽居中。
    /// 三者空时各自返回 0 高元素，不占位。
    pub(crate) fn render_banners(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_approval_banner(cx)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_clarification(cx)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_todo_bar(cx)),
            )
            .into_any_element()
    }

    /// 输入卡片 = **一体式卡片**（对齐 Tauri 版 `ChatInput`）：
    /// 外框限宽 896 居中、bg-secondary、rounded-2xl；卡片内 = 编辑器 + 底部控件行。
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let ctx_pct = self.ctx_pct();
        let ctx_color = self.ctx_color();
        let ctx_has_usage = self.ctx_usage().is_some() && self.ctx_window() > 0;
        let ctx_center_text = if ctx_has_usage {
            format!("{ctx_pct}%")
        } else {
            "–".to_string()
        };
        div()
            .border_t_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .px_4()
            .pt_3()
            .pb_3()
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_2xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // 编辑器自身不画边框与底色（`appearance(false)`），
                    // 由外层卡片充当「输入框」——否则会看成上下分离的
                    // 两块（上面一个输入框 + 下面一排按钮），与 Tauri
                    // 版「按钮在输入框内」的形态不符。
                    .child(
                        Textarea::new(&self.input)
                            .h(px(64.))
                            .appearance(false),
                    )
                    // 待发送图片缩略图（单独成函数：压 render_chat 栈帧）
                    .children(if self.composer_images.is_empty() {
                        None
                    } else {
                        Some(self.render_composer_attachments(cx))
                    })
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1())
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    // 控件顺序对齐 vite `ChatInput`：模型 → 思考 → 上下文环 → 发送
                                    // @ 文件引用：弹窗选文件 → 光标处插入 `@file:"<路径>"`
                                    // （序列化口径与 Web 版 `serializeInlineChip` 逐字一致）
                                    .child(
                                        div()
                                            .id("mention-file")
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .child(
                                                svg()
                                                    .path(IconName::AtSign.path())
                                                    .size(px(14.))
                                                    .text_color(rgb(
                                                        colors::TEXT_SECONDARY(),
                                                    )),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.open_file_picker(
                                                        FilePickPurpose::Mention,
                                                        cx,
                                                    )
                                                },
                                            )),
                                    )
                                    // 附件（图片）：OS 选择器 → data URL 挂到待发送列表
                                    .child(
                                        div()
                                            .id("attach-image")
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .child(
                                                svg()
                                                    .path(IconName::ImagePlus.path())
                                                    .size(px(14.))
                                                    .text_color(rgb(
                                                        colors::TEXT_SECONDARY(),
                                                    )),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.pick_composer_images(cx)
                                                },
                                            )),
                                    )
                                    .child(
                                        // 模型选择：浮层挂在自己身上（`relative` 定位 +
                                        // `deferred` 延迟绘制）——这样它出现在**按钮正上方**，
                                        // 且画在卡片边框之后（不会透出下面的线）
                                        div()
                                            .id("model-select")
                                            .relative()
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(self.current_model_label())
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.model_menu_open =
                                                        !this.model_menu_open;
                                                    this.thinking_menu_open = false;
                                                    this.ctx_panel_open = false;
                                                    cx.notify();
                                                },
                                            ))
                                            .children(if self.model_menu_open {
                                                Some(deferred(self.render_model_menu(cx)))
                                            } else {
                                                None
                                            }),
                                    )
                                    // 思考强度（当前模型没声明档位时这里是空）
                                    .children(self.render_thinking_select(cx))
                                    .child(
                                        // 上下文环：百分比在**圈内**（vite 同形态）。
                                        // ⚠️ 环值口径是 **0-100**（库约定），不是 0-1
                                        // ——写 0-1 时环永远是空的。
                                        div()
                                            .id("ctx-ring-btn")
                                            .relative()
                                            .flex()
                                            .items_center()
                                            .px_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .child(
                                                ProgressCircle::new("ctx-ring")
                                                    .value(ctx_pct as f32)
                                                    .size(px(30.))
                                                    .color(Hsla::from(rgb(
                                                        ctx_color,
                                                    )))
                                                    .child(
                                                        div()
                                                            .text_size(px(9.))
                                                            .text_color(rgb(
                                                                if ctx_has_usage {
                                                                    colors::TEXT_SECONDARY()
                                                                } else {
                                                                    colors::TEXT_TERTIARY()
                                                                },
                                                            ))
                                                            .child(
                                                                ctx_center_text,
                                                            ),
                                                    ),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    this.ctx_panel_open =
                                                        !this.ctx_panel_open;
                                                    // 三个浮层互斥：开一个收另外两个
                                                    this.model_menu_open = false;
                                                    this.thinking_menu_open = false;
                                                    if !this.ctx_panel_open {
                                                        this.ctx_compress = None;
                                                    }
                                                    cx.notify();
                                                },
                                            ))
                                            .children(if self.ctx_panel_open {
                                                Some(deferred(self.render_ctx_panel(cx)))
                                            } else {
                                                None
                                            }),
                                    )
                            .child({
                                let stopping = self.streaming;
                                div()
                                    .id("send")
                                    .px_3()
                                    .py_1()
                                    .rounded_xl()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .bg(if stopping {
                                        rgb(colors::ERROR())
                                    } else {
                                        rgb(colors::ACCENT())
                                    })
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    // 与 Tauri 版 `ChatInput` 同形：图标 + 文案
                                    // （发送 = lucide `Send`，停止 = `Square`）
                                    .child(
                                        svg()
                                            .path(if stopping {
                                                IconName::Square.path()
                                            } else {
                                                IconName::Send.path()
                                            })
                                            .size(px(14.))
                                            .text_color(rgb(0xffffff)),
                                    )
                                    .child(if stopping {
                                        "停止"
                                    } else {
                                        "发送"
                                    })
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            if this.streaming {
                                                this.stop_current_turn(cx);
                                            } else {
                                                this.send_current_input(
                                                    window, cx,
                                                );
                                            }
                                        },
                                    ))
                            })
                    ),
            )
            )
            .into_any_element()
    }

    /// 输入卡片下方统计行：会话累计（未命中/命中/输出/命中率）+ 最近一轮性能
    /// （TTFT / tok/s）。两路数据都为空时整行不渲染（不留空白条）。
    pub(crate) fn render_usage_line(&self) -> AnyElement {
        if self.session_usage.is_none() && self.request_perf.is_none() {
            return div().into_any_element();
        }
        let mut line = div()
            .w_full()
            .max_w(px(colors::CHAT_MAX_W))
            .mx_auto()
            .pt_2()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(11.))
            .text_color(rgb(colors::TEXT_TERTIARY()));
        let dot = || {
            div().text_color(rgb(colors::BORDER())).child("·")
        };
        if let Some((uncached, cached, completion)) =
            self.session_usage
        {
            let total = uncached + cached;
            let hit_rate = if total > 0 {
                (cached as f64 / total as f64 * 100.0).round()
                    as u64
            } else {
                0
            };
            line = line
                .child(div().child(format!(
                    "未命中输入 {}",
                    group_thousands(uncached)
                )))
                .child(dot())
                .child(div().child(format!(
                    "缓存命中输入 {}",
                    group_thousands(cached)
                )))
                .child(dot())
                .child(div().child(format!(
                    "输出 {}",
                    group_thousands(completion)
                )))
                .child(dot())
                .child(
                    div().child(format!(
                        "缓存命中率 {hit_rate}%"
                    )),
                );
        }
        if let Some((ttft, duration, completion)) =
            self.request_perf
        {
            if self.session_usage.is_some() {
                line = line.child(dot());
            }
            line = line.child(div().child(format!(
            "TTFT {}",
            ttft.map(format_ms)
                .unwrap_or_else(|| "—".to_string())
        )));
            if let Some(tps) =
                tokens_per_sec(ttft, duration, completion)
            {
                line = line
                    .child(dot())
                    .child(div().child(format!("{tps} tok/s")));
            }
        }
        line.into_any_element()
    }

    /// 上下文详情浮层（点圆环弹出）：占用 / 缓存命中 / 输出 +「压缩会话」。
    /// 环本身只是缩略，口径（最近一轮 vs 会话累计）写在浮层里。
    pub(crate) fn render_ctx_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let ctx_pct = self.ctx_pct();
        let ctx_usage = self.ctx_usage();
        let ctx_window = self.ctx_window();
        // 「左标签 / 右值」一行（详情面板三行共用）
        let kv_row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(div().text_color(rgb(colors::TEXT_TERTIARY())).child(label))
                .child(div().flex_1())
                .child(div().text_color(rgb(colors::TEXT_PRIMARY())).child(value))
        };
        // 上下文详情面板（对齐 vite `ContextRing` 的浮层）：占用 / 缓存命中 / 输出
        // +「压缩会话」。环本身只是缩略，口径（最近一轮 vs 会话累计）写在面板里。
        let mut ctx_panel = div()
            .absolute()
            // 相对**环本身**定位（环容器是 `.relative()`）：浮层贴环的正上方
            .bottom(px(34.))
            .right(px(0.))
            .w(px(300.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs()
            .child(kv_row(
                "上下文占用",
                match ctx_usage {
                    Some(u) if ctx_window > 0 => format!(
                        "{} / {}（{ctx_pct}%）",
                        group_thousands(u.prompt_tokens),
                        group_thousands(ctx_window)
                    ),
                    Some(u) => format!("{} token（窗口未知）", group_thousands(u.prompt_tokens)),
                    None => "还没有用量数据".to_string(),
                },
            ))
            .child(kv_row(
                "缓存命中",
                match ctx_usage {
                    Some(u) if u.cache_read > 0 => format!(
                        "{} token（{}%）",
                        group_thousands(u.cache_read),
                        (u.cache_read as f64 / u.prompt_tokens.max(1) as f64 * 100.0).round()
                            as u64
                    ),
                    Some(_) => "0（提供商未返回明细）".to_string(),
                    None => "—".to_string(),
                },
            ))
            .child(kv_row(
                "输出",
                match ctx_usage {
                    Some(u) => format!("{} token", group_thousands(u.completion_tokens)),
                    None => "—".to_string(),
                },
            ))
            .child(
                div()
                    .pt_1()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("以上为最近一轮请求 · 输入框下方小字为会话累计"),
            );
        // 压缩按钮：流式中 / 压缩中不可点（同 vite 的 disabled 口径）
        let compressing = self.ctx_compressing;
        let can_compress = self.current_session.is_some()
            && ctx_usage.is_some()
            && !compressing
            && !self.streaming;
        let btn_color = if can_compress {
            colors::TEXT_SECONDARY()
        } else {
            colors::TEXT_TERTIARY()
        };
        let mut compress_btn = div()
            .id("ctx-compress")
            .w_full()
            .py(px(5.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .text_color(rgb(btn_color))
            .child(
                svg()
                    .path(if compressing {
                        IconName::LoaderCircle.path()
                    } else {
                        IconName::Minimize2.path()
                    })
                    .size(px(12.))
                    .text_color(rgb(btn_color)),
            )
            .child(if compressing {
                "压缩中…".to_string()
            } else if ctx_usage.is_some() {
                format!("压缩会话（当前占用 {ctx_pct}%）")
            } else {
                "压缩会话".to_string()
            });
        if can_compress {
            compress_btn = compress_btn
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| this.compress_session(cx)));
        }
        ctx_panel = ctx_panel.child(compress_btn);
        if let Some((ok, msg)) = &self.ctx_compress {
            ctx_panel = ctx_panel.child(
                div()
                    .text_color(rgb(if *ok {
                        colors::SUCCESS()
                    } else {
                        colors::ERROR()
                    }))
                    .child(msg.clone()),
            );
        }
        ctx_panel.into_any_element()
    }

    /// 思考强度选择器（对齐 vite `ThinkingSelect`）：档位来自当前模型声明，
    /// 模型不支持思考时**整块不渲染**（返回 None，不占位）；`off` 档显示为「default」。
    pub(crate) fn render_thinking_select(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 思考强度选择器（vite `ThinkingSelect` 同口径）：档位来自当前模型声明，
        // 模型不支持思考时**整块不渲染**（不占位）；`off` 档如实显示为「default」。
        let thinking_open = self.thinking_menu_open;
        let thinking_label = self.thinking_label();
        let mut thinking_select: Option<AnyElement> = None;
        if let Some(opts) = self.thinking_options() {
            let mut dd = div()
                .absolute()
                .bottom(px(26.))
                .right(px(0.))
                .w(px(160.))
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_PRIMARY()))
                .py_1()
                .flex()
                .flex_col();
            for (ix, opt) in opts.iter().enumerate() {
                // `off` = 不附加思考参数（None）；其余档位值原样发后端
                let value: Option<String> = if opt == "off" {
                    None
                } else {
                    Some(opt.clone())
                };
                let label = if opt == "off" {
                    "default".to_string()
                } else {
                    opt.clone()
                };
                let is_current = value == self.thinking;
                let mut row = div()
                    .id(("thinking-opt", ix))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_xs();
                row = if is_current {
                    row.text_color(rgb(colors::ACCENT()))
                } else {
                    row.text_color(rgb(colors::TEXT_PRIMARY()))
                };
                dd = dd.child(
                    row.child(div().flex_1().child(label))
                        .children(if is_current {
                            Some(
                                svg()
                                    .path(IconName::Check.path())
                                    .size(px(12.))
                                    .text_color(rgb(colors::ACCENT())),
                            )
                        } else {
                            None
                        })
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_thinking(value.clone(), cx);
                        })),
                );
            }
            thinking_select = Some(
                div()
                    .id("thinking-select")
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(format!("思考 {thinking_label}"))
                    .child(
                        svg()
                            .path(IconName::ChevronDown.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.thinking_menu_open = !this.thinking_menu_open;
                        // 三个浮层互斥：开一个收另外两个
                        this.model_menu_open = false;
                        this.ctx_panel_open = false;
                        cx.notify();
                    }))
                    .children(if thinking_open {
                        Some(deferred(dd))
                    } else {
                        None
                    })
                    .into_any_element(),
            );
        }
        thinking_select
    }

    /// 模型下拉浮层（按钮在输入卡片里，这里只造浮层内容）：
    /// 只列带 `chat` 能力的模型（capabilities 是 kebab-case 数组，后端拼成 `/` 分隔串）。
    pub(crate) fn render_model_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        // 模型下拉候选：只列带 `chat` 能力的模型（与 Tauri 版 `ModelSelector` 同口径，
        // capabilities 是 kebab-case 数组，后端拼成 `/` 分隔的串）
        let chat_models: Vec<(String, String)> = self
            .models
            .iter()
            .filter(|m| m.capabilities.split('/').any(|c| c == "chat"))
            .map(|m| (m.provider.clone(), m.name.clone()))
            .collect();
        let mut menu = div()
            .absolute()
            // 相对**模型按钮**定位：浮层贴在它正上方（不再统一吊在控件行右端）
            .bottom(px(30.))
            .right(px(0.))
            .w(px(256.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_PRIMARY()))
            .py_1()
            .flex()
            .flex_col();
        for (ix, (provider, name)) in chat_models.iter().enumerate() {
            let provider = provider.clone();
            let name = name.clone();
            let is_current = self
                .session_model
                .as_ref()
                .map(|(p, m)| p == &provider && m == &name)
                .unwrap_or(false);
            let mut row = div()
                .id(("model-option", ix))
                .px_3()
                .py_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .cursor_pointer()
                .text_xs();
            if is_current {
                row = row.text_color(rgb(colors::ACCENT()));
            } else {
                row = row.text_color(rgb(colors::TEXT_PRIMARY()));
            }
            let label = name.clone();
            let prov = provider.clone();
            menu = menu.child(
                row.child(div().flex_1().child(name))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(provider),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_chat_model(prov.clone(), label.clone(), cx);
                    })),
            );
        }
        menu.into_any_element()
    }

    /// 输入卡片里的附件缩略图条（每张带移除按钮）。
    ///
    /// 抽成独立方法只为**压 `render_chat` 的栈帧**（GPUI 递归 prepaint，单帧过大
    /// 会栈溢出）。
    pub(crate) fn render_composer_attachments(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut strip = div().w_full().flex().flex_row().flex_wrap().gap_2();
        for (i, src) in self.composer_images.iter().enumerate() {
            let Some((fmt, bytes)) = decode_data_url(src) else {
                continue;
            };
            let arc = std::sync::Arc::new(gpui_kit::Image::from_bytes(fmt, bytes));
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(
                        gpui_kit::img(gpui_kit::ImageSource::Image(arc))
                            .max_w(px(64.))
                            .max_h(px(64.))
                            .rounded_md(),
                    )
                    .child(
                        div()
                            .id(("rm-attach", i))
                            .px_1()
                            .rounded_md()
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(IconName::X.path())
                                    .size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY())),
                            )
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                if i < this.composer_images.len() {
                                    this.composer_images.remove(i);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        strip.into_any_element()
    }

    /// 选择图片附件（OS 文件对话框）→ 读成 data URL 挂到待发送列表。
    ///
    /// 注：OS 模态框不可自动化（ADR-052），自动化验收走 `attach_image` 动作
    /// 直接给路径（与知识导入「按路径导入」同一口径）。
    pub(crate) fn pick_composer_images(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("选择图片".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let urls: Vec<String> = paths
                .iter()
                .filter_map(|p| image_path_to_data_url(p))
                .collect();
            if urls.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.composer_images.extend(urls);
                cx.notify();
            });
        })
        .detach();
    }

    /// 在输入框**光标处**插入一段文本（光标自动前移到插入内容之后）。
    ///
    /// 用 `TextareaState::insert` 而不是「读 value → 拼串 → `set_value`」：
    /// `set_value` 会把选区重置到开头并清空撤销栈，用户接着打字的位置就丢了。
    pub(crate) fn insert_into_composer(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.clone();
        input.update(cx, |state, cx| {
            state.insert(text, window, cx);
        });
        cx.notify();
    }

    /// @ 文件引用：把选中的文件序列化成 `` `@file:"<绝对路径>"` `` 插入输入框并关闭弹窗。
    ///
    /// 序列化口径与 Web 版 **逐字一致**（`gui-vite/src/lib/attachments.ts` 的
    /// `serializeInlineChip`：Markdown 行内代码 + 绝对路径）：
    /// - 模型侧看到的是连贯句子里的行内 code，路径直白，`read_file` 直接可用；
    /// - 消息区渲染不需要特殊处理（行内 code 是 markdown 原生语法）；
    /// - 跨端复制粘贴、草稿恢复都只是纯文本。
    ///
    /// 末尾补一个空格：连续引用两个文件时 chip 之间不会粘在一起。
    pub(crate) fn mention_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let snippet = format!("`@file:\"{path}\"` ");
        self.insert_into_composer(snippet, window, cx);
        self.dialog = None;
        cx.notify();
    }

    /// @ 文件引用（无 `&mut Window` 的入口：自动化端口）。
    ///
    /// `Context` 里拿不到 window，就用保存的窗口句柄现取；**最终仍调
    /// `TextareaState::insert`**（与真实点击路径同一个 API），不复制插入逻辑。
    pub(crate) fn mention_file_auto(&mut self, path: String, cx: &mut Context<Self>) -> Result<(), String> {
        let handle = self.window_handle;
        let input = self.input.clone();
        let snippet = format!("`@file:\"{path}\"` ");
        handle
            .update(cx, move |_, window, cx| {
                input.update(cx, |state, cx| {
                    state.insert(snippet, window, cx);
                });
            })
            .map_err(|e| format!("窗口已关闭，无法插入：{e}"))?;
        self.dialog = None;
        cx.notify();
        Ok(())
    }

    /// 清空输入框。
    ///
    /// 只给自动化收尾用（测完不让残留文本污染用户下一次发送）。转的是
    /// `TextareaState::set_value` 这个既有 API，没有第二套写入口。
    pub(crate) fn clear_composer(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let handle = self.window_handle;
        let input = self.input.clone();
        handle
            .update(cx, move |_, window, cx| {
                input.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                });
            })
            .map_err(|e| format!("窗口已关闭：{e}"))?;
        cx.notify();
        Ok(())
    }
}
