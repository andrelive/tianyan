//! 会话侧边任务视图：后台任务面板、追问卡片、审批横幅、待办/目标窄条、子代理活动流。
//!
//! 这五块都是「主对话之外的异步状态」，共同点是要在空态时返回 0 高元素（不占位）——
//! 所以放一起，改占位逻辑时一处能看到全部。

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    /// 拉取本会话的后台任务（`GET /api/v1/tasks`，按 `parent_session_id` 归属过滤）。
    ///
    /// Tauri 版靠「3s 轮询 + `task_status` 事件唤醒」双保险；这里先做轮询
    /// （事件唤醒在 `handle_event` 里补，见 `task_status` 分支）。
    /// 失败**静默保留旧值**——面板是观测面，不能因为一次网络抖动把已有列表清空。
    pub(crate) fn load_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks");
            if let Ok(body) = fetch_json(url).await {
                let arr = body.as_array().cloned().unwrap_or_default();
                // 全量任务一次拉回来：面板只显示当前会话的，左栏会话角标要按
                // `parent_session_id` 聚合**所有**会话（同一份轮询供两处用，
                // 与 Tauri 版把 `backgroundTasks` 放进 store 同口径）。
                let all: Vec<TaskRow> = arr
                    .iter()
                    .map(|t| TaskRow {
                        id: t
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        kind: t
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        status: t
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        parent_session_id: t
                            .get("parent_session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        output_tail: t
                            .get("output_tail")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        error: t
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect();
                let rows: Vec<TaskRow> = all
                    .iter()
                    .filter(|t| {
                        session
                            .as_deref()
                            .map(|s| s == t.parent_session_id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.tasks = rows;
                    this.all_tasks = all;
                    this.rebuild_session_rows(cx);
                });
            }
        })
        .detach();
    }

    /// 会话后台任务停靠条（右侧 dock）。
    ///
    /// 对齐 Tauri 版 `AgentTasksPanel`：无任务时可折叠成竖条；活跃任务在上
    /// （顺序由后端返回决定）、终态保留展示。**取消按钮暂未接**（`POST /tasks/{id}/cancel`）。
    pub(crate) fn render_tasks_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tasks_collapsed {
            return div()
                .id("tasks-expand")
                .w(px(36.))
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .py_2()
                .border_l_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.tasks_collapsed = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("任务"),
                );
        }
        let mut panel = div()
            .w(px(300.))
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(format!("后台任务 {}", self.tasks.len())),
                    )
                    .child(
                        div()
                            .id("tasks-collapse")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("⟩")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.tasks_collapsed = true;
                                cx.notify();
                            })),
                    ),
            );
        if self.tasks.is_empty() {
            panel = panel.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("本会话暂无后台任务"),
            );
        }
        // 任务卡片（对齐 Tauri 版 `AgentTasksPanel`）：头 = 箭头 + 状态 + 类型徽章 +
        // 描述 + 状态文案 + 取消；展开体 = 终端输出尾部 / 子代理消息流。
        let mut cards: Vec<AnyElement> = Vec::new();
        for (ix, t) in self.tasks.iter().enumerate() {
            let open = self.expanded_task.as_deref() == Some(t.id.as_str());
            let terminal = matches!(
                t.status.as_str(),
                "completed" | "success" | "failed" | "cancelled" | "canceled"
            );
            let (glyph, color) = match t.status.as_str() {
                "running" | "pending" => ("●", colors::ACCENT()),
                "completed" | "success" => ("✓", colors::SUCCESS()),
                "failed" | "cancelled" | "canceled" => ("✗", colors::ERROR()),
                _ => ("○", colors::TEXT_TERTIARY()),
            };
            // 失败要带原因（截断），与 Tauri 版 `statusLabel` 同口径
            let status_label: String = match t.status.as_str() {
                "pending" => "等待中".to_string(),
                "running" => "运行中".to_string(),
                "completed" | "success" => "已完成".to_string(),
                "cancelled" | "canceled" => "已取消".to_string(),
                "failed" if !t.error.is_empty() => {
                    format!("失败（{}）", ellipsis(&t.error, 20))
                }
                "failed" => "失败".to_string(),
                other => other.to_string(),
            };
            let is_command = t.kind == "command";
            let kind_label = if is_command { "终端" } else { "委托" };
            let kind_color = if is_command {
                colors::TEXT_TERTIARY()
            } else {
                colors::ACCENT()
            };
            let chevron = if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let mut head = div()
                .id(("task-head", ix))
                .px_2()
                .py_1()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    svg()
                        .path(chevron.path())
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(div().text_xs().text_color(rgb(color)).child(glyph))
                .child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(kind_color))
                        .text_size(px(10.))
                        .text_color(rgb(kind_color))
                        .child(kind_label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(ellipsis(&t.description, 26)),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(if t.status == "failed" {
                            colors::ERROR()
                        } else {
                            colors::TEXT_TERTIARY()
                        }))
                        .child(status_label),
                );
            // 非终态才给「取消」；点它不能顺带把卡片展开（stop_propagation）
            if !terminal {
                let cancel_id = t.id.clone();
                head = head.child(
                    div()
                        .id(("task-cancel", ix))
                        .px_1()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .cursor_pointer()
                        .child("取消")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.cancel_task(cancel_id.clone(), cx);
                        })),
                );
            }
            let open_id = t.id.clone();
            let head = head.on_click(cx.listener(move |this, _, _, cx| {
                let id = open_id.clone();
                if this.expanded_task.as_deref() == Some(id.as_str()) {
                    this.expanded_task = None;
                } else {
                    this.expanded_task = Some(id.clone());
                    // 委托任务的子代理消息按需拉一次并缓存（task_id 即子会话 id）
                    if this.task_messages.get(&id).is_none() {
                        this.load_task_messages(id.clone(), cx);
                    }
                }
                cx.notify();
            }));
            let mut card = div()
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(rgb(if t.status == "failed" {
                    colors::ERROR()
                } else {
                    colors::BORDER()
                }))
                .bg(rgb(colors::BG_PRIMARY()))
                .overflow_hidden()
                .child(head);
            if open {
                let mut body = div()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1();
                // 失败任务：展开体顶部给完整原因（头部只放截断版）
                if t.status == "failed" && !t.error.is_empty() {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .child(t.error.clone()),
                    );
                }
                if is_command {
                    let text = if t.output_tail.is_empty() {
                        "（暂无输出）".to_string()
                    } else {
                        t.output_tail.clone()
                    };
                    body = body.child(
                        div()
                            .max_h(px(200.))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(0x0d1117))
                            .p_2()
                            .text_size(px(11.))
                            .text_color(rgb(0xc9d1d9))
                            .child(text),
                    );
                } else {
                    // 子代理活动流：紧凑单行投影（思考 / 输出 / 工具），
                    // 不是消息气泡 —— 与 Web 版 `DelegateActivity` 同构
                    body = body.child(self.render_delegate_activity(&t.id, !terminal, cx));
                }
                card = card.child(body);
            }
            cards.push(card.into_any_element());
        }
        panel
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .children(cards)
                    .overflow_y_scrollbar(),
            )
            .id("tasks-panel")
    }

    /// 取消一个后台任务（`POST /tasks/{id}/cancel`），随后刷新任务列表。
    ///
    /// 失败静默：任务可能已自行结束，下一次轮询会把真实状态带回来
    /// （与 Web 版「取消失败不弹错、等轮询」同口径）。
    pub(crate) fn cancel_task(&mut self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tasks/{task_id}/cancel");
            let _ = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| this.load_tasks(cx));
        })
        .detach();
    }

    /// 拉子代理任务的消息（展开委托任务时用）。
    ///
    /// 子代理是**带父会话引用的独立会话**（ADR-030：`task_id` 即 `session_id`），
    /// 所以直接读它的消息接口即可，不需要另开一条流。
    pub(crate) fn load_task_messages(&self, task_id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url =
                format!("http://{SERVER_HOST}:{port}/api/v1/sessions/{task_id}/messages?limit=20");
            if let Ok(body) = fetch_json(url).await {
                let arr = body
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                // 与主对话流同一套结构化解析：子代理的活动流要靠 segments 区分
                // 思考 / 输出 / 工具，纯文本摊平后这些结构就没了（此前正是如此）
                let rows: Vec<MsgRow> = arr
                    .iter()
                    .map(|m| {
                        let (segs, tool_calls) = parse_segments(m);
                        MsgRow {
                            id: m
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            role: m
                                .get("role")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            text: segments_text(m).into(),
                            segs,
                            tool_calls,
                            compression: false,
                            usage: parse_msg_usage(m),
                            images: parse_msg_images(m),
                            ts: parse_msg_ts(m),
                        }
                    })
                    .collect();
                let _ = this.update(cx, |this, cx| {
                    this.task_messages.insert(task_id, rows);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交追问回答（`POST /api/v1/chat/answer`）。
    ///
    /// `answers` 是 `ask_user` 工具的返回值（直接回给模型）：
    /// `{ answers: [{question, answer}], extra }`——与 Tauri 版 `ClarificationBubble`
    /// 的序列化口径一致，模型侧看到的形态不因客户端不同而变。
    pub(crate) fn submit_clarification(&mut self, cx: &mut Context<Self>) {
        let Some(questions) = self.clarification.clone() else {
            return;
        };
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let answers: Vec<serde_json::Value> = questions
            .iter()
            .enumerate()
            .map(|(i, (q, _))| {
                serde_json::json!({
                    "question": q,
                    "answer": self.clarify_answers.get(i).cloned().unwrap_or_default(),
                })
            })
            .collect();
        let port = self.port;
        self.status = "正在提交回答…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/chat/answer");
            let body = serde_json::json!({
                "session_id": sid,
                "answers": { "answers": answers, "extra": "" },
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.status = "回答已提交".to_string();
                        this.clarification = None;
                        this.clarify_answers.clear();
                    }
                    Err(e) => this.status = format!("提交回答失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 追问卡片（`ask_user` 挂起时显示在输入区上方）。
    ///
    /// 对齐 Tauri 版 `ClarificationBubble` 的**语义**（不是像素）：问题 + 选项，
    /// 全部答完才可提交。第一版只做「点选项作答」；无选项的问题给出提示让用户
    /// 用输入框里的原话补（避免为极少数场景先造一个多行富文本表单）。
    pub(crate) fn render_clarification(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(questions) = &self.clarification else {
            return div().into_any_element();
        };
        let all_answered = !questions.is_empty()
            && self
                .clarify_answers
                .iter()
                .take(questions.len())
                .all(|a| !a.trim().is_empty());
        let mut card = div().px_4().pb_2().child(
            div()
                .w_full()
                .max_w(px(896.))
                .mx_auto()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::ACCENT()))
                .bg(rgb(colors::BG_SECONDARY()))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(colors::ACCENT()))
                        .child("模型在向你确认（回答后它继续执行）"),
                ),
        );
        for (qi, (question, options)) in questions.iter().enumerate() {
            let answered = self
                .clarify_answers
                .get(qi)
                .map(|a| !a.trim().is_empty())
                .unwrap_or(false);
            let q_title = if answered {
                format!("✓ {}. {}", qi + 1, question)
            } else {
                format!("{}. {}", qi + 1, question)
            };
            let mut q_block = div().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .text_color(if answered {
                        rgb(colors::SUCCESS())
                    } else {
                        rgb(colors::TEXT_PRIMARY())
                    })
                    .child(q_title),
            );
            if options.is_empty() {
                q_block = q_block.child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("（该问题没有选项：把你输入框里写的原话作为回答）"),
                );
            }
            for (oi, option) in options.iter().enumerate() {
                let selected = self
                    .clarify_answers
                    .get(qi)
                    .map(|a| a == option)
                    .unwrap_or(false);
                let mut chip = div()
                    .id(("clarify", qi * 100 + oi))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .cursor_pointer()
                    .text_xs();
                if selected {
                    chip = chip
                        .bg(rgb(colors::ACCENT()))
                        .border_color(rgb(colors::ACCENT()))
                        .text_color(rgb(0xffffff));
                } else {
                    chip = chip
                        .bg(rgb(colors::BG_HOVER()))
                        .text_color(rgb(colors::TEXT_PRIMARY()));
                }
                let answer = option.clone();
                q_block = q_block.child(chip.child(option.clone()).on_click(cx.listener(
                    move |this, _, _w, cx| {
                        if let Some(slot) = this.clarify_answers.get_mut(qi) {
                            *slot = answer.clone();
                        }
                        cx.notify();
                    },
                )));
            }
            card = card.child(q_block);
        }
        let mut submit = div()
            .id("clarify-submit")
            .px_3()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(rgb(0xffffff));
        submit = if all_answered {
            submit.bg(rgb(colors::ACCENT())).cursor_pointer()
        } else {
            submit.bg(rgb(colors::BG_TERTIARY()))
        };
        card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(if all_answered {
                            "已全部作答".to_string()
                        } else {
                            format!("还有 {} 个问题未回答", questions.len())
                        }),
                )
                .child(div().flex_1())
                .child(
                    submit
                        .child("提交回答")
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            if this.clarify_answers.iter().all(|a| !a.trim().is_empty()) {
                                this.submit_clarification(cx);
                            }
                        })),
                ),
        )
        .into_any_element()
    }

    /// 拉取本会话的待审批操作（`GET /api/v1/approval/status`）。
    ///
    /// 只保留 `session_id` 匹配当前会话的项——审批是**会话内**的挂起操作，
    /// 别的会话的审批不该在本文出现（那会诱导用户给不相干的命令授权）。
    pub(crate) fn load_approvals(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let session = self.current_session.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/status");
            if let Ok(body) = fetch_json(url).await {
                let mut rows: Vec<ApprovalRow> = Vec::new();
                if let Some(arr) = body.get("pending_approvals").and_then(|v| v.as_array()) {
                    for a in arr {
                        let sid = a
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if session.as_deref() != Some(sid.as_str()) {
                            continue;
                        }
                        rows.push(ApprovalRow {
                            request_id: a
                                .get("request_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            description: a
                                .get("action_description")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            risk: a
                                .get("risk_level")
                                .and_then(|v| v.as_str())
                                .unwrap_or("-")
                                .to_string(),
                        });
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    this.pending_approvals = rows;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 提交审批决策（`POST /api/v1/approval/respond`，decision = approve / deny）。
    pub(crate) fn respond_approval(
        &mut self,
        request_id: String,
        decision: &'static str,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        self.status = format!("正在提交审批：{decision}");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/approval/respond");
            let body = serde_json::json!({ "request_id": request_id, "decision": decision });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("审批已提交（{decision}）"),
                    Err(e) => format!("提交审批失败：{e}"),
                };
                this.load_approvals(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 审批横幅（消息区顶部）：对齐 Tauri 版 `ApprovalBanner`（琥珀色告警 + 允许/拒绝）。
    ///
    /// 只显示**第一条**待审批：审批是一个个来的，堆一屏横幅既看不出关系也不知道点哪个；
    /// 处理完一条，下一条自然顶上（轮询刷新）。
    pub(crate) fn render_approval_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(a) = self.pending_approvals.first() else {
            return div().into_any_element();
        };
        let rid_approve = a.request_id.clone();
        let rid_deny = a.request_id.clone();
        div()
            .px_4()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::WARNING()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(colors::WARNING()))
                                    .child("⚠ 操作等待授权"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!("风险：{}", a.risk)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(ellipsis(&a.description, 300)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("approve")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::SUCCESS()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("允许")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_approve.clone(), "approve", cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("deny")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ERROR()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("拒绝")
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.respond_approval(rid_deny.clone(), "deny", cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 拉取会话待办与目标（`GET /todos` + `/goals`，均按 `session_id` 过滤）。
    ///
    /// 两者都是**会话绑定**的推理辅助（ADR-022：无全局计划页），所以跟着当前会话走、
    /// 切会话即重拉。失败静默（与任务面板同口径：观测面不因抖动清空）。
    pub(crate) fn load_todos_goals(&self, cx: &mut Context<Self>) {
        let Some(sid) = self.current_session.clone() else {
            return;
        };
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let todos_res = fetch_json(format!("{base}/todos?session_id={sid}")).await;
            let goals_res = fetch_json(format!("{base}/goals?session_id={sid}")).await;
            let mut todos: Vec<(String, String)> = Vec::new();
            if let Ok(body) = todos_res {
                if let Some(arr) = body.get("todos").and_then(|v| v.as_array()) {
                    for t in arr {
                        todos.push((
                            t.get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("pending")
                                .to_string(),
                            t.get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        ));
                    }
                }
            }
            let mut goals: Vec<GoalRow> = Vec::new();
            if let Ok(body) = goals_res {
                if let Some(arr) = body.get("goals").and_then(|v| v.as_array()) {
                    for g in arr {
                        // 元素是 `{ goal: {...}, progress, todo_total, todo_done }`
                        let goal = g.get("goal").unwrap_or(g);
                        goals.push(GoalRow {
                            title: goal
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: goal
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("active")
                                .to_string(),
                            progress: g.get("progress").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_total: g.get("todo_total").and_then(|v| v.as_u64()).unwrap_or(0),
                            todo_done: g.get("todo_done").and_then(|v| v.as_u64()).unwrap_or(0),
                        });
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.todos = todos;
                this.goals = goals;
                cx.notify();
            });
        })
        .detach();
    }

    /// 会话待办 / 目标窄条（在输入卡片**上方**，限宽 896px 居中）。
    ///
    /// 对齐 gui-vite `SessionTodoPanel`：
    /// - 两者都空时**整条不渲染**（不占位），有内容时可折叠；
    /// - **卡片 = header + 列表同一个容器**——折叠收起的是列表，不是把列表甩到
    ///   卡片外面（那样 header 一块、条目一块，观感上是两块分开的）；
    /// - 状态用 Lucide 图标（`CircleCheck` / `LoaderCircle` / `CircleDotDashed` /
    ///   `Circle`），不用 `[x]` / `[ ]` 文本——文本形态读起来像正文，且与 vite 两套观感。
    ///
    /// 已完成项只做置灰（vite 另有 line-through，GPUI 的 `Styled` 没有对应笔画修饰）。
    pub(crate) fn render_todo_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.todos.is_empty() && self.goals.is_empty() {
            return div().into_any_element();
        }
        let collapsed = self.todo_bar_collapsed;
        let in_progress = self
            .todos
            .iter()
            .filter(|(status, _)| status == "in_progress")
            .count();
        let done_count = self
            .todos
            .iter()
            .filter(|(status, _)| status == "completed")
            .count();
        // header 计数文案对齐 vite：`待办 N 项（进行中 N）· 完成 N`
        let mut count_text = format!("{} 项", self.todos.len());
        if in_progress > 0 {
            count_text.push_str(&format!("（进行中 {in_progress}）"));
        }
        if done_count > 0 {
            count_text.push_str(&format!(" · 完成 {done_count}"));
        }

        // header 整行可点（vite 那一侧就是通栏 `<button>`，不是只有小箭头能点）
        let mut card = div().flex().flex_col().gap_1().child(
            div()
                .id("todo-bar-toggle")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    svg()
                        .path(IconName::ListTodo.path())
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_SECONDARY())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("待办"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(count_text),
                )
                .child(div().flex_1())
                .child(
                    svg()
                        .path(if collapsed {
                            IconName::ChevronDown.path()
                        } else {
                            IconName::ChevronUp.path()
                        })
                        .size(px(12.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.todo_bar_collapsed = !this.todo_bar_collapsed;
                    cx.notify();
                })),
        );
        if !collapsed {
            // 列表落在**卡片内**（对应 vite 的 `{expanded && (...)}`）
            let mut list = div().flex().flex_col().gap_1().pt(px(2.));
            for g in &self.goals {
                let (icon, color) = match g.status.as_str() {
                    "completed" => (IconName::CircleCheck, colors::SUCCESS()),
                    "archived" => (IconName::Circle, colors::TEXT_TERTIARY()),
                    _ => (IconName::Target, colors::ACCENT()),
                };
                // 进度文案与 Tauri 版一致：有子待办才显示百分比，否则「进行中」
                //（`todo_total == 0` 时后端给的 progress 是 0，直接显示 0% 会误导）
                let progress_text = if g.todo_total > 0 {
                    format!("{}%（{}/{}）", g.progress, g.todo_done, g.todo_total)
                } else {
                    "进行中".to_string()
                };
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(color)))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(ellipsis(&g.title, 40)),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(progress_text),
                        ),
                );
            }
            for (status, title) in &self.todos {
                let (icon, color) = match status.as_str() {
                    "completed" => (IconName::CircleCheck, colors::SUCCESS()),
                    "in_progress" => (IconName::LoaderCircle, colors::ACCENT()),
                    "pending" => (IconName::CircleDotDashed, colors::TEXT_TERTIARY()),
                    _ => (IconName::Circle, colors::TEXT_TERTIARY()),
                };
                let text_color = match status.as_str() {
                    "completed" => colors::TEXT_TERTIARY(),
                    "in_progress" => colors::TEXT_PRIMARY(),
                    _ => colors::TEXT_SECONDARY(),
                };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(svg().path(icon.path()).size(px(12.)).text_color(rgb(color)))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(text_color))
                            .child(ellipsis(title, 48)),
                    );
                // vite 对未开始项右侧标「待开始」，这里同口径
                if status == "pending" {
                    row = row.child(
                        div()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("待开始"),
                    );
                }
                list = list.child(row);
            }
            card = card.child(list);
        }
        div()
            .px_4()
            .pb(px(6.))
            .child(
                div()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .px_3()
                    .py_2()
                    .child(card),
            )
            .into_any_element()
    }

    /// 子代理活动流（对齐 Web 版 `DelegateActivity`）。
    ///
    /// **紧凑单行列表**，不是消息气泡：图标 + 标签 + 摘要 + 字数/耗时，点开看原文。
    /// 相邻同类段合并 —— 流式逐 delta 会切出大量碎片段，不合并会一行一个 token。
    pub(crate) fn render_delegate_activity(
        &self,
        task_id: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(messages) = self.task_messages.get(task_id) else {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "加载中…"
                })
                .into_any_element();
        };
        let mut rows: Vec<ActivityRow> = Vec::new();
        for (mi, m) in messages.iter().enumerate() {
            for (si, seg) in m.segs.iter().enumerate() {
                let key = format!("{task_id}:{mi}:{si}");
                match seg {
                    Seg::Thinking(t) => {
                        if let Some(ActivityRow::Thinking { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Thinking {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Text(t) => {
                        if let Some(ActivityRow::Text { text, .. }) = rows.last_mut() {
                            text.push_str(t);
                        } else {
                            rows.push(ActivityRow::Text {
                                key,
                                text: t.clone(),
                            });
                        }
                    }
                    Seg::Tool { id, name, .. } => {
                        let call = m
                            .tool_calls
                            .iter()
                            .find(|c| &c.id == id)
                            .or_else(|| m.tool_calls.iter().find(|c| &c.name == name))
                            .cloned()
                            .unwrap_or_else(|| ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: String::new(),
                                presentation: "generic".to_string(),
                                result: None,
                                duration_ms: None,
                                error: None,
                            });
                        rows.push(ActivityRow::Tool { key, call });
                    }
                }
            }
        }
        if rows.is_empty() {
            return div()
                .px_1()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(if live {
                    "子智能体工作中…"
                } else {
                    "无过程记录"
                })
                .into_any_element();
        }
        let mut list = div().max_h(px(360.)).flex().flex_col().gap_1().w_full();
        for (ix, row) in rows.into_iter().enumerate() {
            list = list.child(self.render_activity_row(ix, row, cx));
        }
        list.overflow_y_scrollbar().into_any_element()
    }

    /// 活动流的一行：收起态一行（图标 + 标签 + 摘要 + meta + 箭头），展开态一块原文。
    pub(crate) fn render_activity_row(
        &self,
        ix: usize,
        row: ActivityRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open_key = self.expand.borrow().activity.clone();
        let (key, is_tool) = match &row {
            ActivityRow::Thinking { key, .. } | ActivityRow::Text { key, .. } => {
                (key.clone(), false)
            }
            ActivityRow::Tool { key, .. } => (key.clone(), true),
        };
        let open = open_key.as_deref() == Some(key.as_str());
        let chevron = if open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };

        // 头：图标 + 标签 + 摘要 + meta + 箭头
        let mut head = div()
            .id(("act", ix))
            .w_full()
            .px_1()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .text_size(px(11.));
        let mut details: Vec<AnyElement> = Vec::new();
        match row {
            ActivityRow::Tool { call, .. } => {
                let (icon, label) = presentation_of(&call.presentation);
                let failed = call.error.is_some();
                let settled = call.duration_ms.is_some() || call.result.is_some();
                let target = summarize_tool_target(&call.arguments);
                head = head
                    .child(if failed {
                        svg()
                            .path(IconName::CircleX.path())
                            .size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .into_any_element()
                    } else {
                        svg()
                            .path(icon.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(target),
                    );
                if let Some(ms) = call.duration_ms {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format_duration(ms)),
                    );
                } else if !settled {
                    head = head.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("…"),
                    );
                }
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format_arguments(&call.arguments))
                        .into_any_element(),
                );
                if let Some(err) = call.error {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::ERROR()))
                            .child(err)
                            .into_any_element(),
                    );
                }
                if let Some(result) = call.result.filter(|r| !r.is_empty()) {
                    details.push(
                        div()
                            .px_2()
                            .py_1()
                            .max_h(px(220.))
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(result)
                            .into_any_element(),
                    );
                }
            }
            ActivityRow::Thinking { text, .. } => {
                // 思考行摘要取**最后一行**（模型的当前念头），输出行取第一行
                let banner = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("思考"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text)
                        .into_any_element(),
                );
            }
            ActivityRow::Text { text, .. } => {
                let banner = text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                head = head
                    .child(
                        svg()
                            .path(IconName::TextAlignStart.path())
                            .size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("输出"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(banner),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 字", text.chars().count())),
                    );
                details.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        head = head.child(
            svg()
                .path(chevron.path())
                .size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY())),
        );
        let _ = is_tool;
        let head = head.on_click(cx.listener(move |this, _, _, cx| {
            let key = key.clone();
            let cur = this.expand.borrow().activity.clone();
            this.expand.borrow_mut().activity = if cur.as_deref() == Some(key.as_str()) {
                None
            } else {
                Some(key.clone())
            };
            cx.notify();
        }));

        let mut outer = div().flex().flex_col().w_full().child(head);
        if open {
            for d in details {
                outer = outer.child(d);
            }
        }
        outer.into_any_element()
    }
}
