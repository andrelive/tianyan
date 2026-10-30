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
                .child(presets);
            if self.sched_preset == 0 {
                form = form.child(field_wrap(&self.sched_custom_secs));
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
                                            .child(format_interval(t.interval_secs)),
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
        if name.is_empty() || prompt.is_empty() || secs < 60 {
            self.sched_status = "名称与指令不能为空，间隔至少 60 秒".to_string();
            cx.notify();
            return;
        }
        self.create_sched_task_direct(name, secs, workspace, prompt, cx);
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
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/scheduled-tasks");
            let body = serde_json::json!({
                "name": name,
                "interval_secs": secs,
                "workspace": workspace,
                "prompt": prompt,
            });
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
