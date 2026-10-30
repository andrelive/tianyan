//! 洞察页：技能/工具/文档/搜索四个追踪面 + 模型用量分组。
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
    /// 洞察页：总览四卡 + 模型用量（含分组）+ 调度器任务表。
    pub(crate) fn render_insights(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.insight_stats.clone().unwrap_or_default();
        let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();

        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .flex_wrap()
                .child(stat_card(
                    "技能",
                    format!("{}", s.total_skills_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_skill_calls)),
                ))
                .child(stat_card(
                    "工具",
                    format!("{}", s.total_tools_tracked),
                    format!("累计调用 {} 次", group_thousands(s.total_tool_calls)),
                ))
                .child(stat_card(
                    "文档",
                    format!("{}", s.total_docs_tracked),
                    "VFS 访问过的文档".to_string(),
                ))
                .child(stat_card(
                    "搜索",
                    format!("{}", s.total_searches),
                    "检索次数".to_string(),
                )),
        );

        if let Some(t) = &self.usage_total {
            let mut usage = div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                .child(format!("近 {} 天模型用量", self.usage_days)),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(format!(
                                    "{} 次调用 · 总 token {} · 缓存命中 {:.1}%",
                                    group_thousands(t.calls),
                                    group_thousands(t.total_tokens),
                                    t.cache_hit_rate * 100.0
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .flex_wrap()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("未命中输入 {}", group_thousands(t.uncached_input)))
                        .child(format!("缓存命中输入 {}", group_thousands(t.cached_input)))
                        .child(format!("输出 {}", group_thousands(t.completion_tokens))),
                );
            if !self.usage_grouped.is_empty() {
                let mut rows = div().flex().flex_col().gap_1().w_full();
                for g in self.usage_grouped.iter().take(10) {
                    rows = rows.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(g.group.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "{} 次 · {} tok · 命中 {:.0}%",
                                        group_thousands(g.calls),
                                        group_thousands(g.total_tokens),
                                        g.cache_hit_rate * 100.0
                                    )),
                            ),
                    );
                }
                usage = usage.child(rows);
            }
            body = body.child(usage);
        }

        let mut sched = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
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
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("调度器"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(if self.scheduler_running {
                                colors::SUCCESS()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(if self.scheduler_running {
                                "运行中"
                            } else {
                                "未装配"
                            }),
                    ),
            );
        if let Some(id) = &self.scheduler_executing {
            sched = sched.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::ACCENT()))
                    .child(format!("正在执行 {id}")),
            );
        }
        if self.sched_status_tasks.is_empty() {
            sched = sched.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("没有已注册的定时任务"),
            );
        }
        for t in &self.sched_status_tasks {
            sched = sched.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(t.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!(
                                "{} · 跑 {} 次 · 上次 {}前",
                                format_interval(t.interval_secs),
                                t.run_count,
                                format_secs_short(t.last_run_ago_secs)
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("下次 {}后", format_secs_short(t.next_due_in_secs))),
                    ),
            );
        }
        body = body.child(sched);

        // 天数切换 + 刷新
        let mut days_bar = div().flex().flex_row().items_center().gap_1();
        for d in [1i64, 3, 7, 30] {
            let is_cur = self.usage_days == d;
            days_bar = days_bar.child(
                div()
                    .id(("ins-days", d as usize))
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
                    .text_size(px(10.))
                    .text_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(format!("{d} 天"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.usage_days = d;
                        this.load_insights(cx);
                    })),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
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
                                            .path(IconName::ChartLine.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("洞察"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.insights_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(days_bar)
                            .child(
                                div()
                                    .id("ins-reload")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .cursor_pointer()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("刷新")
                                    .on_click(cx.listener(|this, _, _, cx| this.load_insights(cx))),
                            ),
                    ),
            )
            .child(body.overflow_y_scrollbar())
    }

    /// 加载洞察数据（`/stats` + `/usage/stats?days=N` + `/scheduler/status`）。
    ///
    /// 三个请求都失败也只是少一块，不阻塞整页（每个面独立降级）。
    pub(crate) fn load_insights(&self, cx: &mut Context<Self>) {
        let port = self.port;
        let days = self.usage_days;
        cx.spawn(async move |this, cx| {
            let stats_res = fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/stats")).await;
            let usage_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days={days}"
            ))
            .await;
            let sched_res = fetch_json(format!(
                "http://{SERVER_HOST}:{port}/api/v1/scheduler/status"
            ))
            .await;

            let stats = stats_res.ok().map(|b| InsightStats {
                total_skills_tracked: b
                    .get("total_skills_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_skill_calls: b
                    .get("total_skill_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tools_tracked: b
                    .get("total_tools_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_tool_calls: b
                    .get("total_tool_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_docs_tracked: b
                    .get("total_docs_tracked")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_searches: b
                    .get("total_searches")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            });

            let (usage_total, usage_grouped) = match usage_res {
                Ok(b) => (
                    UsageTotal::from_json(b.get("total")),
                    UsageGroup::from_json_array(b.get("grouped")),
                ),
                Err(_) => (None, Vec::new()),
            };

            let (running, executing, sched_tasks) = match sched_res {
                Ok(b) => {
                    let running = b.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
                    let executing = b
                        .get("executing_task_id")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    let tasks: Vec<SchedStatusRow> = b
                        .get("tasks")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(SchedStatusRow {
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
                                        run_count: t
                                            .get("run_count")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        last_run_ago_secs: t
                                            .get("last_run_ago_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        next_due_in_secs: t
                                            .get("next_due_in_secs")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (running, executing, tasks)
                }
                Err(_) => (false, None, Vec::new()),
            };

            let n = sched_tasks.len();
            let _ = this.update(cx, |this, cx| {
                this.insight_stats = stats;
                this.usage_total = usage_total;
                this.usage_grouped = usage_grouped;
                this.scheduler_running = running;
                this.scheduler_executing = executing;
                this.sched_status_tasks = sched_tasks;
                this.insights_status = format!(
                    "调度器{} · {n} 个任务",
                    if running { "运行中" } else { "未装配" }
                );
                cx.notify();
            });
        })
        .detach();
    }
}
