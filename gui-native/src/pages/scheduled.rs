//! 定时任务页：间隔制任务（ADR-024）的增删。
//!

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::theme::colors;
use crate::types::*;
use crate::widgets::*;
use gpui_kit::assets::IconName;

/// 新建定时任务的提交载荷（表单与自动化共用，避免 8 个裸参数）。
pub(crate) struct SchedDraft {
    /// 任务名称。
    pub name: String,
    /// 等价间隔（落盘双写 / 旧服务器兜底）。
    pub interval_secs: u64,
    /// 调度规格（`{"kind":...}`，与后端 `Schedule` 同形）。
    pub schedule: serde_json::Value,
    /// 错过策略（`None` = 不传，服务端默认「立即补跑」）。
    pub missed: Option<serde_json::Value>,
    /// 工作目录。
    pub workspace: String,
    /// 给智能体的指令。
    pub prompt: String,
}

impl ClientApp {
    /// 定时任务页：列表卡片 + 新建表单（间隔预设 / 自定义秒 / 工作区 / 指令）。
    ///
    /// 与 Web 版 `ScheduledTasksSection` 同口径：间隔预设按钮组而非下拉
    /// （GPUI 没有原生 select；按钮行反而更好点），自定义走秒数输入。
    pub(crate) fn render_scheduled(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let head = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path(IconName::CalendarClock.path())
                                    .size(px(16.))
                                    .text_color(rgb(colors::TEXT_SECONDARY())),
                            )
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("定时任务"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                "按执行间隔调用智能体在指定工作区工作；\
                                 服务停机期间超期的任务重启后补跑一次",
                            ),
                    ),
            )
            .child(
                div()
                    .id("sched-toggle")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(if self.sched_form_open {
                        "取消"
                    } else {
                        "＋ 新建定时任务"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sched_form_open = !this.sched_form_open;
                        cx.notify();
                    })),
            );

        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        // 状态行（创建/删除结果；空串不占位）
        if !self.sched_status.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.sched_status.clone()),
            );
        }

        // 新建表单
        if self.sched_form_open {
            // 重复方式（间隔 / 每天 / 每周 / 一次性）——决定下面的输入区
            let mut modes = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for m in SchedMode::ALL {
                let is_cur = self.sched_mode == m;
                modes = modes.child(
                    seg_btn(("sched-mode", m as usize), m.label(), is_cur).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.sched_mode = m;
                            cx.notify();
                        },
                    )),
                );
            }
            let mut presets = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for (secs, label) in SCHED_PRESETS {
                let is_cur = self.sched_preset == secs;
                presets = presets.child(
                    div()
                        .id(("sched-preset", secs))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(if is_cur {
                            colors::ACCENT()
                        } else {
                            colors::BORDER()
                        }))
                        .bg(rgb(if is_cur {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(if is_cur {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_SECONDARY()
                        }))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sched_preset = secs;
                            cx.notify();
                        })),
                );
            }
            let mut form = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                .gap_2()
                .child(field_wrap(&self.sched_name))
                .child(modes);
            // 输入区按模式分派
            if self.sched_mode == SchedMode::Interval {
                form = form.child(presets);
                if self.sched_preset == 0 {
                    form = form.child(field_wrap(&self.sched_custom_secs));
                }
            } else {
                if self.sched_mode == SchedMode::Weekly {
                    let mut days = div().flex().flex_row().items_center().gap_1().flex_wrap();
                    for (d, label) in WEEKDAY_LABELS {
                        let on = self.sched_days.contains(&d);
                        days = days.child(seg_btn(("sched-day", d as usize), label, on).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(pos) = this.sched_days.iter().position(|x| *x == d) {
                                    this.sched_days.remove(pos);
                                } else {
                                    this.sched_days.push(d);
                                }
                                cx.notify();
                            }),
                        ));
                    }
                    form = form.child(section_label("每周哪几天")).child(days);
                }
                // 时刻输入的语义随模式变化（每天 15:30 / 一次性 2026-11-05T15:30）
                form = form
                    .child(section_label(self.sched_mode.at_placeholder()))
                    .child(field_wrap(&self.sched_at));
                // 错过策略（定点任务的关键开关：立即补跑会在重启时于任意时刻执行）
                let mut mseg = div().flex().flex_row().items_center().gap_1().flex_wrap();
                for m in SchedMissed::ALL {
                    let on = self.sched_missed == m;
                    mseg = mseg.child(
                        seg_btn(("sched-missed", m as usize), m.label(), on).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.sched_missed = m;
                                cx.notify();
                            },
                        )),
                    );
                }
                form = form
                    .child(section_label("错过时（服务没在跑）"))
                    .child(mseg);
                if self.sched_missed == SchedMissed::Within {
                    form = form.child(field_wrap(&self.sched_grace));
                }
                form = form.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(
                            "「立即补跑」会在服务重启时于任意时刻补执行——收盘后这类任务请用\
                             「错过跳过」或「窗口内补」，否则会在盘中跑。",
                        ),
                );
            }
            form = form
                .child(field_wrap(&self.sched_workspace))
                .child(field_wrap_area(&self.sched_prompt))
                .child(
                    div()
                        .id("sched-create")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(colors::ACCENT()))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child("创建")
                        .on_click(cx.listener(|this, _, _, cx| this.create_sched_task(cx))),
                );
            body = body.child(form);
        }

        // 列表
        if self.sched_tasks.is_empty() {
            body = body.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::CalendarClock.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                "暂无定时任务——可让智能体用 schedule_task 工具创建，或在此手动创建",
                            ),
                    ),
            );
        } else {
            let mut list = div().flex().flex_col().gap_2().w_full();
            for (ix, t) in self.sched_tasks.iter().enumerate() {
                let id = t.id.clone();
                let mut card = div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(t.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(colors::BORDER()))
                                            .bg(rgb(colors::BG_TERTIARY()))
                                            .text_size(px(10.))
                                            .text_color(rgb(colors::ACCENT()))
                                            // 调度规格 + 错过策略（后端算好的可读描述；
                                            // 旧服务器不返回 → 回退成间隔文案）
                                            .child(format!(
                                                "{}{}",
                                                t.schedule_desc,
                                                if t.missed_desc.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" · {}", t.missed_desc)
                                                }
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "工作区: {}",
                                        if t.workspace.is_empty() {
                                            "(默认)"
                                        } else {
                                            &t.workspace
                                        }
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(ellipsis(&t.prompt, 140)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "下次(约): {}",
                                        t.next_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(relative_from_now)
                                            .unwrap_or_else(|| "待定".to_string())
                                    ))
                                    .child(
                                        t.last_run_at
                                            .as_deref()
                                            .and_then(parse_rfc3339_secs)
                                            .map(|ts| format!("上次: {}", relative_from_now(ts)))
                                            .unwrap_or_default(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id(("sched-del", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("删除")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_sched_task(id.clone(), cx)
                            })),
                    );
                if let Some(res) = t.last_result.as_ref().filter(|r| !r.is_empty()) {
                    card = card.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("结果: {}", ellipsis(res, 120))),
                    );
                }
                list = list.child(card);
            }
            body = body.child(list);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body.overflow_y_scrollbar())
    }

    /// 加载定时任务列表（`GET /scheduled-tasks`）。
    pub(crate) fn load_sched_tasks(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<SchedTaskRow> = body
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|t| {
                            Some(SchedTaskRow {
                                id: t.get("id")?.as_str()?.to_string(),
                                name: t
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                interval_secs: t
                                    .get("interval_secs")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0),
                                // 后端新字段（旧服务器不返回 → 回退成间隔描述）
                                schedule_desc: t
                                    .get("schedule_desc")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| {
                                        let secs = t
                                            .get("interval_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        format!("每 {secs} 秒")
                                    }),
                                missed_desc: t
                                    .get("missed_desc")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                workspace: t
                                    .get("workspace")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                prompt: t
                                    .get("prompt")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                enabled: t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                                // RFC3339 字符串（磁盘上的旧 epoch 数字由后端兼容读入）
                                last_run_at: t
                                    .get("last_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                next_run_at: t
                                    .get("next_run_at")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                last_result: t
                                    .get("last_result")
                                    .and_then(|v| v.as_str())
                                    .map(|v| v.to_string()),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.sched_tasks = rows;
                        this.sched_status = format!("{n} 个定时任务");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.sched_status = format!("加载定时任务失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 创建定时任务（`POST /scheduled-tasks`）。
    ///
    /// 间隔取预设；预设为 `0`（自定义）时读秒数输入框，最小 60 秒
    /// （与 Web 版 `min={60}` 同口径 —— 更短的间隔没有意义，只会刷屏）。
    pub(crate) fn create_sched_task(&mut self, cx: &mut Context<Self>) {
        let name = self.sched_name.read(cx).value().trim().to_string();
        let workspace = self.sched_workspace.read(cx).value().trim().to_string();
        let prompt = self.sched_prompt.read(cx).value().trim().to_string();
        if name.is_empty() || prompt.is_empty() {
            self.sched_status = "名称与指令不能为空".to_string();
            cx.notify();
            return;
        }
        let at = self.sched_at.read(cx).value().trim().to_string();
        // 组装调度规格 + 等价间隔（旧服务器与落盘双写兜底都用得上）
        let (schedule, interval_secs) = match self.sched_mode {
            SchedMode::Interval => {
                let secs = if self.sched_preset == 0 {
                    self.sched_custom_secs
                        .read(cx)
                        .value()
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0)
                } else {
                    self.sched_preset
                };
                if secs < 60 {
                    self.sched_status = "间隔至少 60 秒".to_string();
                    cx.notify();
                    return;
                }
                (serde_json::json!({ "kind": "every", "secs": secs }), secs)
            }
            SchedMode::Daily => {
                if at.is_empty() {
                    self.sched_status = "请填时刻（如 15:30）".to_string();
                    cx.notify();
                    return;
                }
                (serde_json::json!({ "kind": "daily", "at": at }), 24 * 3600)
            }
            SchedMode::Weekly => {
                let mut days = self.sched_days.clone();
                days.sort_unstable();
                days.dedup();
                if at.is_empty() || days.is_empty() {
                    self.sched_status = "请选择星期并填时刻（如 09:00）".to_string();
                    cx.notify();
                    return;
                }
                let n = days.len() as u64;
                (
                    serde_json::json!({ "kind": "weekly", "weekdays": days, "at": at }),
                    (7 * 24 * 3600) / n,
                )
            }
            SchedMode::Once => {
                if at.is_empty() {
                    self.sched_status = "请填执行时间（如 2026-11-05T15:30）".to_string();
                    cx.notify();
                    return;
                }
                (serde_json::json!({ "kind": "once", "at": at }), 24 * 3600)
            }
        };
        // 错过策略：间隔制忽略它，不传（避免语义误导）
        let missed = (self.sched_mode != SchedMode::Interval).then(|| match self.sched_missed {
            SchedMissed::Skip => serde_json::json!({ "kind": "skip" }),
            SchedMissed::RunOnce => serde_json::json!({ "kind": "run_once" }),
            SchedMissed::Within => {
                let hours = self
                    .sched_grace
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(8.0)
                    .max(0.1);
                serde_json::json!({ "kind": "within", "grace_secs": (hours * 3600.0) as u64 })
            }
        });
        self.create_sched_task_with(
            SchedDraft {
                name,
                interval_secs,
                schedule,
                missed,
                workspace,
                prompt,
            },
            cx,
        );
    }

    /// 真正发创建请求（UI 表单与自动化 `sched_create` **共用这一份**）。
    ///
    /// 分开的理由同 ADR-052 D8：自动化若另写一份请求构造，两条路径就分叉了。
    /// 校验也放在调用方之前各做一次，这里只信任入参。
    pub(crate) fn create_sched_task_direct(
        &mut self,
        name: String,
        secs: u64,
        workspace: String,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        // 自动化/旧路径：纯间隔制（显式传 schedule，行为与旧版逐字一致）
        self.create_sched_task_with(
            SchedDraft {
                name,
                interval_secs: secs,
                schedule: serde_json::json!({ "kind": "every", "secs": secs }),
                missed: None,
                workspace,
                prompt,
            },
            cx,
        );
    }

    /// 发创建请求的**唯一实现**（表单与自动化都走它）。
    ///
    /// `schedule` / `missed` 用 JSON 传递（`{"kind":...}`），与后端 `Schedule` /
    /// `MissedPolicy` 的 serde 形状一致——前端不复制一份枚举语义。
    pub(crate) fn create_sched_task_with(&mut self, draft: SchedDraft, cx: &mut Context<Self>) {
        let SchedDraft {
            name,
            interval_secs,
            schedule,
            missed,
            workspace,
            prompt,
        } = draft;
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            let mut body = serde_json::json!({
                "name": name,
                "interval_secs": interval_secs,
                "schedule": schedule,
                "workspace": workspace,
                "prompt": prompt,
            });
            if let Some(missed) = missed {
                body["missed"] = missed;
            }
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.sched_status = "定时任务已创建".to_string();
                        this.sched_form_open = false;
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("创建失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除定时任务（`DELETE /scheduled-tasks/{id}`）。
    pub(crate) fn delete_sched_task(&mut self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks/{}",
                url_encode(&id)
            );
            let res = delete_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.sched_status = "定时任务已删除".to_string();
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("删除失败：{e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// 分段按钮（重复方式 / 星期 / 错过策略共用；与设置页的分段控件同形）。
fn seg_btn(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    active: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(if active {
            colors::ACCENT()
        } else {
            colors::BORDER()
        }))
        .bg(rgb(if active {
            colors::BG_TERTIARY()
        } else {
            colors::BG_SECONDARY()
        }))
        .cursor_pointer()
        .text_size(px(11.))
        .text_color(rgb(if active {
            colors::ACCENT()
        } else {
            colors::TEXT_SECONDARY()
        }))
        .child(label)
}

/// 表单小节标签（小字灰）。
fn section_label(text: &'static str) -> gpui_kit::Div {
    div()
        .pt_1()
        .text_size(px(10.))
        .text_color(rgb(colors::TEXT_TERTIARY()))
        .child(text)
}
