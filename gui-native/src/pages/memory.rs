//! 记忆页：L0/L1 浏览 + L2 展开。
//!
//! 运维子域的条目会被过滤（`is_operational_path`），不在用户视图里出现。

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
    pub(crate) fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 按目录分组（relative_path 的父段；根级归到「(根)」）
        let mut groups: Vec<(String, Vec<&MemoryRow>)> = Vec::new();
        for m in &self.memories {
            let key = m
                .relative_path
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_else(|| "(根)".to_string());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(m),
                None => groups.push((key, vec![m])),
            }
        }
        let selected = self
            .memories
            .iter()
            .find(|m| self.memory_selected.as_deref() == Some(m.uri.as_str()))
            .cloned();

        // 左：分组列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_2();
        if self.memories.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("暂无记忆——自演化综述任务会在会话结束后写入"),
                    ),
            );
        }
        for (gi, (dir, items)) in groups.iter().enumerate() {
            let mut section = div().flex().flex_col().gap_1().child(
                div()
                    .px_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(dir.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("· {}", items.len())),
                    ),
            );
            for (ix, m) in items.iter().enumerate() {
                let is_sel = self.memory_selected.as_deref() == Some(m.uri.as_str());
                let uri = m.uri.clone();
                // 重要度三档配色：≥0.7 绿（稳固）/ ≥0.4 次级 / 其余红（待巩固）
                let imp_color = if m.importance >= 0.7 {
                    colors::SUCCESS()
                } else if m.importance >= 0.4 {
                    colors::TEXT_SECONDARY()
                } else {
                    colors::ERROR()
                };
                section = section.child(
                    div()
                        .id(("mem", gi * 1000 + ix))
                        .w_full()
                        .px_2()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(if is_sel {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .cursor_pointer()
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
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(rgb(if is_sel {
                                            colors::ACCENT()
                                        } else {
                                            colors::TEXT_PRIMARY()
                                        }))
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(m.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(imp_color))
                                        .child(format!("{:.2}", m.importance)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(
                                    m.abstract_text
                                        .as_ref()
                                        .map(|t| ellipsis(t, 56))
                                        .unwrap_or_else(|| "（无摘要）".to_string()),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_memory(uri.clone(), cx);
                        })),
                );
            }
            list = list.child(section);
        }

        // 右：详情（L0 → L1 → L2，L2 折叠）
        let detail = match &selected {
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Brain.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一条记忆查看三层内容"),
                )
                .into_any_element(),
            Some(m) => {
                let mut head = div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(m.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(m.relative_path.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("重要度 {:.2}", m.importance))
                            .child(if m.updated_at.is_empty() {
                                String::new()
                            } else {
                                format!("更新于 {}", short_stamp(&m.updated_at))
                            })
                            .child(if m.tags.is_empty() {
                                String::new()
                            } else {
                                format!("#{}", m.tags.join(" #"))
                            }),
                    );
                let mut body = div().flex_1().min_h_0().p_4().flex().flex_col().gap_3();
                if let Some(t) = &m.abstract_text {
                    body = body.child(memory_block("L0 摘要", t.clone()));
                }
                if let Some(t) = &m.overview_text {
                    body = body.child(memory_block("L1 概览", t.clone()));
                }
                if let Some(t) = &m.detail_text {
                    let open = self.memory_detail_open;
                    let label = if open {
                        "▾ L2 全文（收起）".to_string()
                    } else {
                        format!("▸ L2 全文（{} 字，点击展开）", t.chars().count())
                    };
                    body = body.child(
                        div()
                            .id("mem-detail-toggle")
                            .cursor_pointer()
                            .text_size(px(10.))
                            .text_color(rgb(colors::ACCENT()))
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.memory_detail_open = !this.memory_detail_open;
                                cx.notify();
                            })),
                    );
                    if open {
                        body = body.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(t.clone()),
                        );
                    }
                }
                head = head.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(m.uri.clone()),
                );
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(head)
                    .child(body.overflow_y_scrollbar())
                    .into_any_element()
            }
        };

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
                                            .path(IconName::Brain.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("记忆"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(self.memories_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("mem-reload")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| this.load_memories(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .child(list.overflow_y_scrollbar()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(detail)),
            )
    }

    /// 加载记忆条目（`GET /memory`）。
    ///
    /// 后端已聚合三类来源（`memory/` 事实与决策、`user/` 画像、`agent/patterns` 模式）
    /// 并且只返回叶子：目录只是导航中间节点，内容为空、importance 是默认值，
    /// 直接展示只会误导（这是后端注释里的原话，界面不再二次筛选）。
    ///
    /// 选中一条记忆（**列表点击与自动化共用这一份**，ADR-052 D8）：详情不需要
    /// 额外请求 —— `GET /memory` 一次就带回 L0/L1/L2 三层（`detail` 几百到上千字），
    /// 这里只切选中并把 L2 折叠复位。
    pub(crate) fn select_memory(&mut self, uri: String, cx: &mut Context<Self>) {
        self.memory_selected = Some(uri);
        self.memory_detail_open = false;
        cx.notify();
    }

    pub(crate) fn load_memories(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/memory");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<MemoryRow> = body
                        .get("memories")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|m| {
                            let meta = m.get("metadata");
                            let text_of = |key: &str| {
                                m.get(key)
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .filter(|s| !s.trim().is_empty())
                            };
                            Some(MemoryRow {
                                uri: m.get("uri")?.as_str()?.to_string(),
                                name: m
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                relative_path: m
                                    .get("relative_path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                importance: meta
                                    .and_then(|v| v.get("importance"))
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.5)
                                    as f32,
                                updated_at: meta
                                    .and_then(|v| v.get("updated_at"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                tags: meta
                                    .and_then(|v| v.get("tags"))
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                abstract_text: text_of("abstract"),
                                overview_text: text_of("overview"),
                                detail_text: text_of("detail"),
                            })
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.memories = rows;
                        this.memories_status = format!("{n} 条记忆（L0/L1/L2 三层）");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.memories_status = format!("加载记忆失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
