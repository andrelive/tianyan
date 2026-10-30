//! 左栏会话列表：分组、折叠、增删改、多会话 Tab。
//!
//! 派生关系：`sessions_data`（服务端原样）+ 折叠态 + 选中态 → `session_rows`
//! （渲染用的行）。任何一项变化都要 `rebuild_session_rows` 重算，不就地改行。

use crate::*;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::theme::colors;
use crate::types::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputState, TextareaState};

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

    /// 切会话时收起**与会话/输入框绑定**的弹窗。
    ///
    /// 为什么必须收：`DeleteSession` / `RenameSession` / `RollbackMessage` 都带具体的
    /// `session_id`，留在界面上会让用户以为在操作当前会话——**点确认删的却是上一个**
    /// （实测过的坑：开着「删除会话 A」的框切到 B，框还在）。`WorkspacePicker` 与
    /// `FilePicker` 的目标是「当前会话 / 当前输入框」，切完会话语义已变，同样要收。
    /// `ResetRole` / `DeleteRole` 是角色页的，与会话无关，**不清**。
    fn dismiss_session_scoped_dialogs(&mut self) {
        if matches!(
            self.dialog,
            Some(Dialog::DeleteSession { .. })
                | Some(Dialog::RenameSession { .. })
                | Some(Dialog::RollbackMessage { .. })
                | Some(Dialog::WorkspacePicker { .. })
                | Some(Dialog::FilePicker { .. })
        ) {
            self.dialog = None;
            self.dialog_input = None;
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // 会话视图状态：park / restore（机制与理由见 `SessionSnapshot` 的文档注释）
    // ────────────────────────────────────────────────────────────────────

    /// 把当前会话的视图状态搬进 `parked`，并把这些字段清回默认。
    ///
    /// 必须在**切走之前**调用。键 = `current_session`（`None` = 还没发首条消息的
    /// 新会话——它同样有主屏/面板状态，不能因为没 id 就丢）。
    pub(crate) fn park_view_state(&mut self) {
        // ⚠️ 新会话（`current_session == None`）**不 park**。
        // `start_new_session` 是「先 park 本会话、再按新键 restore」：两次都命中
        // `None` 键就等于自存自读，界面还是上一份内容。旧 `drafts` 方案栽过同一个
        // 坑（「点新建会话，输入框里还是上一段文字」），换实现时这条结论**必须**
        // 跟着搬过来——实测：实体化后忘搬，同一 bug 立刻复现。
        let Some(key) = self.current_session.clone() else {
            self.reset_view_state();
            return;
        };
        // 完全空的会话不值得占 LRU 槽位（空白会话之间来回切时别把有用的挤掉）
        let empty = self.messages.is_empty()
            && self.message_total == 0
            && self.todos.is_empty()
            && self.goals.is_empty()
            && self.pending_approvals.is_empty()
            && self.ws_root.is_empty();
        if empty {
            self.reset_view_state();
            return;
        }
        // ⚠️ 穷尽构造：新增 per-session 字段若不加进 `SessionSnapshot`，这里编译报错
        // ——这是「漏一个补一个」的机制性防线，**不要**改成 `..Default::default()`。
        let snap = SessionSnapshot {
            messages: std::mem::replace(&mut self.messages, Rc::new(Vec::new())),
            // `ListState` 内部是 `Rc<RefCell<..>>`：clone 会共享滚动位置，必须换走
            msg_list_state: Some(std::mem::replace(
                &mut self.msg_list_state,
                GpuiListState::new(0, ListAlignment::Bottom, px(120.)),
            )),
            // 同上：`ExpandRef` 也是 `Rc` 共享语义
            expand: Some(std::mem::replace(
                &mut self.expand,
                Rc::new(std::cell::RefCell::new(crate::msg::ExpandState::default())),
            )),
            message_total: std::mem::take(&mut self.message_total),
            oldest_seq: self.oldest_seq.take(),
            has_more_history: std::mem::take(&mut self.has_more_history),
            loading_older: std::mem::take(&mut self.loading_older),
            composer_images: std::mem::take(&mut self.composer_images),
            session_model: self.session_model.take(),
            thinking: self.thinking.take(),
            ctx_live: self.ctx_live.take(),
            ctx_window_live: std::mem::take(&mut self.ctx_window_live),
            ctx_panel_open: std::mem::take(&mut self.ctx_panel_open),
            ctx_compress: self.ctx_compress.take(),
            session_usage: self.session_usage.take(),
            request_perf: self.request_perf.take(),
            todos: std::mem::take(&mut self.todos),
            goals: std::mem::take(&mut self.goals),
            todo_bar_collapsed: std::mem::take(&mut self.todo_bar_collapsed),
            tasks: std::mem::take(&mut self.tasks),
            tasks_collapsed: std::mem::take(&mut self.tasks_collapsed),
            expanded_task: self.expanded_task.take(),
            task_messages: std::mem::take(&mut self.task_messages),
            pending_approvals: std::mem::take(&mut self.pending_approvals),
            clarification: self.clarification.take(),
            clarify_answers: std::mem::take(&mut self.clarify_answers),
            ws_root: std::mem::take(&mut self.ws_root),
            ws_path: std::mem::take(&mut self.ws_path),
            ws_entries: std::mem::take(&mut self.ws_entries),
            ws_file: self.ws_file.take(),
            ws_status: std::mem::take(&mut self.ws_status),
            ws_diff_open: std::mem::take(&mut self.ws_diff_open),
            ws_diff_files: std::mem::take(&mut self.ws_diff_files),
            ws_diff: self.ws_diff.take(),
            ws_diff_status: std::mem::take(&mut self.ws_diff_status),
            current_title: std::mem::take(&mut self.current_title),
            new_session_workspace: self.new_session_workspace.take(),
        };
        // LRU：同键先丢弃旧快照，最近使用的排到末尾
        self.parked
            .retain(|(k, _)| k.as_deref() != Some(key.as_str()));
        self.parked.push((Some(key), snap));
        if self.parked.len() > MAX_PARKED_SESSIONS {
            self.parked.remove(0);
        }
        self.streaming = false;
    }

    /// 把视图状态清回默认（首次进入某会话 / park 完一个空会话）。
    pub(crate) fn reset_view_state(&mut self) {
        self.messages = Rc::new(Vec::new());
        self.msg_list_state = GpuiListState::new(0, ListAlignment::Bottom, px(120.));
        self.expand = Rc::new(std::cell::RefCell::new(crate::msg::ExpandState::default()));
        self.message_total = 0;
        self.oldest_seq = None;
        self.has_more_history = false;
        self.loading_older = false;
        self.composer_images.clear();
        self.session_model = None;
        self.thinking = None;
        self.ctx_live = None;
        self.ctx_window_live = 0;
        self.ctx_panel_open = false;
        self.ctx_compress = None;
        self.session_usage = None;
        self.request_perf = None;
        self.todos.clear();
        self.goals.clear();
        self.todo_bar_collapsed = false;
        self.tasks.clear();
        self.tasks_collapsed = false;
        self.expanded_task = None;
        self.task_messages.clear();
        self.pending_approvals.clear();
        self.clarification = None;
        self.clarify_answers.clear();
        self.ws_root.clear();
        self.ws_path.clear();
        self.ws_entries.clear();
        self.ws_file = None;
        self.ws_status.clear();
        self.ws_diff_open = false;
        self.ws_diff_files.clear();
        self.ws_diff = None;
        self.ws_diff_status.clear();
        self.current_title = "选择左侧会话查看消息".to_string();
        self.new_session_workspace = None;
        self.streaming = false;
    }

    /// 取回目标会话的快照并填入 `self`。
    ///
    /// 返回是否**命中**：`false` = 首次进入该会话（调用方需要去拉消息）；
    /// `true` = 状态已恢复（调用方**不应**再 `load_messages`，否则 `scroll_to_end`
    /// 会把刚恢复的滚动位置冲掉）。
    pub(crate) fn restore_view_state(&mut self) -> bool {
        let key = self.current_session.clone();
        let Some(i) = self.parked.iter().position(|(k, _)| k == &key) else {
            self.reset_view_state();
            return false;
        };
        let (_, snap) = self.parked.remove(i);
        self.messages = snap.messages;
        self.msg_list_state = snap
            .msg_list_state
            .unwrap_or_else(|| GpuiListState::new(0, ListAlignment::Bottom, px(120.)));
        self.expand = snap.expand.unwrap_or_else(|| {
            Rc::new(std::cell::RefCell::new(crate::msg::ExpandState::default()))
        });
        self.message_total = snap.message_total;
        self.oldest_seq = snap.oldest_seq;
        self.has_more_history = snap.has_more_history;
        self.loading_older = snap.loading_older;
        self.composer_images = snap.composer_images;
        self.session_model = snap.session_model;
        self.thinking = snap.thinking;
        self.ctx_live = snap.ctx_live;
        self.ctx_window_live = snap.ctx_window_live;
        self.ctx_panel_open = snap.ctx_panel_open;
        self.ctx_compress = snap.ctx_compress;
        self.session_usage = snap.session_usage;
        self.request_perf = snap.request_perf;
        self.todos = snap.todos;
        self.goals = snap.goals;
        self.todo_bar_collapsed = snap.todo_bar_collapsed;
        self.tasks = snap.tasks;
        self.tasks_collapsed = snap.tasks_collapsed;
        self.expanded_task = snap.expanded_task;
        self.task_messages = snap.task_messages;
        self.pending_approvals = snap.pending_approvals;
        self.clarification = snap.clarification;
        self.clarify_answers = snap.clarify_answers;
        self.ws_root = snap.ws_root;
        self.ws_path = snap.ws_path;
        self.ws_entries = snap.ws_entries;
        self.ws_file = snap.ws_file;
        self.ws_status = snap.ws_status;
        self.ws_diff_open = snap.ws_diff_open;
        self.ws_diff_files = snap.ws_diff_files;
        self.ws_diff = snap.ws_diff;
        self.ws_diff_status = snap.ws_diff_status;
        self.current_title = snap.current_title;
        self.new_session_workspace = snap.new_session_workspace;
        // `streaming` 不随快照走：它由全局 `running_sessions` 决定，调用方设置
        true
    }

    /// 把当前输入框实体存进 `inputs`（`Entity` 是句柄，拷贝不复制状态）。
    ///
    /// 必须在**切走之前**调用。键 = `current_session`（`None` = 未落库的新会话，
    /// 它同样有自己的输入框）。
    pub(crate) fn park_input(&mut self) {
        // 同上：新会话不 park（否则「新建会话 → 输入 → 再点新建」会读回自己）。
        let Some(key) = self.current_session.clone() else {
            return;
        };
        self.inputs
            .retain(|(k, _)| k.as_deref() != Some(key.as_str()));
        self.inputs.push((Some(key), self.input.clone()));
        if self.inputs.len() > MAX_PARKED_SESSIONS {
            self.inputs.remove(0);
        }
    }

    /// 换到目标会话的输入框实体。
    ///
    /// **没有就新建** —— 不能复用当前实体，否则两个会话共享同一份文本（那正是旧
    /// `drafts` 方案要绕开的坑）。新建后若切换前焦点在输入框，把焦点还回去，
    /// 否则用户切一次会话就得重新点一下输入框。
    pub(crate) fn restore_input(
        &mut self,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        // 切换前焦点是否在输入框。
        // 走 `FocusHandle::is_focused(&Window)`：`InputBaseState` 本身没有 `is_focused`
        // （实测 `no method named is_focused`），但它有固有的 `focus_handle()`。
        let had_focus = window
            .as_ref()
            .map(|w| self.input.read(cx).focus_handle(cx).is_focused(w))
            .unwrap_or(false);
        let key = self.current_session.clone();
        if let Some(i) = self.inputs.iter().position(|(k, _)| k == &key) {
            // 有现成实体：换句柄（文本 / 光标 / undo 历史都在里面）
            let (_, entity) = self.inputs.remove(i);
            self.input = entity;
            self.draft_restore_path = "restored".to_string();
        } else {
            // 新建：每个会话必须有自己的实体
            let placeholder = "输入消息 · Enter 发送 / Shift+Enter 换行";
            // 注意用 `as_mut()`（借而不是 move）：后面恢复焦点还要再用一次
            if let Some(w) = window.as_mut() {
                let w: &mut Window = w;
                self.input = cx.new(|cx| TextareaState::new(w, cx).placeholder(placeholder));
                self.draft_restore_path = "created-window".to_string();
            } else {
                // 自动化/后台路径没有 `&mut Window`（也不在窗口借用中）→ 现取
                let handle = self.window_handle;
                let mut created: Option<Entity<TextareaState>> = None;
                if let Err(e) = handle.update(cx, |_, w, cx| {
                    created = Some(cx.new(|cx| TextareaState::new(w, cx).placeholder(placeholder)));
                }) {
                    self.set_error(format!("输入框创建失败（窗口不可用）：{e}"));
                }
                if let Some(e) = created {
                    self.input = e;
                    self.draft_restore_path = "created-handle".to_string();
                }
            }
        }
        if had_focus {
            if let Some(w) = window.as_mut() {
                let w: &mut Window = w;
                self.input.update(cx, |state, cx| state.focus(w, cx));
            }
        }
        // 订阅必须跟着**当前**这个实体走：上面可能刚换了实体（取回或新建），
        // 旧订阅挂在被弃用的那个上，Enter 就会只换行、不发送（见 `bind_composer_submit`）。
        self.bind_composer_submit(window, cx);
    }

    /// 新建会话：清空选中与消息。真正的会话记录由服务端在首条消息时按本地
    /// 生成的 id 创建（事件按 `session_id` 路由，id 必须发起前就确定）。
    pub(crate) fn start_new_session(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        // 先存草稿：切走再切回写了一半的内容还在（与 Web 版 ChatInput 同语义）
        self.park_input();
        // park 旧会话的视图状态（键 = 旧的 current_session），再切到「新会话」
        self.park_view_state();
        self.dismiss_session_scoped_dialogs();
        self.current_session = None;
        // 「新会话」也有一份自己的视图状态（键 = None）：之前 park 过就恢复，
        // 首次则清默认。这样「新建会话 → 输入 → 切走 → 再点新建」不会带着上一次
        // 新建会话的面板/折叠状态。
        let restored = self.restore_view_state();
        if !restored || self.current_title.trim().is_empty() {
            self.current_title = "新会话（发送首条消息后创建）".to_string();
        }
        self.streaming = false;
        // 弹层是 UI 瞬态，不随会话恢复
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        // 附件也按会话存草稿（上面已 save），这里清空后由 restore 填回本档的草稿
        // ——不会把上一会话的附件误发到新会话，也不会丢掉自己写了一半的内容
        self.composer_images.clear();
        self.restore_input(window, cx);
        self.rebuild_session_rows(cx);
    }

    /// 选中会话：切换路由键并拉消息
    pub(crate) fn open_session(
        &mut self,
        id: String,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        // 切换前先存当前会话的草稿（写了一半的 prompt 不能因为点另一个会话就没了）
        // + park 视图状态（滚动位置 / 折叠 / 分页游标 / 面板）
        if self.current_session.as_deref() != Some(id.as_str()) {
            self.park_input();
            self.park_view_state();
            // 针对具体会话的弹窗不能跟着切过去（否则会误删上一个会话）
            self.dismiss_session_scoped_dialogs();
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
        // 恢复该会话的视图状态（滚动位置 / 消息折叠态 / 分页游标 / 上下文环 /
        // 会话面板 / 文件页目录）。**首次进入**（无快照）→ 清默认，随后由 load_* 拉。
        // 机制与「为什么必须整组搬」见 `SessionSnapshot` 的文档注释。
        let restored = self.restore_view_state();
        // 轮状态来自**全局**集合（跨会话语义），不随快照走：切走期间轮没停，
        // 切回来按钮必须还是「停止」（用户实测过它退回「发送」）
        let running = self.running_sessions.contains(&id);
        self.streaming = running;
        // 弹层（模型 / 思考选择器）是 UI 瞬态，切会话一律收起
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        // 会话级模型 / 思考强度的权威值在会话头里：拉回来覆盖快照里的旧值
        self.load_session_model(id.clone(), cx);
        // 消息：**有快照就不重拉** —— 这份快照现在真的是完整的。
        //
        // 关键前提是 `apply_event_offline`：切走期间该会话的流式事件会按 `session_id`
        // 写回它自己那份快照（对齐 vite 的 per-session store），所以 restore 出来就是
        // 最新状态，不需要再用「整页重拉 + 按 seq 对齐」去猜 —— 那套猜测正是此前
        // 「切回来内容只剩思考块 / 上滚的历史被 50 条截断 / 位置被钉在旧处」的来源。
        //
        // 唯一要兜的情况：最后一条 assistant **还没有 id**（= 这一轮没以权威消息收尾，
        // 多半是切走期间 SSE 断过、事件真的丢了）。这时才拉一页对齐尾部；那一轮还在
        // 跑（`running`）则不动 —— 占位是它的落点，等权威消息整条覆盖即可。
        if !restored {
            self.load_messages(id, cx);
        } else if !running {
            let stale_tail = self
                .messages
                .last()
                .map(|m| m.role == "assistant" && m.id.is_empty())
                .unwrap_or(false);
            if stale_tail {
                let limit = self.messages.len().clamp(MESSAGE_PAGE, 200);
                self.load_messages_with(id, limit, None, cx);
            }
        }
        self.load_session_usage(cx);
        self.load_todos_goals(cx);
        self.load_approvals(cx);
        // 选中态与「当前会话始终可见」都体现在左栏行上
        self.restore_input(window, cx);
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
                        // 该会话的输入框实体也不能留着（否则删掉的会话还能靠它"复活"输入内容）
                        this.inputs
                            .retain(|(k, _)| k.as_deref() != Some(id.as_str()));
                        // 轮状态记账同理：会话没了，那个 id 不该再占集合
                        this.running_sessions.remove(&id);
                        // 视图快照也要清：否则删掉的会话还能靠快照"复活"一份幽灵状态，
                        // 且白占 LRU 槽位
                        this.parked
                            .retain(|(k, _)| k.as_deref() != Some(id.as_str()));
                        // 打开过的 Tab 也一并移除（否则 Tab 条上留一个点不开的幽灵）
                        this.open_tabs.retain(|t| t != &id);
                        if this.current_session.as_deref() == Some(id.as_str()) {
                            this.current_session = None;
                            this.streaming = false;
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
    pub(crate) fn confirm_rollback(
        &mut self,
        session_id: String,
        message_id: String,
        cx: &mut Context<Self>,
    ) {
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_session(id.clone(), Some(window), cx);
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // ⚠️ 必须吞掉冒泡：外层 tab 的 on_click 是「打开会话」，
                                // 它会把这个 id 重新 push 回 `open_tabs` —— 于是 close_tab
                                // 删掉、紧接着又被加回来，表现就是「点 × 关不掉」（实测）。
                                cx.stop_propagation();
                                this.close_tab(id_close.clone(), Some(window), cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.current_session.as_deref() != Some(id_open.as_str()) {
                            this.open_session(id_open.clone(), Some(window), cx);
                        }
                    })),
            );
        }
        row.into_any_element()
    }

    /// 关闭一个会话 Tab：若是当前会话则切到剩余 tab 的最后一个（没有则回空态）。
    pub(crate) fn close_tab(
        &mut self,
        id: String,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.open_tabs.retain(|t| t != &id);
        if self.current_session.as_deref() == Some(id.as_str()) {
            match self.open_tabs.last().cloned() {
                Some(next) => self.open_session(next, window, cx),
                None => self.start_new_session(window, cx),
            }
        } else {
            cx.notify();
        }
    }
}
