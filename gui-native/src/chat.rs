//! 会话页：消息列 + 输入卡片 + 上下文环 + 模型/思考选择器。
//!
//! 布局三段平级挂 `flex_row`（左列表 / 右聊天 / 右任务面板），任务面板不是右栏的
//! 子节点——改挂载点前先跑括号栈快照，编译通过不代表挂对了父节点。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::theme::colors;
use crate::types::View;
use crate::types::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::list as gpui_list;

/// 输入卡片下方统计行的高度（恒定，含无数据时的占位）。
///
/// 旧值是 `pt_2`(8) + 11px 文字行高(≈16) ≈ 24，这里取整为 24 并**固定**：
/// 切换会话时 `session_usage` 会被清空等新数据，若整行随之消失，
/// 输入区高度就抖一下（用户实测「多会话切换上下抖动」）。
const USAGE_LINE_H: f32 = 24.0;

/// 待发图片张数上限（与 Web 版 `ChatInput.tsx` 同数：4 张）。
///
/// 再多既撑上下文，也会让模型抓不到重点；Web 版在选图按钮满 4 张时禁用，
/// 这里在粘贴通道拦截并给出提示（粘贴没有「按钮禁用」这个信号）。
pub(crate) const MAX_COMPOSER_IMAGES: usize = 4;

