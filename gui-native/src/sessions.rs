//! 左栏会话列表：分组、折叠、增删改、多会话 Tab。
//!
//! 派生关系：`sessions_data`（服务端原样）+ 折叠态 + 选中态 → `session_rows`
//! （渲染用的行）。任何一项变化都要 `rebuild_session_rows` 重算，不就地改行。

use crate::*;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::InputState;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    pub(crate) fn load_sessions(&self, cx: &mut Context<Self>) {
        let port = self.port;
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
                            working_directory: s
                                .get("working_directory")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            updated_at: s
                                .get("updated_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            first_request_tokens: s
                                .get("first_request_tokens")
                                .and_then(|v| v.as_u64()),
                        })
                        .collect();
                    let n = rows.len();
                    println!("[会话] 拉到 {n} 条");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.sessions_data = rows;
                            this.session_count = n;
                            this.set_status(format!("127.0.0.1:{port} · 会话 {n} 条"));
                            this.rebuild_session_rows(cx);
                        });
                    });
                }
                Err(e) => {
                    println!("[会话] 拉取失败: {e}");
                    cx.update(|app| {
                        let _ = this.update(app, |this, cx| {
                            this.set_error(format!("拉取会话失败：{e}"));
                            cx.notify();
                        });
                    });
                }
            }
        })
        .detach();
    }

    /// 重建左栏行（分组 / 会话 / 查看更多）。
    ///
    /// 排序对齐 `gui-vite/src/components/session/SessionList.tsx` 的 `sortGroups`：
    /// 1.「最近活跃」段：24h 内有活动的工作区，按组内最近时间倒序，最多 3 个；
    /// 2.「其余」段：按路径字母序（稳定段，位置不随使用漂移）；
    /// 3. 默认组（`""`）始终垫底（即使没有未绑定会话，也要能在这儿新建）。
    ///
    /// 组内折叠：初始 3 条 + 「查看更多」累加；**当前会话始终可见**（从别处切进
    /// 深处会话时不会被折叠藏住，同 Tauri 版）。
    pub(crate) fn rebuild_session_rows(&mut self, cx: &mut Context<Self>) {
        use std::collections::{BTreeMap, HashSet};
        let mut groups: BTreeMap<String, Vec<&SessionRow>> = BTreeMap::new();
        groups.entry(String::new()).or_default();
        for s in &self.sessions_data {
            groups
                .entry(s.working_directory.clone())
                .or_default()
                .push(s);
        }
        for list in groups.values_mut() {
            list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        }
        let now = now_unix_secs();
        let last_activity = |list: &[&SessionRow]| -> i64 {
            list.iter()
                .filter_map(|s| parse_rfc3339_secs(&s.updated_at))
                .max()
                .unwrap_or(0)
        };
        let mut active: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, l)| !k.is_empty() && last_activity(l) >= now - ACTIVE_GROUP_WINDOW_SECS)
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        // sort_by_key + Reverse：比较键只算一次（sort_by 的闭包会被反复调用）
        active.sort_by_key(|a| std::cmp::Reverse(last_activity(&a.1)));
        active.truncate(ACTIVE_GROUP_LIMIT);
        let active_keys: HashSet<&str> = active.iter().map(|(k, _)| k.as_str()).collect();
        let mut rest: Vec<(String, Vec<&SessionRow>)> = groups
            .iter()
            .filter(|(k, _)| !k.is_empty() && !active_keys.contains(k.as_str()))
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        let mut ordered = active;
        ordered.extend(rest);
        ordered.extend(
            groups
                .iter()
                .filter(|(k, _)| k.is_empty())
                .map(|(k, l)| (k.clone(), l.clone())),
        );

        let mut rows: Vec<SessionListRow> = Vec::new();
        for (key, list) in ordered {
            let collapsed = self.collapsed_groups.contains(&key);
            let count = list.len();
            rows.push(SessionListRow::Group {
                label: group_label(&key),
                key: key.clone(),
                count,
                collapsed,
            });
            if collapsed {
                continue;
            }
            let base =
                SESSIONS_INITIAL_VISIBLE + self.expanded_groups.get(&key).copied().unwrap_or(0);
            let visible = match self
                .current_session
                .as_deref()
                .and_then(|id| list.iter().position(|s| s.id == id))
            {
                Some(pos) if pos >= base => pos + 1,
                _ => base,
            }
            .min(count);
            for s in list.iter().take(visible) {
                rows.push(SessionListRow::Session {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    meta: format_relative(&s.updated_at),
                    tasks: self.running_tasks_of(&s.id),
                    selected: self.current_session.as_deref() == Some(s.id.as_str()),
                });
            }
            let hidden = count.saturating_sub(visible);
            if hidden > 0 {
                rows.push(SessionListRow::More { key, hidden });
            }
        }
        self.session_rows = rows;
        cx.notify();
    }

    /// 该会话正在跑的后台任务数（`pending` / `running` 计）。
    pub(crate) fn running_tasks_of(&self, session_id: &str) -> usize {
        self.all_tasks
            .iter()
            .filter(|t| {
                t.parent_session_id == session_id
                    && (t.status == "pending" || t.status == "running")
            })
            .count()
    }

    /// 折叠 / 展开一个工作区分组。
    pub(crate) fn toggle_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key);
        }
        self.rebuild_session_rows(cx);
    }

    /// 组内「查看更多」：再展开 `SESSIONS_LOAD_STEP` 条。
    pub(crate) fn expand_group(&mut self, key: String, cx: &mut Context<Self>) {
        *self.expanded_groups.entry(key).or_insert(0) += SESSIONS_LOAD_STEP;
        self.rebuild_session_rows(cx);
    }

    /// 保存当前输入草稿（按会话键；新会话用空串）。
    ///
    /// 切换会话 / 新建会话**之前**调用。空正文且无图片时不占位（顺手清掉空条目）。
    pub(crate) fn save_composer_draft(&mut self, cx: &App) {
        let key = self.current_session.clone().unwrap_or_default();
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() && self.composer_images.is_empty() {
            self.drafts.remove(&key);
            return;
        }
        self.drafts
            .insert(key, (text, self.composer_images.clone()));
    }

    /// 恢复目标会话的草稿（`open_session` / `start_new_session` 设好键之后调用）。
    ///
    /// 经窗口句柄现取 `&mut Window`：`Context` 里没有 window，而
    /// `TextareaState::set_value` 需要它（与 `backfill_settings_fields` 同一手法，
    /// 不依赖 render —— 隐藏窗口下也必须生效）。
    pub(crate) fn restore_composer_draft(&mut self, cx: &mut Context<Self>) {
        let key = self.current_session.clone().unwrap_or_default();
        let (text, images) = self.drafts.get(&key).cloned().unwrap_or_default();
        self.composer_images = images;
        let handle = self.window_handle;
        let input = self.input.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        });
    }

    /// 新建会话：清空选中与消息。真正的会话记录由服务端在首条消息时按本地
    /// 生成的 id 创建（事件按 `session_id` 路由，id 必须发起前就确定）。
    pub(crate) fn start_new_session(&mut self, cx: &mut Context<Self>) {
        // 先存草稿：切走再切回写了一半的内容还在（与 Web 版 ChatInput 同语义）
        self.save_composer_draft(cx);
        self.current_session = None;
        self.streaming = false;
        self.session_model = None;
        self.model_menu_open = false;
        self.thinking = None;
        self.thinking_menu_open = false;
        self.ctx_live = None;
        self.ctx_window_live = 0;
        self.ctx_panel_open = false;
        self.ctx_compress = None;
        // 附件也按会话存草稿（上面已 save），这里清空后由 restore 填回本档的草稿
        // ——不会把上一会话的附件误发到新会话，也不会丢掉自己写了一半的内容
        self.composer_images.clear();
        self.restore_composer_draft(cx);
        self.messages = Rc::new(Vec::new());
        self.msg_list_state = GpuiListState::new(0, ListAlignment::Bottom, px(120.));
        self.message_total = 0;
        self.current_title = "新会话（发送首条消息后创建）".to_string();
        self.rebuild_session_rows(cx);
    }

    /// 选中会话：切换路由键并拉消息
    pub(crate) fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        // 切换前先存当前会话的草稿（写了一半的 prompt 不能因为点另一个会话就没了）
        if self.current_session.as_deref() != Some(id.as_str()) {
            self.save_composer_draft(cx);
        }
        self.current_session = Some(id.clone());
        // 浏览器式会话 Tab：打开过的会话留在顶部（去重；超上限丢最早且非当前的）
        if !self.open_tabs.iter().any(|t| t == &id) {
            self.open_tabs.push(id.clone());
            while self.open_tabs.len() > MAX_SESSION_TABS {
                let victim = self
                    .open_tabs
                    .iter()
                    .find(|t| **t != id)
                    .cloned()
                    .unwrap_or_else(|| self.open_tabs[0].clone());
                self.open_tabs.retain(|t| t != &victim);
            }
        }
        // 切会话即放弃上一会话的流式状态（本客户端只渲染当前会话的增量）
        self.streaming = false;
        // 会话级模型 / 上下文进度都是**按会话**的：切走即回落（模型回到 preferences.chat）
        self.session_model = None;
        self.model_menu_open = false;
        self.thinking = None;
        self.thinking_menu_open = false;
        // 实时用量与面板状态按会话重置；占用本身由新会话的消息 usage 恢复
        self.ctx_live = None;
        self.ctx_window_live = 0;
        self.ctx_panel_open = false;
        self.ctx_compress = None;
        // 统计行是按会话的：切走即清空，由新会话的两次 GET 重新填
        self.session_usage = None;
        self.request_perf = None;
        // 会话语义的面板数据先清空（避免看着新会话却显示上一条的待办/审批），
        // 再**立即**拉一次 —— 否则要等 3s 轮询窗口，体感就是"切过去慢一拍"
        self.todos.clear();
        self.goals.clear();
        self.pending_approvals.clear();
        // 文件页目录树按会话绑定目录解析：切会话必须作废，否则会继续显示
        // 上一条会话（或全局默认）的目录
        self.ws_root.clear();
        self.ws_entries.clear();
        self.ws_file = None;
        self.ws_path.clear();
        // 会话级模型 / 思考强度的权威值在会话头里：拉回来覆盖上面对默认值的回落
        self.load_session_model(id.clone(), cx);
        self.load_messages(id, cx);
        self.load_session_usage(cx);
        self.load_todos_goals(cx);
        self.load_approvals(cx);
        // 选中态与「当前会话始终可见」都体现在左栏行上
        self.restore_composer_draft(cx);
        self.rebuild_session_rows(cx);
    }

    /// 打开「重命名会话」对话框：新建输入框实体并回填原标题。
    ///
    /// `InputState` 没有公开的 `set_value`，所以每次开弹窗新建一个实体，
    /// 而不是复用一个再回填（复用的写法编译不过）。
    pub(crate) fn open_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .sessions_data
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.dialog_input = Some(cx.new(|cx| InputState::new(window, cx).default_value(title)));
        self.dialog = Some(Dialog::RenameSession { id });
        cx.notify();
    }

    /// 提交重命名（`POST /sessions/{id}/title`），成功后就地更新左栏标题。
    pub(crate) fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::RenameSession { id }) = self.dialog.clone() else {
            return;
        };
        let title = self
            .dialog_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.dialog = None;
        self.dialog_input = None;
        if title.is_empty() {
            cx.notify();
            return;
        }
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}/title");
            match post_json(url, serde_json::json!({ "title": title })).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(s) = this.sessions_data.iter_mut().find(|s| s.id == id) {
                            s.title = title.clone();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("重命名失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 确认删除会话（`DELETE /sessions/{id}`）。
    ///
    /// 删除的是当前会话时清空主区（消息 / 统计 / 待办 / 目标），回到「选一个会话」空态。
    pub(crate) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::DeleteSession { id, .. }) = self.dialog.clone() else {
            return;
        };
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{id}");
            match delete_json(url).await {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sessions_data.retain(|s| s.id != id);
                        // 该会话的输入草稿随会话一起清掉（否则会话列表里删了、
                        // 草稿还挂在 HashMap 里，长期不退出会缓涨）
                        this.drafts.remove(&id);
                        // 打开过的 Tab 也一并移除（否则 Tab 条上留一个点不开的幽灵）
                        this.open_tabs.retain(|t| t != &id);
                        if this.current_session.as_deref() == Some(id.as_str()) {
                            this.current_session = None;
                            this.messages = Rc::new(Vec::new());
                            this.msg_list_state =
                                GpuiListState::new(0, ListAlignment::Bottom, px(120.));
                            this.message_total = 0;
                            this.current_title = "选择左侧会话查看消息".to_string();
                            this.session_usage = None;
                            this.request_perf = None;
                            this.todos.clear();
                            this.goals.clear();
                        }
                        this.rebuild_session_rows(cx);
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("删除失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 请求回退到某条用户消息之前：先弹确认框（删除不可恢复）。
    pub(crate) fn request_rollback(
        &mut self,
        session_id: String,
        message_id: String,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(Dialog::RollbackMessage {
            session_id,
            message_id,
            preview: ellipsis(text.trim(), 16),
        });
        cx.notify();
    }

    /// 确认回退：`POST /sessions/{id}/messages/delete`（删该消息及其后全部；
    /// ADR-040 事务内含工作区文件回退），成功后重拉消息链与会话统计。
    pub(crate) fn confirm_rollback(&mut self, session_id: String, message_id: String, cx: &mut Context<Self>) {
        self.dialog = None;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url =
                format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{session_id}/messages/delete");
            let res = post_json(url, serde_json::json!({ "message_id": message_id })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.set_status("已回退".to_string());
                        this.load_messages(session_id.clone(), cx);
                        this.load_session_usage(cx);
                        this.load_sessions(cx);
                    }
                    Err(e) => this.set_error(format!("回退失败：{e}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 左栏的一行（分组 / 会话 / 查看更多）。
    ///
    /// 行高由内容决定，不再用等高虚拟列表：三类行的高度本来就不一样，而
    /// `List` 的测量缓存会把「查看更多」压到上一行标题上（实测重叠）。
    pub(crate) fn render_session_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.session_rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            SessionListRow::Group {
                key,
                label,
                count,
                collapsed,
            } => {
                let chevron = if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                };
                div()
                    .id(("ws-group", ix))
                    .w_full()
                    .px_1()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(chevron.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        svg()
                            .path(IconName::FolderOpen.path())
                            .size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{count}")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_group(key.clone(), cx);
                    }))
                    .into_any_element()
            }
            SessionListRow::Session {
                id,
                title,
                meta,
                tasks,
                selected,
            } => {
                // 行内操作按钮要带走 id / 标题（`title` 随后会被 move 进标题节点）
                let rename_id = id.clone();
                let delete_id = id.clone();
                let hover_id = id.clone();
                let delete_title = title.clone();
                // 悬停才显示行内操作（对齐 Web 版）；容器宽度固定，
                // 避免鼠标进出时标题区反复变宽变窄
                let is_hovered = self.hovered_session.as_deref() == Some(id.as_str());
                let mut head = div().flex().flex_row().items_center().gap_2().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(rgb(if selected {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_PRIMARY()
                        }))
                        .child(if title.is_empty() {
                            "新对话".to_string()
                        } else {
                            title
                        }),
                );
                if tasks > 0 {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .bg(rgb(colors::ERROR()))
                            .text_size(px(9.))
                            .text_color(rgb(0xffffff))
                            .child(if tasks > 99 {
                                "99+".to_string()
                            } else {
                                format!("{tasks}")
                            }),
                    );
                }
                // 行内操作：重命名 / 删除。**只在悬停这一行时渲染**（对齐 Web 版
                // `hoveredSession === session.id`），且**不预留宽度**——留着 50px
                // 空位的话，标题在非悬停时就被白占了，hover 与不 hover 看起来没差别
                // （用户指出过）。代价是悬停时标题区会窄 50px，与 Web 版一致。
                // 两个回调都 `stop_propagation()`——整行的 on_click 是「打开会话」，
                // 不拦住会把弹窗和切会话一起触发。
                head = head.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(if is_hovered {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("rename", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Pencil.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY())),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.open_rename(rename_id.clone(), window, cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id(("delete", ix))
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .child(
                                            svg()
                                                .path(IconName::Trash.path())
                                                .size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY())),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.dialog = Some(Dialog::DeleteSession {
                                                id: delete_id.clone(),
                                                title: delete_title.clone(),
                                            });
                                            cx.notify();
                                        })),
                                )
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                );
                div()
                    .id(("ws-session", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(if selected {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(head)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(meta),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id.clone(), cx);
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        // 行内操作按钮只在悬停这一行时渲染（对齐 Web 版口径）
                        let next = if *hovered {
                            Some(hover_id.clone())
                        } else {
                            None
                        };
                        if this.hovered_session != next {
                            this.hovered_session = next;
                            cx.notify();
                        }
                    }))
                    .into_any_element()
            }
            SessionListRow::More { key, hidden } => div()
                .id(("ws-more", ix))
                .w_full()
                .pl_4()
                .pr_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("查看更多")
                .child(
                    div()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("({hidden})")),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.expand_group(key.clone(), cx);
                }))
                .into_any_element(),
        }
    }

    /// 顶部会话 Tab 条（对齐 vite `SessionTabBar`）：打开的会话横向排列、当前项高亮、
    /// 每项带关闭按钮；没有任何 tab 时退回原来的单行标题（不占额外高度）。
    ///
    /// 单独成方法：`render_chat` 的栈帧已经很紧（GPUI 递归 prepaint，
    /// 单帧过大会 `has overflowed its stack`），这里不再往它体内塞嵌套。
    pub(crate) fn render_session_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.open_tabs.is_empty() {
            return div()
                .px_3()
                .py_2()
                .text_sm()
                .border_b_1()
                .border_color(rgb(colors::BORDER()))
                .child(self.current_title.clone())
                .into_any_element();
        }
        let mut row = div()
            .w_full()
            .min_w_0()
            .px_2()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .overflow_hidden();
        for id in &self.open_tabs {
            let is_cur = self.current_session.as_deref() == Some(id.as_str());
            let title = self
                .sessions_data
                .iter()
                .find(|s| &s.id == id)
                .map(|s| ellipsis(&s.title, 12))
                .unwrap_or_else(|| "会话".to_string());
            let id_open = id.clone();
            let id_close = id.clone();
            row = row.child(
                div()
                    .id(format!("session-tab-{id}"))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .whitespace_nowrap()
                    .bg(rgb(if is_cur {
                        colors::BG_TERTIARY()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .text_size(px(12.))
                    .text_color(rgb(if is_cur {
                        colors::TEXT_PRIMARY()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(title)
                    .child(
                        div()
                            .id(format!("session-tab-x-{id}"))
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
                                this.close_tab(id_close.clone(), cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        if this.current_session.as_deref() != Some(id_open.as_str()) {
                            this.open_session(id_open.clone(), cx);
                        }
                    })),
            );
        }
        row.into_any_element()
    }

    /// 关闭一个会话 Tab：若是当前会话则切到剩余 tab 的最后一个（没有则回空态）。
    pub(crate) fn close_tab(&mut self, id: String, cx: &mut Context<Self>) {
        self.open_tabs.retain(|t| t != &id);
        if self.current_session.as_deref() == Some(id.as_str()) {
            match self.open_tabs.last().cloned() {
                Some(next) => self.open_session(next, cx),
                None => self.start_new_session(cx),
            }
        } else {
            cx.notify();
        }
    }
}
