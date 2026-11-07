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
use gpui_kit::component::input::InputState;

/// [`ClientApp::restore_view_state`] 的结果：调用方据此决定「要不要去服务端拉一页」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreOutcome {
    /// 没有快照（首次打开该会话 / 被 LRU 挤掉）—— 本地什么都没有，必须拉。
    Miss,
    /// 有快照，但**不能**当权威内容用（跨代：断线重连过，或此刻就是断开态）—— 必须拉。
    Stale,
    /// 快照就是当前内容（同代 + 在线 + 消息非空）—— 切回零请求，直接用投影。
    Fresh,
}

/// 「切回会话能否直接用本地投影」的判据（纯函数，便于单测钉住三个条件）。
///
/// 三个条件缺一不可：
/// - `snap_epoch == stream_epoch`：事件流**没换代** —— 切走期间的增量都由
///   `apply_event_offline`（以及非当前会话的快照帧）写进了这份快照，一条不少；
/// - `connected`：此刻在线 —— 断开态下「断线期间没丢事件」这个前提不成立；
/// - `message_count > 0`：快照真有内容 —— 空投影渲染出来就是「进会话什么都没有」。
///
/// 为什么不能只判「有快照」：那正是 (63) 那条被实测推翻的旧口径
/// （「切回只看到部分工具步骤」—— 快照因 LRU 或断线停在旧位置却一直显示）。
pub(crate) fn projection_usable(
    snap_epoch: u64,
    stream_epoch: u64,
    connected: bool,
    message_count: usize,
) -> bool {
    connected && snap_epoch == stream_epoch && message_count > 0
}

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

    /// 该会话此刻是否在等用户回答追问（`ask_user` 是同步工具，执行会挂起）。
    ///
    /// 两处来源：当前会话看 `self.clarification`；**切走过**的会话看它自己那份
    /// 快照（`parked`）—— 追问态随 `SessionSnapshot` 走，所以切回来还在，
    /// 顶部 Tab 上那个「等待你的回答」问号也要能看见（vite `SessionTabBar`
    /// 的 `pendingAsk` 同语义）。
    pub(crate) fn session_awaiting_answer(&self, id: &str) -> bool {
        if self.current_session.as_deref() == Some(id) {
            return self.clarification.is_some();
        }
        self.parked
            .iter()
            .any(|(k, s)| k.as_deref() == Some(id) && s.clarification.is_some())
    }

    /// 把这个会话放进顶部 Tab 条（去重 + 超上限丢最早的**非当前**项）。
    ///
    /// 两个调用点必须共用这一份逻辑：`open_session`（点会话）与 `send_text`
    /// （**新会话首轮直接发消息**——它没走过 `open_session`）。早先漏了后者，
    /// 于是新会话期间顶部只剩一行标题，「运行中」绿点无处显示 ——
    /// 用户报的「顶部没有活跃提示」正是这条。
    pub(crate) fn ensure_tab(&mut self, id: &str) {
        if self.open_tabs.iter().any(|t| t == id) {
            return;
        }
        self.open_tabs.push(id.to_string());
        while self.open_tabs.len() > MAX_SESSION_TABS {
            let victim = self
                .open_tabs
                .iter()
                .find(|t| t.as_str() != id)
                .cloned()
                .unwrap_or_else(|| self.open_tabs[0].clone());
            self.open_tabs.retain(|t| t != &victim);
        }
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
                | Some(Dialog::FilePicker)
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
        // ⚠️ 必须在构造 `SessionSnapshot` **之前**取值。
        //
        // 结构体字面量的字段按**书写顺序**求值，而下面 `msg_list_state` 那一格会调
        // `new_msg_list_state(0, …)` —— 它内部会 `scrolled_up.set(false)`（新列表
        // 一律贴底）。字段顺序若写成「先 replace 再读」，读到的就是已经被清掉的
        // false，于是"用户滚离过底部"这个事实在 park 时就被抹掉，切回必然贴底。
        let msg_scrolled_up_at_park = self.msg_scrolled_up.get();
        // 诊断：park 那一刻的真实位置与意图。
        //
        // 前面几轮都在查「切回后为什么贴底」，日志却显示 restore 侧一切正常
        // （`scrolled_up=true`、偏移没被钳）。把 park 侧也打出来才能分清：
        // 究竟是「切回丢的」还是「切走前就已经在底部」。
        crate::app::diag_log(&format!(
            "[保位] park id={:?} len={} {} scrolled_up={}",
            self.current_session,
            self.messages.len(),
            pos_brief(&self.msg_list_state),
            msg_scrolled_up_at_park
        ));
        let snap = SessionSnapshot {
            // 记下 park 时的事件流代际：切回时据此判断这份投影还能不能当权威内容
            // （见 `projection_usable`；跨代说明断线重连过，缺口不可恢复）。
            epoch: self.stream_epoch,
            messages: std::mem::replace(&mut self.messages, Rc::new(Vec::new())),
            // `ListState` 内部是 `Rc<RefCell<..>>`：clone 会共享滚动位置，必须换走
            msg_list_state: Some(std::mem::replace(
                &mut self.msg_list_state,
                crate::app::new_msg_list_state(0, self.msg_scrolled_up.clone()),
            )),
            msg_scrolled_up: msg_scrolled_up_at_park,
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
            clarify_tab: std::mem::take(&mut self.clarify_tab),
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
        };
        // LRU：同键先丢弃旧快照，最近使用的排到末尾
        self.parked
            .retain(|(k, _)| k.as_deref() != Some(key.as_str()));
        self.parked.push((Some(key), snap));
        if self.parked.len() > MAX_PARKED_SESSIONS {
            self.parked.remove(0);
        }
        self.streaming = false;
        self.turn_running = false;
    }

    /// 把视图状态清回默认（首次进入某会话 / park 完一个空会话）。
    pub(crate) fn reset_view_state(&mut self) {
        self.messages = Rc::new(Vec::new());
        self.msg_list_state = crate::app::new_msg_list_state(0, self.msg_scrolled_up.clone());
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
        self.usage_shown = None;
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
        self.clarify_tab = 0;
        self.clarify_inputs.clear();
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
        self.streaming = false;
        self.turn_running = false;
    }

    /// 取回目标会话的快照并填入 `self`。
    ///
    /// 返回值说明这份快照**能不能当权威内容用**（见 [`RestoreOutcome`]）：
    /// `Fresh` = 状态已恢复且投影完整（调用方**不应**再 `load_messages`，否则
    /// `scroll_to_end` 会把刚恢复的滚动位置冲掉，还要白等一次往返）；
    /// `Miss` / `Stale` = 状态尽力恢复了，但调用方**必须**去服务端拉一页消息。
    pub(crate) fn restore_view_state(&mut self) -> RestoreOutcome {
        let key = self.current_session.clone();
        let Some(i) = self.parked.iter().position(|(k, _)| k == &key) else {
            self.reset_view_state();
            return RestoreOutcome::Miss;
        };
        let (_, snap) = self.parked.remove(i);
        // 判据必须在把 `snap` 拆开搬进 `self` **之前**算（代际与条数都在 snap 上）
        let outcome = if projection_usable(
            snap.epoch,
            self.stream_epoch,
            self.events_connected,
            snap.messages.len(),
        ) {
            RestoreOutcome::Fresh
        } else {
            RestoreOutcome::Stale
        };
        // ⚠️ **不信任快照里的 `ListState`**（这是一个会让内容「凭空消失」的坑）：
        //
        // 它的 `item_count` 是 **park 那一刻**的条数，而切走期间
        // `apply_event_offline` 会往这份 `messages` 里继续追加消息 —— 条数变了、
        // `item_count` 没变，虚拟列表就只渲染旧条数那么多项：**新内容明明在数据里，
        // 界面上却看不见**。用户实测的现象正是「输出完了又丢数据，新输入一条消息
        // 之后就恢复正常」（发送路径会 `reset()` 重建列表，所以一输入就好了）。
        //
        // 因此这里一律按当前条数**重建**（重建不丢阅读位置：下面用 park 时视线
        // 所在那条的 `seq` 找回来）。
        // 锚点 = (**消息 seq**, **该消息内的滚动偏移**)。
        //
        // 两个都要：只带 seq 时 `scroll_to(ix, 0)` 会把视口钉在那条消息的**顶部**——
        // 用户切走时若正看一条长回复的中段，切回来就整体上移（实测「切走时在底部、
        // 切回来比底部高一点」「位置越高偏得越多」）。`Top` 对齐下渲染只依赖
        // `(item_ix, offset_in_item)`，两者齐全才是像素级还原。
        let anchor = snap
            .msg_list_state
            .as_ref()
            .map(|ls| ls.logical_scroll_top());
        let anchor_seq = anchor
            .and_then(|off| snap.messages.get(off.item_ix))
            .map(|m| m.seq)
            .filter(|s| *s > 0);
        let anchor_offset = anchor.map_or(px(0.), |off| off.offset_in_item);
        // 偏移**不再钳到 1200px**（曾经的做法）：`offset_in_item` 是「视口顶部在那条
        // 消息内多深」。早先钳位的理由是「那条消息可能变矮，旧偏移超出自身高度会让
        // `layout_items` 把该项顶到负 y 之外」，但钳位本身制造了一个更常见的坏情况：
        //
        //   视口顶部落在**一条很长的消息内部**（工具卡片密集的会话，单条几千 px），
        //   上滚一屏后偏移常达数千；钳到 1200 之后，切回来就落在该消息的中下部 ——
        //   而这条消息往往就是最后一条，于是**看起来就是"又贴底了"**。
        //   （实测日志：len=151 / anchor_ix=150 / anchor_off=1200.0，钳位值原样出现）
        //
        // 现在用 `seq` 定位而不是 `item_ix`，即使偏移真的超界，最坏结果也只是
        // 「那条消息被顶出视口」，不会错位到别的消息上；而不钳位能保住长消息内部的
        // 真实位置。两害相权，不钳更稳。
        let anchor_offset = anchor_offset.max(px(0.));
        println!(
            "[消息] 切回定位: seq={anchor_seq:?} offset={:?} 条数={}",
            anchor_offset,
            self.messages.len()
        );
        self.messages = snap.messages;
        // 意图先落定：恢复锚点**不改变**「用户滚离过底部」这个事实，所以取快照里
        // 存的值，而不是下面那句按 `item_ix` 反推。
        let scrolled_up_at_park = snap.msg_scrolled_up;
        // ⚠️ **复用**快照里那个 `ListState`，不要新建。
        //
        // 这是「上滚一屏后切回必贴底」的根因所在，也是唯一不需要任何补帧动作的修法。
        // 引擎在 `List::request_layout` 里这样取首帧的视口高：
        //
        //     let available_height = state.last_layout_bounds
        //         .map(|b| b.size.height)     // 有上一次布局 → 真实视口高（实测 540px）
        //         .unwrap_or(state.overdraw); // 首帧没有它 → 拿 overdraw 顶替（我们传 1200px）
        //
        // 而 `layout_items` 末尾那条「回到尾部就恢复跟随」的判据是
        //
        //     if scroll_offset + available_height >= total_height - 1 { start_following() }
        //
        // 代入「视口顶落在最后一条内 `off` 处」的情形，它等价于 `off + available >= h_last - 1`。
        // 真实视口 540 下，只有真贴底（`off >= h_last - 540`）才成立；换成 1200 之后，
        // 实测值 `off=956 / h_last=2156` 给出 `956 + 1200 = 2156 >= 2155` —— **恰好成立**，
        // 引擎据此认定「用户已回到底部」并 `start_following()`。而 `is_following` 一旦为真，
        // 之后每帧都在 `layout_items` 开头把 `scroll_top` 设成「末尾之后」，整表贴底、
        // 再也不会自己回去。
        //
        // 这也解释了那个精确的阈值：中招区间是「距底 <= overdraw - 真实视口 = 660px」，
        // 所以**上滚一屏（540px）必现、多滚几屏反而正常**。
        //
        // 新建实例没有 `last_layout_bounds`，必然落进 overdraw 分支；复用快照实例则
        // 天然带着上一次布局的视口高与各项测量缓存 —— 首帧画出来就是对的，不需要
        // 「先不跟随、两帧后再钉一次」那种补丁（那会在屏幕上闪一下）。
        let n = self.messages.len();
        self.msg_list_state = match snap.msg_list_state {
            Some(ls) => {
                // 只对齐条数：等价于「末尾追加 / 末尾截断」，**不碰**已有项的测量缓存。
                // 不能用 `reset` / `reset_with_uniform_height` —— 它们走
                // `splice(0..old, n)`，会把每一项重建成 `Unmeasured`，测量缓存与首帧
                // 视口高一起没了（那正是上面那条 overdraw 分支被走到的原因）。
                let old = ls.item_count();
                if n > old {
                    ls.splice(old..old, n - old);
                } else if n < old {
                    ls.splice(n..old, 0);
                }
                ls
            }
            None => crate::app::new_msg_list_state(n, self.msg_scrolled_up.clone()),
        };
        if let Some(seq) = anchor_seq {
            if let Some(ix) = self.messages.iter().position(|m| m.seq == seq) {
                // 后续新内容/流式增量不得把用户拉回底部 —— 依据是**他切走前就在
                // 滚离状态**，不是"这条消息是不是最后一条"。
                //
                // 早先这里写的是 `set(ix + 1 < len)`：视口顶部落在最后一条长消息
                // 内部时 `ix == len - 1`，推导出 false，等于宣布"他在底部"，
                // 切回后任何跟随都会贴底（用户实测：上滚一屏必现，多滚几屏反而正常）。
                self.msg_scrolled_up.set(scrolled_up_at_park);
                // 定位分三步，顺序不能换（每步都在拆掉下一步的坑）：
                //
                // ① 重测锚点那条及它后面的。它们的测量缓存可能停在「内容还没长成
                //    现在这样」的那一刻（折叠态变过 / 异步渲染 / 流式中间态都算），
                //    而首帧 layout 会拿这个旧高度同时算 `rendered_height`（视口顶那条
                //    到末尾的累计）和 `summary().height`（总高）。两个一起偏小，于是
                //    「距底还有一屏」被读成「已经到底」：既把位置归一化成底部对齐，
                //    又 `start_following()` 开启跟随 ⇒ 下一帧整表贴底。
                //    实测就是 `(150, 836)` 在首帧被算成 `(147, 9.3)`。
                //
                // ② 清掉 ① 埋下的 `pending_scroll`。`remeasure_items` 会把它设成
                //    「当前锚点 + 像素偏移」，而首帧 layout 里有一句
                //        scroll_top.offset_in_item = offset.min(element_size.height);
                //    —— 拿**首帧渲染出的**高度去钳偏移。那个高度可能小于稳定高度
                //    （折叠态还没恢复 / 内容异步），一钳偏移就变小，接着又触发第 ① 条
                //    说的归一化。`scroll_to_end()` 是唯一会 `pending_scroll = None` 的
                //    公开入口。
                //
                // ③ 再定位。此时 `pending_scroll` 已是 None，`scroll_to` 的
                //    `rebase_pending_scroll` 对 None 是空操作，首帧拿到的偏移原样生效。
                //
                // 切走时本就在底部（`scrolled_up_at_park == false`）就停在 ② 的结果上：
                // 「末尾之后」既贴底又**不**停跟随（不能换成 `scroll_to` —— 它落在末尾
                // 之前会 `stop_following()`，之后新内容到达就不再自动贴底了）。
                if ix < n {
                    self.msg_list_state.remeasure_items(ix..n);
                }
                self.msg_list_state.scroll_to_end();
                if scrolled_up_at_park {
                    self.msg_list_state.scroll_to(gpui_kit::ListOffset {
                        item_ix: ix,
                        offset_in_item: anchor_offset,
                    });
                }
            } else {
                // 锚点那条已被压缩点替代/删掉：退回底部（`Top` 对齐不自动贴底）
                self.msg_list_state.scroll_to_end();
                self.msg_scrolled_up.set(false);
            }
        } else {
            // 无锚点 = 切走时停在底部（`logical_scroll_top` 指向末尾之后，取不到消息）：
            // 显式到底。**不能省**：`Top` 对齐下未指定位置等于「停在顶部」，
            // 这正是「切回会话停在中间/顶部」那个回归的来源。
            self.msg_list_state.scroll_to_end();
            self.msg_scrolled_up.set(false);
        }
        // 首帧 prepaint 要把列表内元素发来的 autoscroll 请求压掉（vendor 里的 patch）：
        // 焦点恢复会让可聚焦元素（卡片 / 复制按钮…）请求「把我滚进视口」，引擎会直接
        // 用那个位置覆盖刚恢复好的阅读位置 —— 实测 `(150, 776)` 被改成 `(147, 9.3)`，
        // 随后跟随又被带开，下一帧整表贴底。一次性抑制，之后交互不受影响。
        self.msg_list_state.suppress_autoscroll_once();
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
        // 恢复是「换了一份权威值」，不接上一个会话的动画中间帧
        self.usage_shown = None;
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
        self.clarify_tab = snap.clarify_tab;
        // 编辑框实体不进快照（Entity 不是可序列化/可复制的状态）：清空后由调用方
        // 的 `ensure_clarify_inputs(cx)` 按当前问题数重建（它需要 cx，而本函数只有 &mut self）
        self.clarify_inputs.clear();
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
        // `streaming` 不随快照走：它由全局 `running_sessions` 决定，调用方设置
        outcome
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
            .map(|w| self.input.read(cx).focus_handle.clone().is_focused(w))
            .unwrap_or(false);
        let key = self.current_session.clone();
        if let Some(i) = self.inputs.iter().position(|(k, _)| k == &key) {
            // 有现成实体：换句柄（文本 / 光标 / undo 历史都在里面）
            let (_, entity) = self.inputs.remove(i);
            self.input = entity;
            self.draft_restore_path = "restored".to_string();
        } else {
            // 新建：每个会话必须有自己的实体（自研输入框，高度自适应 1~8 行）
            // 注意用 `as_mut()`（借而不是 move）：后面恢复焦点还要再用一次
            if let Some(w) = window.as_mut() {
                let _w: &mut Window = w;
                self.input = cx.new(ComposerState::new);
                self.draft_restore_path = "created-window".to_string();
            } else {
                // 自动化/后台路径没有 `&mut Window`（也不在窗口借用中）→ 现取
                let handle = self.window_handle;
                let mut created: Option<Entity<ComposerState>> = None;
                if let Err(e) = handle.update(cx, |_, w, cx| {
                    let _ = w;
                    created = Some(cx.new(ComposerState::new));
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
        // `Miss` = 没有快照（首次点新建）→ 用默认标题；`Fresh` / `Stale` 都有标题
        // （`Stale` 只是消息要重拉，标题仍是这个会话的）
        if matches!(restored, RestoreOutcome::Miss) || self.current_title.trim().is_empty() {
            self.current_title = "新会话（发送首条消息后创建）".to_string();
        }
        self.streaming = false;
        self.turn_running = false;
        // 弹层是 UI 瞬态，不随会话恢复
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        // 附件也按会话存草稿（上面已 save），这里清空后由 restore 填回本档的草稿
        // ——不会把上一会话的附件误发到新会话，也不会丢掉自己写了一半的内容
        self.composer_images.clear();
        self.restore_input(window, cx);
        // 「新建会话」= 回到默认工作区：这里重置待绑定目录。
        //
        // 位置很关键：必须放在 `park_view_state` / `restore_view_state` **之后**——
        // 选目录新建的路径是 `start_new_session()` → 再设 `new_session_workspace`，
        // 谁后写谁生效（见 `confirm_workspace_picker` 的注释）。
        self.new_session_workspace = None;
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
        // 订阅用的 id 副本：下面 `load_messages(id, …)` 会把 `id` move 走
        let sid_for_subscribe = id.clone();
        // 浏览器式会话 Tab：打开过的会话留在顶部（去重；超上限丢最早且非当前的）
        self.ensure_tab(&id);
        // 恢复该会话的视图状态（滚动位置 / 消息折叠态 / 分页游标 / 上下文环 /
        // 会话面板 / 文件页目录）。**首次进入**（无快照）→ 清默认，随后由 load_* 拉。
        // 机制与「为什么必须整组搬」见 `SessionSnapshot` 的文档注释。
        let restored = self.restore_view_state();
        // 轮状态来自**全局**集合（跨会话语义），不随快照走：切走期间轮没停，
        // 切回来按钮必须还是「停止」（用户实测过它退回「发送」）
        let running = self.running_sessions.contains(&id);
        self.streaming = running;
        // 服务端权威那一路同源（与 `streaming` 并联进 `busy()`，见 `ClientApp::busy`）
        self.turn_running = running;
        // 弹层（模型 / 思考选择器）是 UI 瞬态，切会话一律收起
        self.model_menu_open = false;
        self.thinking_menu_open = false;
        // 会话级模型 / 思考强度的权威值在会话头里：拉回来覆盖快照里的旧值
        self.load_session_model(id.clone(), cx);
        // 消息：**投影可用就零请求**，否则拉一页权威内容。
        //
        // 判据在 [`RestoreOutcome`]：`Fresh` = 快照与当前事件流同一代 + 在线 + 有消息。
        // 同一代之内，切走期间的增量由 `apply_event_offline` 写回这份快照（一条不少），
        // 所以本地投影**就是**权威内容 —— 再拉一次不但白等一个往返，还会让消息区
        // 先闪一下（旧列表被整页替换、`GpuiListState` 重建）。
        //
        // 必须拉的三类：
        // - `Miss`：首开 / 被 LRU 挤掉（快照没了，本地什么都没有）；
        // - `Stale`：断线重连过 —— 断线期间的增量根本到不了客户端，快照会永久停在
        //   旧位置，不重拉就会一直显示下去（用户实测过「切回只看到部分工具步骤」）；
        // - 断开态（`Stale` 的一种）：此刻连增量都在路上不成立。
        //
        // 页边界不靠前端兜：服务端 `get_messages_page` 会把页首对齐到 user 消息
        // （整轮不会被切开，见 [`MESSAGE_BASE`] 的注释），前端只要按
        // `next_before_seq` 游标走。
        let use_projection = matches!(restored, RestoreOutcome::Fresh);
        // 诊断：切回时的决策输入（排查「切回贴底」）
        crate::app::diag_log(&format!(
            "[保位] open id={} outcome={:?} len={} {} scrolled_up={}",
            id,
            restored,
            self.messages.len(),
            pos_brief(&self.msg_list_state),
            self.msg_scrolled_up.get()
        ));
        // 诊断（第 2 探针）：接下来几帧在 `render` 里各打一次位置。
        //
        // 上面那行 `open` 是在 `restore_view_state()` 刚 `scroll_to()` 完、layout 还没
        // 跑的时刻读的，它等于 park 值本身。而实测证明 `cx.defer` 测不到布局之后
        // （嵌套三层的回调在同一次绘制前被一次性 flush，时间戳相差 < 1ms、并且
        // `viewport_bounds()` 全为 0），所以改挂到 `render` 开头：那里读到的是**上
        // 一帧 layout 跑完之后**的位置。
        self.restore_probe = 6;
        // 历史窗口是否为空：为空说明「有订阅但没有内容」——下面要**强制重订阅**，
        // 因为 resident 幂等会把正常重发挡掉，而这个会话可能从来没拿到过快照
        // （vite `useSessionHistory` 的 `needForce` 同口径）。
        // 投影分支必为空？不 —— `Fresh` 要求消息非空，所以这里的 `history_empty`
        // 只会出现在「要拉消息」的分支上。
        let history_empty = self.messages.is_empty();
        if use_projection {
            println!(
                "[消息] 切回会话使用本地投影（{} 条，零请求）",
                self.messages.len()
            );
        } else {
            // `anchor_seq` = 恢复出来的窗口**顶部**那条消息的 seq：重拉后按它找回
            // 阅读位置，否则每次切回都会被 `ListAlignment::Bottom` 钉到底部
            // （vite 靠 store 里保留的 Virtuoso 实例天然保位，native 得显式给锚点）。
            // 锚点必须带**消息内偏移**（`offset_in_item`），只给 seq 会落到那条消息
            // 的顶部（见 `load_messages_with` 的文档）。
            let anchor = self.msg_list_state.logical_scroll_top();
            let anchor_seq = self
                .messages
                .get(anchor.item_ix)
                .map(|m| m.seq)
                .filter(|s| *s > 0);
            crate::app::diag_log(&format!(
                "[保位] 拉取分支 anchor_ix={} len={} anchor_seq={:?} off={:.1}",
                anchor.item_ix,
                self.messages.len(),
                anchor_seq,
                f32::from(anchor.offset_in_item)
            ));
            self.load_messages_with(id, MESSAGE_BASE, anchor_seq, anchor.offset_in_item, cx);
        }
        self.load_session_usage(cx);
        self.load_todos_goals(cx);
        self.load_approvals(cx);
        // 订阅该会话（ADR-029 resident）：服务端收到订阅后推一帧快照，前端 replace
        // 窗口 —— 幂等（已订阅直接返回），断线重连时由 `on_event_stream_connected`
        // 整集重放。**这行是"打开/切回就有权威内容"的触发点**，缺了它快照永远不会来。
        //
        // ⚠️ 投影分支必须传 `false`：`force` 会让服务端**再推一帧快照** replace 窗口
        // —— 那会把已经分页加载的更早历史冲回最近一窗（内容凭空变少），滚动位置也
        // 会被重建到底部。已在 `subscribed_sessions` 里的会话幂等直接返回；不在集合里
        // （订阅曾失败）的会照常补发，那正是它需要的。
        self.subscribe_session(sid_for_subscribe, !use_projection && history_empty, cx);
        // 选中态与「当前会话始终可见」都体现在左栏行上
        self.restore_input(window, cx);
        // 追问编辑框按恢复出来的问题数补齐（切回一个挂起追问的会话时要能继续答）
        self.ensure_clarify_inputs(cx);
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
                            this.turn_running = false;
                            this.messages = Rc::new(Vec::new());
                            this.msg_list_state =
                                crate::app::new_msg_list_state(0, this.msg_scrolled_up.clone());
                            this.message_total = 0;
                            this.current_title = "选择左侧会话查看消息".to_string();
                            this.session_usage = None;
                            this.usage_shown = None;
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
            text,
        });
        cx.notify();
    }

    /// 确认回退：`POST /sessions/{id}/messages/delete`（删该消息及其后全部；
    /// ADR-040 事务内含工作区文件回退），成功后重拉消息链与会话统计。
    pub(crate) fn confirm_rollback(
        &mut self,
        session_id: String,
        message_id: String,
        text: String,
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
                        // 把被回退的那条用户输入**放回输入框**（用户实测指出
                        // 「回退后没有回到输入框中，导致要重新输入」）。
                        // 异步回调拿不到 `&mut Window`，所以写进待回填槽位，
                        // 由下一次 `render` 落进编辑器（那里一定有窗口）。
                        if !text.trim().is_empty() {
                            this.pending_composer_text = Some(text);
                        }
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
                // 运行中：呼吸绿点（vite `SessionList` 的 `session-running-<id>` 同位置）。
                // 与顶部 Tab 用**同一份** `running_sessions`：两处显示不一致比都不显示更糟。
                let mut head = div().flex().flex_row().items_center().gap_2();
                if self.running_sessions.contains(&id) {
                    head = head.child(pulse_dot(
                        format!("session-row-pulse-{id}"),
                        colors::SUCCESS(),
                        6.0,
                    ));
                }
                head = head.child(
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
            // 活跃标（对齐 vite `SessionTabBar`）：等待回答 > 运行中；任务角标在标题后。
            // 为什么放在这里而不是每帧从别处推导：`running_sessions` 是跨会话的权威集合
            // （`turn_state` 事件 + 发送时插入 + 快照帧记账），Tab 只是它的一个读取点。
            let running = self.running_sessions.contains(id);
            let awaiting = self.session_awaiting_answer(id);
            let tasks = self.running_tasks_of(id);
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
                    .child({
                        // 状态标排在标题**前**（vite 同序：等待回答 → 运行中 → 标题 → 任务角标），
                        // 所以先建空行再加标、最后补标题；标题必须最后 move 进去。
                        let mut head = div().flex().flex_row().items_center().gap_1().min_w_0();
                        if awaiting {
                            // 等待你的回答：琥珀问号（vite `HelpCircle` + amber）
                            head = head.child(
                                svg()
                                    .path(IconName::CircleQuestionMark.path())
                                    .size(px(11.))
                                    .flex_shrink_0()
                                    .text_color(rgb(colors::WARNING())),
                            );
                        } else if running {
                            // 运行中：呼吸绿点（vite `animate-pulse` + emerald）
                            head = head.child(pulse_dot(
                                format!("session-tab-pulse-{id}"),
                                colors::SUCCESS(),
                                6.0,
                            ));
                        }
                        head = head.child(title);
                        // 该会话名下还在跑的后台任务数（vite 红色角标）
                        if tasks > 0 {
                            head = head.child(
                                div()
                                    .flex_shrink_0()
                                    .min_w(px(14.))
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
                        head
                    })
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

/// 诊断用：把列表当前的位置压成一行读得懂的字。
///
/// 只打 `(item_ix, offset_in_item)` 是不够判断「在不在底部」的 —— 同一个偏移在
/// 长消息和短消息里含义完全不同。所以连带打两个基准量：
///
/// - `vh` = 视口高度（`available_height` 的来源）。贴底时 `off == ih - vh`，这是
///   唯一能一眼认出「这行就是贴底」的等式；不知道 `vh` 就只能靠猜偏移大小。
/// - `ih` = 锚点那条消息**实测**高度（`bounds_for_item` 拿的是渲染后的真实 bounds）。
///   它才是判断「切回时那条消息有没有变矮」的直接证据 —— `offset` 是相对这条消息
///   顶部算的，它一变，同一个 offset 指的物理位置就变了。
///
/// 该项尚未渲染时 `bounds_for_item` 返回 `None`，此时 `ih` 记 0（**不是**「高度为零」，
/// 而是「还没量」）——看日志时别误读。
pub(crate) fn pos_brief(state: &gpui_kit::ListState) -> String {
    let o = state.logical_scroll_top();
    let vh = f32::from(state.viewport_bounds().size.height);
    let ih = state
        .bounds_for_item(o.item_ix)
        .map_or(0.0, |b| f32::from(b.size.height));
    format!(
        "ix={} off={:.1} vh={:.1} ih={:.1} items={} follow={}",
        o.item_ix,
        f32::from(o.offset_in_item),
        vh,
        ih,
        state.item_count(),
        state.is_following_tail()
    )
}

#[cfg(test)]
mod projection_tests {
    //! 「切回会话直接用本地投影」判据的回归。
    //!
    //! 为什么单测纯函数而不是 UI 断言：判据只看四个标量（快照代际 / 当前代际 /
    //! 连接态 / 条数），四种组合各有明确期望；而端到端要造「切走 → 断线 → 重连 →
    //! 切回」的真实时序，成本高且不稳。判据钉住了，`open_session` 的分支就只剩
    //! `matches!(restored, Fresh)` 一行。
    // ⚠️ 不要写 `use super::*`（bin crate 根是 `main.rs`，glob 会把整棵模块树拉进
    // 测试模块，`#[test]` 宏展开撞 `recursion_limit`）。只显式导入需要的项。
    use crate::sessions::{projection_usable, RestoreOutcome};

    /// 常态：同一代、在线、有内容 → 零请求直接用投影。
    #[test]
    fn same_epoch_online_nonempty_is_usable() {
        assert!(projection_usable(3, 3, true, 42));
    }

    /// 断线重连过（代际变了）→ 断线期间的增量不可恢复，必须重拉。
    ///
    /// 这是本判据存在的**唯一理由**：只判「有快照」正是被实测推翻的旧口径
    /// （切回只看到部分工具步骤，而库里一条不少）。
    #[test]
    fn epoch_mismatch_is_not_usable() {
        assert!(
            !projection_usable(2, 3, true, 42),
            "跨代必须重拉：断线期间的增量到不了客户端"
        );
    }

    /// 此刻断开（还没重连上）→ 连「增量都在路上」都不成立，必须重拉。
    #[test]
    fn disconnected_is_not_usable() {
        assert!(
            !projection_usable(3, 3, false, 42),
            "断开态下不能把本地投影当权威内容"
        );
    }

    /// 空投影不可用：渲染出来就是「进会话什么都没有」，必须重拉。
    #[test]
    fn empty_projection_is_not_usable() {
        assert!(!projection_usable(3, 3, true, 0));
    }

    /// `restore_view_state` 的返回值三分：无快照 = Miss，有快照但投影不可用 = Stale。
    ///
    /// `Miss` 与 `Stale` 对调用方是同一件事（都要拉消息），但**原因不同** ——
    /// 分开是为了日志与 `/state` 能区分「首开」与「重连后补拉」。
    #[test]
    fn outcome_kinds_are_distinct() {
        assert_ne!(RestoreOutcome::Miss, RestoreOutcome::Stale);
        assert_ne!(RestoreOutcome::Miss, RestoreOutcome::Fresh);
        assert_ne!(RestoreOutcome::Stale, RestoreOutcome::Fresh);
    }
}
