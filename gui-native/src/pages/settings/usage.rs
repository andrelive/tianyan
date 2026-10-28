//! 设置页「用量统计」栏目：LLM token 消耗（未命中输入 / 缓存命中 / 输出 / 命中率）。
//!
//! 与 Web 版 `UsageTab` 的差别（都是「更好」方向的差别）：
//! - **分组维度可切**（按模型 / 按提供商）——vite 只在请求里固定按 model；
//! - 每个分组多两列（未命中输入 / 命中输入 / 输出）+ 一行 token 占比条，
//!   一眼看出「谁在烧 token」；
//! - 区间文案与「共 N 次调用」始终在数据上方（vite 只在有数据时才显示）。

use crate::format::*;
use crate::theme::colors;
use crate::types::*;
use crate::widgets::stat_card;
use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）
use gpui_kit::component::scroll::ScrollableElement as _;

/// 快捷时段（与后端 `days` 参数同口径：0 = 全部时间）。
const USAGE_RANGES: [(&str, i64); 5] = [
    ("1 天", 1),
    ("3 天", 3),
    ("7 天", 7),
    ("30 天", 30),
    ("全部", 0),
];

impl ClientApp {
    /// 拉用量统计（`GET /api/v1/usage/stats`）。
    ///
    /// 独立于洞察页的 `load_insights`：那个会顺带拉调度器/审批/统计四份数据，
    /// 而设置页只需要用量——少三个往返，切栏目更快。
    pub(crate) fn load_settings_usage(&mut self, cx: &mut Context<Self>) {
        let port = self.port;
        let days = self.usage_days;
        let group_by = self.usage_group_by;
        self.usage_msg = "正在加载用量…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/usage/stats?days={days}&group_by={group_by}"
            );
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(body) => {
                        this.usage_total = UsageTotal::from_json(body.get("total"));
                        this.usage_grouped = UsageGroup::from_json_array(body.get("grouped"));
                        this.usage_msg = String::new();
                    }
                    Err(e) => {
                        this.usage_msg = format!("加载用量失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 渲染「用量统计」栏目。
    pub(crate) fn render_settings_usage(&self, cx: &mut Context<Self>) -> AnyElement {
        let range_label = if self.usage_days > 0 {
            format!("最近 {} 天", self.usage_days)
        } else {
            "全部时间".to_string()
        };
        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p_4()
            .overflow_y_scrollbar()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(
                        "覆盖聊天、子代理委托、演化任务等所有 LLM 调用；命中率 = 缓存命中输入 / 总输入。",
                    ),
            );

        // ── 查询区：快捷时段 + 分组维度 ──
        let mut controls = div().flex().flex_row().items_center().gap_2().flex_wrap();
        for (label, days) in USAGE_RANGES {
            let active = self.usage_days == days;
            controls = controls.child(
                div()
                    .id(("usage-range", days as usize))
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
                        colors::BG_PRIMARY()
                    }))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(if active {
                        colors::TEXT_PRIMARY()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        if this.usage_days == days {
                            return;
                        }
                        this.usage_days = days;
                        this.load_settings_usage(cx);
                    })),
            );
        }
        controls = controls.child(div().w(px(12.)));
        for (label, key) in [("按模型", "model"), ("按提供商", "provider")] {
            let active = self.usage_group_by == key;
            controls = controls.child(
                div()
                    .id(("usage-group", key.len()))
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
                        colors::BG_PRIMARY()
                    }))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(if active {
                        colors::TEXT_PRIMARY()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        if this.usage_group_by == key {
                            return;
                        }
                        this.usage_group_by = key;
                        this.load_settings_usage(cx);
                    })),
            );
        }
        col = col.child(controls);

        // ── 状态 ──
        if !self.usage_msg.is_empty() {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(self.usage_msg.clone()),
            );
        }

        // ── 汇总 ──
        match &self.usage_total {
            Some(t) => {
                col = col.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!(
                            "{range_label} · {} 次 LLM 调用 · 总 {} token",
                            group_thousands(t.calls),
                            group_thousands(t.total_tokens)
                        )),
                );
                let mut cards = div().flex().flex_row().gap_2().flex_wrap();
                cards = cards
                    .child(stat_card(
                        "未命中输入",
                        group_thousands(t.uncached_input),
                        "全价计费".to_string(),
                    ))
                    .child(stat_card(
                        "缓存命中输入",
                        group_thousands(t.cached_input),
                        "折扣计费".to_string(),
                    ))
                    .child(stat_card(
                        "输出",
                        group_thousands(t.completion_tokens),
                        "生成 token".to_string(),
                    ))
                    .child(stat_card(
                        "缓存命中率",
                        format!("{:.1}%", t.cache_hit_rate * 100.0),
                        "命中 / 总输入".to_string(),
                    ));
                col = col.child(cards);

                // ── 分组表 ──
                let group_title = if self.usage_group_by == "provider" {
                    "提供商"
                } else {
                    "模型"
                };
                if !self.usage_grouped.is_empty() {
                    let sum: u64 = self.usage_grouped.iter().map(|g| g.total_tokens).sum();
                    let mut table = div()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            // 表头
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .pb_1()
                                .border_b_1()
                                .border_color(rgb(colors::BORDER()))
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child(div().flex_1().min_w_0().child(group_title))
                                .child(div().w(px(52.)).text_right().child("调用"))
                                .child(div().w(px(78.)).text_right().child("未命中"))
                                .child(div().w(px(78.)).text_right().child("命中"))
                                .child(div().w(px(70.)).text_right().child("输出"))
                                .child(div().w(px(52.)).text_right().child("命中率"))
                                .child(div().w(px(120.)).child("占比")),
                        );
                    for g in self.usage_grouped.iter() {
                        let pct = if sum > 0 {
                            g.total_tokens as f64 / sum as f64
                        } else {
                            0.0
                        };
                        table = table.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .py_1()
                                .border_b_1()
                                .border_color(rgb(colors::BORDER()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_PRIMARY()))
                                        .child(g.group.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(52.))
                                        .text_right()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_SECONDARY()))
                                        .child(group_thousands(g.calls)),
                                )
                                .child(
                                    div()
                                        .w(px(78.))
                                        .text_right()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_SECONDARY()))
                                        .child(group_thousands(g.uncached_input)),
                                )
                                .child(
                                    div()
                                        .w(px(78.))
                                        .text_right()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_SECONDARY()))
                                        .child(group_thousands(g.cached_input)),
                                )
                                .child(
                                    div()
                                        .w(px(70.))
                                        .text_right()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_SECONDARY()))
                                        .child(group_thousands(g.completion_tokens)),
                                )
                                .child(
                                    div()
                                        .w(px(52.))
                                        .text_right()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_SECONDARY()))
                                        .child(format!("{:.0}%", g.cache_hit_rate * 100.0)),
                                )
                                // 占比条：宽度按 total_tokens 占全部的比例（120px 轨道）
                                .child(
                                    div()
                                        .w(px(120.))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w(px(88.))
                                                .h(px(6.))
                                                .rounded_full()
                                                .bg(rgb(colors::BG_TERTIARY()))
                                                .flex()
                                                .flex_row()
                                                .child(
                                                    div()
                                                        .w(px((88.0 * pct) as f32))
                                                        .h(px(6.))
                                                        .rounded_full()
                                                        .bg(rgb(colors::ACCENT())),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(9.))
                                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                                .child(format!("{:.0}%", pct * 100.0)),
                                        ),
                                ),
                        );
                    }
                    col = col.child(table);
                } else {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("所选时段内没有分组数据（可能只有零散调用）。"),
                    );
                }
            }
            None => {
                if self.usage_msg.is_empty() {
                    col = col.child(
                        div()
                            .py_4()
                            .text_sm()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{range_label}内暂无 LLM 调用记录。")),
                    );
                }
            }
        }
        col.into_any_element()
    }
}