/// 一次粘贴最多引用多少个文件（与 Web 版同数：20 个）。
pub(crate) const MAX_COMPOSER_FILES: usize = 20;

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
        if self.ctx_compressing || self.busy() {
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
                match res {
                    Ok(_) => {
                        let label = this.thinking_label();
                        this.set_status(format!("思考强度已设置：{label}"));
                    }
                    Err(e) => this.set_error(format!("设置思考强度失败：{e}")),
                }
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
    pub(crate) fn select_chat_model(
        &mut self,
        provider: String,
        model: String,
        cx: &mut Context<Self>,
    ) {
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
            self.set_status(format!("新会话将使用 {model} · {provider}"));
            cx.notify();
            return;
        };
        let thinking = self.thinking.clone();
        let port = self.port;
        self.set_status(format!("正在为会话切换模型到 {model}…"));
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{sid}/model");
            let mut body = serde_json::json!({ "provider": provider, "model": model });
            if let Some(e) = thinking {
                body["thinking"] = serde_json::Value::String(e);
            }
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => this.set_status(format!("会话模型已切换：{model}")),
                    Err(e) => this.set_error(format!("设置会话模型失败：{e}")),
                }
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_new_session(Some(window), cx);
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
                    .child(self.render_message_column(cx))
                    .child(self.render_banners(cx))
                    .child(self.render_composer(cx))
                    .child(self.render_usage_line()),
            )
            .into_any_element()
    }

    /// 消息列：限宽 896 居中（与输入卡片同宽、同左边界）。
    ///
    /// 父级用 `flex_col + items_center` 让限宽块水平居中；限宽块自身
    /// `w_full + max_w` 才会在宽窗口下取 896。
    pub(crate) fn render_message_column(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center();
        // 更早历史入口：**只在滚到列表顶部时出现**。
        //
        // 早先是 `has_more` 为真就常驻在消息区上方——用户实测「一直出现，不符合
        // 预期，应该在滚到顶部时再出现」。Web 版更彻底（滚到顶**自动加载**），
        // 这里仍保留显式按钮：GPUI 的 `ListState` 没有滚动回调，自动加载要额外
        // 造节流/防连击，而按钮语义明确、还能显示加载中状态。
        //
        // 怎么拿到「到顶」：`ListState` 没有事件，但 `logical_scroll_top()` 给出
        // `ListOffset { item_ix, offset_in_item }`，`item_ix == 0` 即已滚到最前项。
        // 缺的那半环是**滚动后重新渲染**——在列表容器上挂 `on_scroll_wheel`
        // （见下），滚轮到达时 `cx.notify()`。它不消费事件，列表自身照常滚动。
        let at_top = self.msg_list_state.logical_scroll_top().item_ix == 0;
        if self.has_more_history && at_top {
            col = col.child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .pt_2()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(
                        div()
                            .id("load-older")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .cursor_pointer()
                            .child(if self.loading_older {
                                "正在加载更早的消息…".to_string()
                            } else {
                                "↑ 加载更早的消息".to_string()
                            })
                            .on_click(
                                cx.listener(|this, _, _window, cx| this.load_older_messages(cx)),
                            ),
                    ),
            );
        }
        col.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .max_w(px(colors::CHAT_MAX_W))
                // 滚轮到达 → 重渲染，让上面那颗「加载更早」按钮按滚动位置出现/消失。
                // 不 stop_propagation：列表自己还要滚。
                .on_scroll_wheel(cx.listener(
                    |_this, _ev: &gpui_kit::ScrollWheelEvent, _window, cx| {
                        cx.notify();
                    },
                ))
                .child({
                    let items = self.messages.clone();
                    let expand = self.expand.clone();
                    let app = self.app_ref.clone();
                    // 回退需要会话 id（消息操作按钮用）
                    let sid = self.current_session.clone().unwrap_or_default();
                    // 「思考中」指示器的两个输入：本轮是否在跑、哪条是末尾。
                    // 判定本身在 `msg::stream_pending` 一处（与下方
                    // `render_stream_below` 共用），这里只负责把「是不是末尾」喂进去。
                    let streaming = self.busy();
                    let tail_ix = self.messages.len().saturating_sub(1);
                    gpui_list(self.msg_list_state.clone(), move |ix, _window, _cx| {
                        let Some(m) = items.get(ix) else {
                            return div().into_any_element();
                        };
                        // 这条是否属于**同一轮**的后续步骤：往前跳过 `tool` 消息，
                        // 上一条非 tool 若是 assistant，说明同一个用户轮次里的下一步
                        // （一轮多步 → 每步一条 assistant 消息）。用于弱化渲染：
                        // 轮内后续不打时间戳，否则看起来像几条独立回复。
                        let is_continuation = {
                            let mut j = ix;
                            let mut cont = false;
                            while j > 0 {
                                j -= 1;
                                match items[j].role.as_str() {
                                    "tool" => continue,
                                    "assistant" => {
                                        cont = true;
                                        break;
                                    }
                                    _ => break,
                                }
                            }
                            cont
                        };
                        // 段渲染口径见 `render_msg_body`（思考块 / 工具卡可折叠，
                        // 结果按 `tool_calls` 的 id 关联）
                        render_msg_body(
                            m,
                            ix,
                            &expand,
                            &app,
                            &sid,
                            is_continuation,
                            stream_pending(m, streaming && ix == tail_ix),
                        )
                    })
                    .size_full()
                }),
        )
        .into_any_element()
    }

    /// 事件流断线横幅：只在 `events_connected == false` 时出现。
    ///
    /// 为什么必须有：断线时界面会**停在某一轮中间不动**（按钮已回到「发送」、正文缺
    /// 最后一段），用户唯一的判断依据就是这条横幅 —— 否则只能怀疑应用卡死然后重启
    /// （用户实测就是这样恢复的）。重连成功后横幅自动消失，快照帧会把窗口对齐。
    pub(crate) fn render_connection_banner(&self) -> AnyElement {
        if self.events_connected {
            return div().into_any_element();
        }
        div()
            .px_4()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::WARNING()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child("与服务端的连接已断开，正在自动重连…（消息不会丢，恢复后会自动对齐）"),
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
                    .child(self.render_connection_banner()),
            )
            // 追问面板**不在这里**：它替换输入框（见 `render_composer` 开头），
            // 与 vite `ChatInput` 同位置。
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_todo_bar(cx)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(colors::CHAT_MAX_W))
                    .child(self.render_stream_below()),
            )
            .into_any_element()
    }

    /// 消息区下方的「思考中…」指示器（vite `ChatPanel` 的状态提示带同位置：
    /// 消息区与输入区之间，不随列表滚动）。
    ///
    /// 只在**首 token 之前**出现：正文还空、思考也还没开始 —— 这是用户最容易
    /// 误判成「没发出去」的那一段静默。一旦思考开始，指示器移到气泡内
    /// （`stream_pending` 单点判定，见 `msg::StreamPending`），这里自动消失。
    pub(crate) fn render_stream_below(&self) -> AnyElement {
        if !self.busy() {
            return div().into_any_element();
        }
        let Some(last) = self.messages.last() else {
            return div().into_any_element();
        };
        if stream_pending(last, true) != StreamPending::Below {
            return div().into_any_element();
        }
        div()
            .px_4()
            .py_2()
            .child(render_stream_hint("stream-hint-below", "思考中…"))
            .into_any_element()
    }

    /// 输入框高度：随内容增高（2 行起，约 200px 封顶后内部滚动）。
    ///
    /// 为什么自己算：GPUI 的 `Textarea` 只暴露 `.h(DefiniteLength)`（固定高度），
    /// 没有 auto-grow —— 早先写死 64px，用户写三行以上的 prompt 就得在 2.5 行高的
    /// 小窗里滚动（Web 版是 3.5rem → 200px 自适应）。
    ///
    /// 估算口径：全角按 2 个半角宽，一行约 58 个半角字符（896px 卡片宽 / 14px 字号）。
    /// 误差一两行不影响观感，**宁可高一点**也不要让人手动滚。
    pub(crate) fn composer_height(&self, cx: &App) -> f32 {
        const COLS: f32 = 58.0;
        const LINE_H: f32 = 21.0;
        const PAD: f32 = 26.0;
        // 封顶所需行数：到这就不再往下数。长文本每帧全扫纯属浪费（输入一万字时
        // 每帧一万次字符判断，60fps 就是 60 万次/秒，全为算一个恒定 200px）。
        const MAX_ROWS: f32 = (200.0 - PAD) / LINE_H;
        let text = self.input.read(cx).value().to_string();
        let mut rows = 0.0f32;
        for line in text.split('\n') {
            let mut w = 0.0f32;
            for ch in line.chars() {
                // 制表符按 4 个半角宽，其余非 ASCII 按全角
                w += if ch == '\t' {
                    4.0
                } else if ch.is_ascii() {
                    1.0
                } else {
                    2.0
                };
                // 单行内也提前停：这一行自己就够了
                if w > COLS * MAX_ROWS {
                    break;
                }
            }
            rows += (w / COLS).ceil().max(1.0);
            if rows >= MAX_ROWS {
                break;
            }
        }
        // 最少**一行**（早先是 2 行 = 68px，用户实测指出「输入框文字下面一直留着
        // 这么高的高度」——那不是图片残留，就是这条 `max(2.0)` 撑出来的）。
        // 单行 = 21 + 26 ≈ 47px，与 vite `min-h-[3.5rem]` 的观感一致。
        let rows = rows.max(1.0);
        (rows * LINE_H + PAD).clamp(44.0, 200.0)
    }

    /// 输入卡片 = **一体式卡片**（对齐 Tauri 版 `ChatInput`）：
    /// 外框限宽 896 居中、bg-secondary、rounded-2xl；卡片内 = 编辑器 + 底部控件行。
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        // 模型在追问时，问询面板**替换输入框**（对齐 vite `ChatInput` 的位置）：
        // 这一刻用户唯一该做的动作是回答问题，两个面板并排会让人不知道该点哪边。
        // 早先它挂在消息列下方当横幅（与 vite 不同形），用户实测指出「不如 vite 好看」。
        if self.clarification.is_some() {
            return div()
                .border_t_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_PRIMARY()))
                .px_4()
                .pt_3()
                .pb_3()
                .child(self.render_clarification(cx))
                .into_any_element();
        }
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
                    // 兜底路径：Textarea 的 `on_paste` 是正解（见下），但它是组件内部
                    // 钩子——万一某次 gpui-component 升级改了触发时机，用户按 Ctrl+V
                    // 就会「什么都没发生」。这里再挂一层 `on_key_down`：只有贴图成功
                    // 才吞事件，纯文本仍交给 Textarea。两条路径都命中时靠
                    // `paste_image_from_item` 里的去重保证不会贴两份。
                    .on_key_down(cx.listener(
                        |this, ev: &gpui_kit::KeyDownEvent, _window, cx| {
                            let k = &ev.keystroke;
                            if k.modifiers.control
                                && k.key == "v"
                                && this.paste_image_from_clipboard(cx)
                            {
                                cx.stop_propagation();
                            }
                        },
                    ))
                    // 编辑器自身不画边框与底色（`appearance(false)`），
                    // 由外层卡片充当「输入框」——否则会看成上下分离的
                    // 两块（上面一个输入框 + 下面一排按钮），与 Tauri
                    // 版「按钮在输入框内」的形态不符。
                    .child({
                    // 待发送图片缩略图：**在输入框上方**（对齐 vite `ChatInput`：
                    // `{images.length > 0 && ...}` 渲染在 `<ComposerEditor>` 之前）。
                    // 早先放在 Textarea 之后（= 下方），用户实测指出与 vite 不同。
                    // 单独成函数：压 render_chat 栈帧。
                    let attachments = if self.composer_images.is_empty() {
                        None
                    } else {
                        Some(self.render_composer_attachments(cx))
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(attachments)
                        .child({
                        // Ctrl+V 的正解是 Textarea 自己的 `on_paste` 钩子（gpui-component
                        // 专门为「粘贴图片 / 文件」留的，等价于 Web 的 `onPaste`），
                        // **不是**外层 `on_key_down`：后者在冒泡阶段根本拿不到事件
                        // （Textarea 内建的 paste action 先消费掉了），实测现象就是
                        // 「焦点在输入框里按 Ctrl+V 没反应，只能点按钮」。
                        // 返回 `false` 时文本粘贴照常走默认实现，不抢。
                        let app = self.app_ref.clone();
                        let height = self.composer_height(cx);
                        Textarea::new(&self.input)
                            .h(px(height))
                            .appearance(false)
                            .on_paste(move |item, window, cx| {
                                let Some(w) = app.lock().ok().and_then(|g| g.clone()) else {
                                    return false;
                                };
                                w.update(cx, |this: &mut ClientApp, cx| {
                                    // 分流顺序不可"优化"：图片优先，文件只兜底。
                                    // 反过来会把「复制的 png」变成模型读不了的路径引用
                                    // （Web 版同一条结论，见 ChatInput.tsx 的图片通道注释）。
                                    this.paste_image_from_item(item, cx)
                                        || this.paste_files_from_item(item, window, cx)
                                })
                                .unwrap_or(false)
                            })
                        })
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
                                    // 贴图：截图直接在剪贴板里时，比「附件 → 翻文件夹」短一步
                                    .child(
                                        div()
                                            .id("paste-image")
                                            .px_1()
                                            .py_1()
                                            .rounded_lg()
                                            .cursor_pointer()
                                            .child(
                                                svg()
                                                    .path(IconName::ClipboardPaste.path())
                                                    .size(px(14.))
                                                    .text_color(rgb(
                                                        colors::TEXT_SECONDARY(),
                                                    )),
                                            )
                                            .on_click(cx.listener(
                                                |this, _, _window, cx| {
                                                    if !this.paste_image_from_clipboard(cx) {
                                                        this.status =
                                                            "剪贴板里没有图片（截图后直接点这里即可贴上）"
                                                                .to_string();
                                                        cx.notify();
                                                    }
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
                                let stopping = self.busy();
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
                                    // （发送 = lucide `Send`，停止 = 实心方块）
                                    .child(
                                        if stopping {
                                            // vite 版是 `<Square className="fill-current" />`
                                            // —— lucide 的描边矩形被 `fill-current` 填满，
                                            // 看起来是**实心**方块。GPUI 这边 `svg()` 渲染的是
                                            // lucide 原始 stroke 版（空心方框），改不了 fill，
                                            // 所以直接画一个白方块。尺寸照抄 lucide 的几何：
                                            // 24 视口里 `rect x=3 y=3 w=18 h=18 rx=2`，按 16px
                                            // 渲染即 12×12 的实心块、1.3px 圆角（用 12px 与
                                            // 1.5px 的圆角，放大截图核对过形状）。
                                            div()
                                                .w(px(12.))
                                                .h(px(12.))
                                                .rounded(px(1.5))
                                                .bg(rgb(0xffffff))
                                                .into_any_element()
                                        } else {
                                            svg()
                                                .path(IconName::Send.path())
                                                .size(px(14.))
                                                .text_color(rgb(0xffffff))
                                                .into_any_element()
                                        },
                                    )
                                    .child(if stopping {
                                        "停止"
                                    } else {
                                        "发送"
                                    })
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            if this.busy() {
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
    /// （TTFT / tok/s）。
    ///
    /// **无数据时也占固定高度**（`USAGE_LINE_H`）。早先空数据直接返回 0 高元素，
    /// 而 `open_session` 会把 `session_usage` / `request_perf` 清空等新数据回来
    /// —— 那一瞬间整行消失，输入区矮 20 多像素、消息列表可视高度跟着变，
    /// 表现就是「多个会话间切换时上下抖动」（用户实测）。恒定高度让切换前后
    /// 布局不变。
    pub(crate) fn render_usage_line(&self) -> AnyElement {
        let mut line = div()
            .w_full()
            .max_w(px(colors::CHAT_MAX_W))
            .mx_auto()
            .h(px(USAGE_LINE_H))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(11.))
            .text_color(rgb(colors::TEXT_TERTIARY()));
        if self.session_usage.is_none() && self.request_perf.is_none() {
            // 空内容但**等高**：占位不留白条，也不让布局跳
            return line.into_any_element();
        }
        let dot = || div().text_color(rgb(colors::BORDER())).child("·");
        if let Some((uncached, cached, completion)) = self.session_usage {
            let total = uncached + cached;
            let hit_rate = if total > 0 {
                (cached as f64 / total as f64 * 100.0).round() as u64
            } else {
                0
            };
            line = line
                .child(div().child(format!("未命中输入 {}", group_thousands(uncached))))
                .child(dot())
                .child(div().child(format!("缓存命中输入 {}", group_thousands(cached))))
                .child(dot())
                .child(div().child(format!("输出 {}", group_thousands(completion))))
                .child(dot())
                .child(div().child(format!("缓存命中率 {hit_rate}%")));
        }
        if let Some((ttft, duration, completion)) = self.request_perf {
            if self.session_usage.is_some() {
                line = line.child(dot());
            }
            line = line.child(div().child(format!(
                "TTFT {}",
                ttft.map(format_ms).unwrap_or_else(|| "—".to_string())
            )));
            if let Some(tps) = tokens_per_sec(ttft, duration, completion) {
                line = line.child(dot()).child(div().child(format!("{tps} tok/s")));
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
        // 压缩点**可见入口**：会话链里有压缩点就在浮层里给一行，点击滚到列表顶
        // （`render_window` 已把最近一条压缩点提到窗口最前）。
        //
        // 为什么必须有：压缩点只是历史里一条很长的摘要消息，落在窗口外就完全
        // 不可见——用户实测「上下文占比从 58% 掉到 2%，一个压缩点都没找到」
        // （native 是被 60 条窗口截掉，Web 是被 50 条分页挡在第 3 页）。
        let marker_positions: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.compression)
            .map(|(i, _)| i)
            .collect();
        if let Some(first) = marker_positions.first().copied() {
            ctx_panel = ctx_panel.child(
                div()
                    .id("ctx-jump-marker")
                    .w_full()
                    .py(px(5.))
                    .rounded_lg()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(
                        svg()
                            .path(IconName::Minimize2.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_SECONDARY())),
                    )
                    .child(format!(
                        "压缩点 {} 处 · 跳到最近一次",
                        marker_positions.len()
                    ))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.msg_list_state.scroll_to(gpui_kit::ListOffset {
                            item_ix: first,
                            offset_in_item: px(0.),
                        });
                        this.ctx_panel_open = false;
                        cx.notify();
                    })),
            );
        }
        // 压缩按钮：流式中 / 压缩中不可点（同 vite 的 disabled 口径）
        let compressing = self.ctx_compressing;
        let can_compress =
            self.current_session.is_some() && ctx_usage.is_some() && !compressing && !self.busy();
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
            // 缩略图点击放大（对齐会话内图片的行为）：与消息里的图片走**同一个**
            // 全屏遮罩（`ClientApp::zoomed_image`），不再出现「会话里的图能放大、
            // 输入框里的不能」（用户实测指出）。
            let arc_for_click = arc.clone();
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .id(("attach-img", i))
                            .cursor_pointer()
                            .child(
                                gpui_kit::img(gpui_kit::ImageSource::Image(arc))
                                    // 对齐 vite `ChatInput` 的 `w-16 h-16 rounded-lg object-cover`：
                                    // **定尺寸 + Cover 裁剪**，不是 `max_w/max_h` 的
                                    // Contain —— 后者元素框是 64×64 而图非正方形时会
                                    // 上下/左右留白（用户实测的「缩略图上下有白条」）。
                                    .w(px(64.))
                                    .h(px(64.))
                                    .object_fit(gpui_kit::ObjectFit::Cover)
                                    .rounded_md(),
                            )
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.zoomed_image = Some(arc_for_click.clone());
                                cx.notify();
                            })),
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

    /// 从系统剪贴板贴图片（截图后直接 Ctrl+V / 点「粘贴图片」）。
    ///
    /// 为何必要：用户排查界面问题的方式就是**截图 → 粘贴**。没有这条路就只能
    /// 「附件 → 翻文件夹找图」，而截图往往只存在剪贴板里。
    ///
    /// GPUI 的 `ClipboardEntry::Image` 直接给原始字节 + 格式，不必落盘，
    /// 复用 [`bytes_to_data_url`] 转成后端认的 data URL。
    /// 返回是否真贴上（剪贴板没有图片时返回 `false`，调用方保留文本粘贴）。
    pub(crate) fn paste_image_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        self.paste_image_from_item(&item, cx)
    }

    /// 从一份剪贴板载荷里取图片贴成待发送附件（**输入框 `on_paste` 与按钮共用**）。
    ///
    /// 后端只认 PNG/JPEG/WebP/GIF，而 Windows 截图（Win+Shift+S）给的是 BMP ——
    /// 这里统一过 `normalize_image_for_api` 转码，避免「贴上去看着好好的、一发就
    /// 400 不支持的图片格式」（实测踩过）。
    /// 返回是否真贴上（没有图片时返回 `false`，文本粘贴照常走默认实现）。
    pub(crate) fn paste_image_from_item(
        &mut self,
        item: &gpui_kit::ClipboardItem,
        cx: &mut Context<Self>,
    ) -> bool {
        for entry in &item.entries {
            let gpui_kit::ClipboardEntry::Image(img) = entry else {
                continue;
            };
            let mime = match img.format {
                gpui_kit::ImageFormat::Png => "image/png",
                gpui_kit::ImageFormat::Jpeg => "image/jpeg",
                gpui_kit::ImageFormat::Gif => "image/gif",
                gpui_kit::ImageFormat::Bmp => "image/bmp",
                _ => "image/png",
            };
            let Some((mime, bytes)) = normalize_image_for_api(mime, &img.bytes) else {
                self.set_status("剪贴板图片无法解码（格式不支持），未贴上".to_string());
                cx.notify();
                return false;
            };
            let Some(url) = bytes_to_data_url(&mime, &bytes) else {
                self.set_status("剪贴板图片超过 8MB，未贴上".to_string());
                cx.notify();
                return false;
            };
            // 去重：`Textarea::on_paste` 与外层 `on_key_down` 是**两条**都可能命中的
            // 路径（见 `render_composer` 的注释），同一张图不能被贴两份。
            if self.composer_images.iter().any(|u| u == &url) {
                return true;
            }
            // 张数上限与 Web 版一致（4 张）：再多既撑上下文，也会让模型抓不到重点
            if self.composer_images.len() >= MAX_COMPOSER_IMAGES {
                self.set_status(format!(
                    "最多 {} 张图片（先删掉一些再贴）",
                    MAX_COMPOSER_IMAGES
                ));
                cx.notify();
                // 返回 true：这次粘贴算「已处理」，不让文本通道再插一遍
                return true;
            }
            self.composer_images.push(url);
            self.set_status("已从剪贴板贴上图片".to_string());
            cx.notify();
            return true;
        }
        false
    }

    /// 剪贴板里的**文件**（资源管理器 Ctrl+C / 拖入）→ 光标处插入 `@file:"…"` 引用。
    ///
    /// 与 Web 版的「路径通道」同一口径：模型拿到路径，自己决定读不读——比塞 base64
    /// 省上下文，也避免「复制的 png 变成模型读不了的二进制块」。
    /// 返回是否插入了东西（没有文件条目返回 `false`，纯文本粘贴照常走默认实现）。
    pub(crate) fn paste_files_from_item(
        &mut self,
        item: &gpui_kit::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut snippet = String::new();
        for entry in &item.entries {
            let gpui_kit::ClipboardEntry::ExternalPaths(ext) = entry else {
                continue;
            };
            for path in ext.paths() {
                // 一次粘贴最多 20 个引用（与 Web 版同数）：超了只丢多余项，
                // 已有内容与已插入的部分不动
                if snippet.matches("@file:").count() >= MAX_COMPOSER_FILES {
                    break;
                }
                snippet.push_str(&format!("`@file:\"{}\"` ", path.to_string_lossy()));
            }
        }
        if snippet.is_empty() {
            return false;
        }
        let n = snippet.matches("@file:").count();
        self.insert_into_composer(snippet, window, cx);
        self.set_status(format!("已引用 {n} 个文件"));
        cx.notify();
        true
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
                // 与粘贴通道**同一个**上限（Web 版 4 张）。选图走的是 OS 多选，一次
                // 可能带回来十几个——早先只有粘贴通道拦了，选图可以无限加（不一致，
                // 实测：选 8 张就真进了 8 张）。
                let mut dropped = 0usize;
                for u in urls {
                    if this.composer_images.len() < MAX_COMPOSER_IMAGES {
                        this.composer_images.push(u);
                    } else {
                        dropped += 1;
                    }
                }
                if dropped > 0 {
                    this.status =
                        format!("超出上限的 {dropped} 张未加入（最多 {MAX_COMPOSER_IMAGES} 张）");
                }
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
    pub(crate) fn mention_file(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snippet = format!("`@file:\"{path}\"` ");
        self.insert_into_composer(snippet, window, cx);
        self.dialog = None;
        cx.notify();
    }

    /// @ 文件引用（无 `&mut Window` 的入口：自动化端口）。
    ///
    /// `Context` 里拿不到 window，就用保存的窗口句柄现取；**最终仍调
    /// `TextareaState::insert`**（与真实点击路径同一个 API），不复制插入逻辑。
    pub(crate) fn mention_file_auto(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
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
