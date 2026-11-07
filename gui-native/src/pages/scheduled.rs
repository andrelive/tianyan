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

/// 调度规格 → 表单的回填结果（编辑用；见 [`schedule_to_form`]）。
pub(crate) struct SchedFormFill {
    /// 重复方式（决定表单显示哪组输入）。
    pub(crate) mode: SchedMode,
    /// 时刻输入（`HH:MM` 或 `YYYY-MM-DDTHH:MM`；间隔制不用）。
    pub(crate) at: String,
    /// 每周模式选中的星期（ISO 1..7）。
    pub(crate) days: Vec<u8>,
    /// 间隔预设（`0` = 自定义，读 `custom_secs`）。
    pub(crate) preset: u64,
    /// 自定义间隔秒数（预设未命中时的回填值）。
    pub(crate) custom_secs: String,
}

/// 错过策略 → 表单的回填结果（编辑用；见 [`missed_to_form`]）。
pub(crate) struct SchedMissedFill {
    /// 错过策略三态。
    pub(crate) policy: SchedMissed,
    /// 补跑窗口小时数（仅 `Within` 用；`8` / `0.5` 这种文本）。
    pub(crate) grace_hours: String,
}

/// 编辑回填的**文本载荷**（要写进输入框的那几段；见 [`ClientApp::apply_sched_edit`]）。
pub(crate) struct SchedEditFill {
    /// 任务名称（写 `sched_name`）。
    pub(crate) name: String,
    /// 工作目录（写 `sched_workspace`）。
    pub(crate) workspace: String,
    /// 指令（写 `sched_prompt`）。
    pub(crate) prompt: String,
    /// 时刻（写 `sched_at`）。
    pub(crate) at: String,
    /// 自定义间隔秒数（写 `sched_custom_secs`）。
    pub(crate) custom_secs: String,
    /// 补跑窗口小时数（写 `sched_grace`）。
    pub(crate) grace_hours: String,
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
                        // 关表单 = 放弃本次编辑（否则下次点「新建」会带着旧任务 id 提交）
                        if !this.sched_form_open {
                            this.sched_editing = None;
                        }
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
                .gap_2();
            // 编辑态：表单已经按该任务的现值回填，标题点明「保存会改哪一条」
            if let Some(editing) = self.sched_editing.as_ref() {
                let label = self
                    .sched_tasks
                    .iter()
                    .find(|t| &t.id == editing)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| editing.clone());
                form = form.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(colors::BG_TERTIARY()))
                                .text_size(px(10.))
                                .text_color(rgb(colors::ACCENT()))
                                .child("编辑中"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(label),
                        ),
                );
            }
            form = form.child(field_wrap(&self.sched_name)).child(modes);
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
                .child(field_wrap_area(&self.sched_prompt));
            // 编辑态：任务启用状态（新建总是启用，不显示这一段）
            if self.sched_editing.is_some() {
                let mut en = div().flex().flex_row().items_center().gap_1();
                for on in [true, false] {
                    en = en.child(
                        seg_btn(
                            ("sched-enabled-form", if on { 1usize } else { 0usize }),
                            if on { "启用" } else { "停用" },
                            self.sched_enabled_form == on,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sched_enabled_form = on;
                            cx.notify();
                        })),
                    );
                }
                form = form.child(section_label("任务状态")).child(en);
            }
            form = form.child(
                div()
                    .id("sched-create")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(colors::ACCENT()))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .child(if self.sched_editing.is_some() {
                        "保存修改"
                    } else {
                        "创建"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.create_sched_task(cx))),
            );
            body = body.child(form);
        }

        // 列表。编辑态让位给表单：表单在列表上方，编辑靠下的任务时若不收起列表，
        // 用户点完「编辑」看不到任何变化（还得自己往上滚）
        if self.sched_editing.is_some() {
            body = body.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("保存或取消后返回任务列表"),
            );
        } else if self.sched_tasks.is_empty() {
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
                let id_toggle = t.id.clone();
                let enabled = t.enabled;
                // 编辑按钮要把整行带进回调（表单要按现值回填）
                let row = t.clone();
                // 标题行：名称 + 调度规格徽标（+ 停用标记）
                let mut title = div()
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
                    );
                if !enabled {
                    title = title.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("已停用"),
                    );
                }
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
                            .child(title)
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
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(card_btn(("sched-edit", ix), "编辑", false).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.begin_edit_sched_task(&row, window, cx)
                                }),
                            ))
                            .child(
                                card_btn(
                                    ("sched-enabled", ix),
                                    if enabled { "停用" } else { "启用" },
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_sched_enabled(id_toggle.clone(), !enabled, cx)
                                    },
                                )),
                            )
                            .child(card_btn(("sched-del", ix), "删除", true).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.delete_sched_task(id.clone(), cx)
                                }),
                            )),
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

    /// 进入某个任务的编辑态：设置**非输入框**状态（编辑 id / 重复方式 / 星期 /
    /// 错过策略 / 状态行），返回要写进输入框的那几段文本。
    ///
    /// 拆成「数据」与「写值」两步，是因为写输入框要 `&mut Window`：UI 点击那条路
    /// 手里有 window（[`Self::begin_edit_sched_task`]），而自动化端口没有——它只能
    /// 借窗口句柄现取（见 `automation.rs` 的 `sched_edit`）。两份实现只共享数据这一半。
    ///
    /// 回填必须覆盖**每一个**字段：漏一个，「保存」就会把该字段写成表单里的残留值。
    /// 调度规格按后端返回的**结构化**字段回填（`schedule_desc` 是给人看的字符串，
    /// 反解不回来）；旧服务器不返回结构化字段时退化为间隔制回填。
    pub(crate) fn apply_sched_edit(&mut self, row: &SchedTaskRow) -> SchedEditFill {
        self.sched_editing = Some(row.id.clone());
        self.sched_form_open = true;
        self.sched_enabled_form = row.enabled;

        let form = schedule_to_form(row);
        self.sched_mode = form.mode;
        self.sched_days = form.days;
        self.sched_preset = form.preset;
        let missed = missed_to_form(row);
        self.sched_missed = missed.policy;
        self.sched_status = format!("正在编辑「{}」——改完点保存修改", row.name);

        SchedEditFill {
            name: row.name.clone(),
            workspace: row.workspace.clone(),
            prompt: row.prompt.clone(),
            at: form.at,
            custom_secs: form.custom_secs,
            grace_hours: missed.grace_hours,
        }
    }

    /// 开始编辑某条任务（UI 点击路径）：回填数据 + 把文本写进表单输入框。
    pub(crate) fn begin_edit_sched_task(
        &mut self,
        row: &SchedTaskRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fill = self.apply_sched_edit(row);
        self.sched_name
            .update(cx, |s, cx| s.set_value(fill.name.as_str(), window, cx));
        self.sched_workspace
            .update(cx, |s, cx| s.set_value(fill.workspace.as_str(), window, cx));
        self.sched_prompt
            .update(cx, |s, cx| s.set_value(fill.prompt.as_str(), window, cx));
        self.sched_at
            .update(cx, |s, cx| s.set_value(fill.at.as_str(), window, cx));
        self.sched_custom_secs.update(cx, |s, cx| {
            s.set_value(fill.custom_secs.as_str(), window, cx)
        });
        self.sched_grace.update(cx, |s, cx| {
            s.set_value(fill.grace_hours.as_str(), window, cx)
        });
        cx.notify();
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
                                // 结构化生效值（编辑表单回填用；旧服务器不返回 → None）
                                schedule: t.get("schedule").cloned(),
                                missed: t.get("missed").cloned(),
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

    /// 表单提交入口：**新建与编辑共用同一条表单**，由 `sched_editing` 分派端点。
    ///
    /// 编辑态（列表里点了「编辑」）走 `PUT /scheduled-tasks/{id}`——后端是**部分
    /// 字段**语义，表单里出现的这些字段整体替换，没出现的保持原值；未设置编辑态时
    /// 走创建。
    pub(crate) fn create_sched_task(&mut self, cx: &mut Context<Self>) {
        let draft = match self.read_sched_draft(cx) {
            Ok(draft) => draft,
            Err(msg) => {
                self.sched_status = msg;
                cx.notify();
                return;
            }
        };
        match self.sched_editing.clone() {
            Some(id) => self.update_sched_task(id, draft, cx),
            None => self.create_sched_task_with(draft, cx),
        }
    }

    /// 从表单读出提交载荷（新建 / 编辑共用；校验失败返回给状态行的一句话）。
    ///
    /// 间隔取预设；预设为 `0`（自定义）时读秒数输入框，最小 60 秒
    /// （与后端 `schedule_task` 工具、`PUT /scheduled-tasks/{id}` 同口径 ——
    /// 更短的间隔没有意义，只会刷屏）。
    pub(crate) fn read_sched_draft(&self, cx: &mut Context<Self>) -> Result<SchedDraft, String> {
        let name = self.sched_name.read(cx).value().trim().to_string();
        let workspace = self.sched_workspace.read(cx).value().trim().to_string();
        let prompt = self.sched_prompt.read(cx).value().trim().to_string();
        if name.is_empty() || prompt.is_empty() {
            return Err("名称与指令不能为空".to_string());
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
                    return Err("间隔至少 60 秒".to_string());
                }
                (serde_json::json!({ "kind": "every", "secs": secs }), secs)
            }
            SchedMode::Daily => {
                if at.is_empty() {
                    return Err("请填时刻（如 15:30）".to_string());
                }
                (serde_json::json!({ "kind": "daily", "at": at }), 24 * 3600)
            }
            SchedMode::Weekly => {
                let mut days = self.sched_days.clone();
                days.sort_unstable();
                days.dedup();
                if at.is_empty() || days.is_empty() {
                    return Err("请选择星期并填时刻（如 09:00）".to_string());
                }
                let n = days.len() as u64;
                (
                    serde_json::json!({ "kind": "weekly", "weekdays": days, "at": at }),
                    (7 * 24 * 3600) / n,
                )
            }
            SchedMode::Once => {
                if at.is_empty() {
                    return Err("请填执行时间（如 2026-11-05T15:30）".to_string());
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
        Ok(SchedDraft {
            name,
            interval_secs,
            schedule,
            missed,
            workspace,
            prompt,
        })
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

    /// 保存编辑（表单 → `PUT /scheduled-tasks/{id}`）：只送表单覆盖到的字段。
    pub(crate) fn update_sched_task(
        &mut self,
        id: String,
        draft: SchedDraft,
        cx: &mut Context<Self>,
    ) {
        let SchedDraft {
            name,
            interval_secs,
            schedule,
            missed,
            workspace,
            prompt,
        } = draft;
        let mut body = serde_json::json!({
            "name": name,
            "interval_secs": interval_secs,
            "schedule": schedule,
            "workspace": workspace,
            "prompt": prompt,
            // 编辑态才有的「启用 / 停用」（表单上可改；新建一律启用）
            "enabled": self.sched_enabled_form,
        });
        if let Some(missed) = missed {
            body["missed"] = missed;
        }
        self.patch_sched_task(id, body, "定时任务已更新", true, cx);
    }

    /// 自动化 / 直接调用用的更新（与 [`Self::create_sched_task_direct`] 对称）：
    /// 只改传了的项（`None` = 保持原值），间隔走间隔制。
    pub(crate) fn update_sched_task_direct(
        &mut self,
        id: String,
        name: Option<String>,
        secs: Option<u64>,
        workspace: Option<String>,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut body = serde_json::Map::new();
        if let Some(name) = name {
            body.insert("name".to_string(), serde_json::json!(name));
        }
        if let Some(secs) = secs {
            // 间隔制：`schedule` 与 `interval_secs` 一起送（后者是旧服务器的唯一入口）
            body.insert(
                "schedule".to_string(),
                serde_json::json!({ "kind": "every", "secs": secs }),
            );
            body.insert("interval_secs".to_string(), serde_json::json!(secs));
        }
        if let Some(workspace) = workspace {
            body.insert("workspace".to_string(), serde_json::json!(workspace));
        }
        if let Some(prompt) = prompt {
            body.insert("prompt".to_string(), serde_json::json!(prompt));
        }
        self.patch_sched_task(
            id,
            serde_json::Value::Object(body),
            "定时任务已更新",
            false,
            cx,
        );
    }

    /// 只改启用状态（卡片上的「启用 / 停用」按钮；请求体只有一个 `enabled`）。
    pub(crate) fn set_sched_enabled(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        let ok_msg = if enabled {
            "定时任务已启用"
        } else {
            "定时任务已停用（定义保留，不再触发）"
        };
        self.patch_sched_task(
            id,
            serde_json::json!({ "enabled": enabled }),
            ok_msg,
            false,
            cx,
        );
    }

    /// 发**部分字段**更新请求的**唯一实现**（表单保存 / 启停按钮 / 自动化都走它）。
    ///
    /// `close_form`：保存表单时收起编辑态（启停按钮不该动表单）。
    pub(crate) fn patch_sched_task(
        &mut self,
        id: String,
        body: serde_json::Value,
        ok_msg: &'static str,
        close_form: bool,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks/{}",
                url_encode(&id)
            );
            let res = put_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.sched_status = ok_msg.to_string();
                        if close_form {
                            this.sched_editing = None;
                            this.sched_form_open = false;
                        }
                        this.load_sched_tasks(cx);
                    }
                    Err(e) => this.sched_status = format!("更新失败：{e}"),
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

/// 间隔制回填（预设命中就用预设，否则「自定义…」+ 秒数）。
fn interval_fill(secs: u64) -> SchedFormFill {
    let preset_hit = SCHED_PRESETS.iter().any(|(s, _)| *s == secs && *s != 0);
    SchedFormFill {
        mode: SchedMode::Interval,
        at: String::new(),
        days: vec![1, 2, 3, 4, 5],
        preset: if preset_hit { secs } else { 0 },
        custom_secs: if preset_hit {
            String::new()
        } else {
            secs.to_string()
        },
    }
}

/// 后端的结构化调度规格 → 表单状态（编辑回填）。
///
/// 旧服务器不返回 `schedule`：只能按 `interval_secs` 回填成间隔制——它区分不出
/// 「每天 15:30」与「每 24 小时」。
fn schedule_to_form(row: &SchedTaskRow) -> SchedFormFill {
    let Some(schedule) = row.schedule.as_ref() else {
        return interval_fill(row.interval_secs);
    };
    let at = schedule
        .get("at")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let weekdays = || {
        schedule
            .get("weekdays")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|d| d.as_u64())
                    .filter(|d| (1..=7).contains(d))
                    .map(|d| d as u8)
                    .collect::<Vec<u8>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![1, 2, 3, 4, 5])
    };
    match schedule.get("kind").and_then(|v| v.as_str()) {
        Some("every") => interval_fill(
            schedule
                .get("secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(row.interval_secs),
        ),
        Some("daily") => SchedFormFill {
            mode: SchedMode::Daily,
            at,
            days: vec![1, 2, 3, 4, 5],
            preset: 0,
            custom_secs: String::new(),
        },
        Some("weekly") => SchedFormFill {
            mode: SchedMode::Weekly,
            at,
            days: weekdays(),
            preset: 0,
            custom_secs: String::new(),
        },
        Some("once") => SchedFormFill {
            mode: SchedMode::Once,
            at,
            days: vec![1, 2, 3, 4, 5],
            preset: 0,
            custom_secs: String::new(),
        },
        // 未知形态（未来新增的 kind）：退化成间隔制，别让表单空着
        _ => interval_fill(row.interval_secs),
    }
}

/// 后端的结构化错过策略 → 表单状态（编辑回填）。
///
/// 旧服务器不返回 `missed`（生效值是「立即补跑」）→ **如实**回填 RunOnce，
/// 不悄悄改成「错过跳过」：改任务的人应该看到它现在的真实行为。
fn missed_to_form(row: &SchedTaskRow) -> SchedMissedFill {
    match row
        .missed
        .as_ref()
        .and_then(|m| m.get("kind"))
        .and_then(|k| k.as_str())
    {
        Some("skip") => SchedMissedFill {
            policy: SchedMissed::Skip,
            grace_hours: String::new(),
        },
        Some("within") => {
            let secs = row
                .missed
                .as_ref()
                .and_then(|m| m.get("grace_secs"))
                .and_then(|v| v.as_u64())
                .unwrap_or(8 * 3600);
            SchedMissedFill {
                policy: SchedMissed::Within,
                grace_hours: hours_text(secs),
            }
        }
        _ => SchedMissedFill {
            policy: SchedMissed::RunOnce,
            grace_hours: String::new(),
        },
    }
}

/// 秒 → 小时文本（整数不带小数点：`8` 而不是 `8.0`；半小时 → `0.5`）。
fn hours_text(secs: u64) -> String {
    let hours = secs as f64 / 3600.0;
    if hours.fract().abs() < f64::EPSILON {
        format!("{}", hours as u64)
    } else {
        format!("{hours:.2}")
    }
}

/// 卡片行内的动作按钮（编辑 / 启停 / 删除共用；无边框、次要色）。
fn card_btn(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    danger: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(11.))
        .text_color(rgb(if danger {
            colors::TEXT_TERTIARY()
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
