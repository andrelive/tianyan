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
use gpui_kit::component::text::TextView;

/// 目录 `dir` 是否有**祖先**被折叠（父目录收起时子分组一并隐藏）。
///
/// 折叠键是**绝对目录路径**（`cases/failed_tasks`），所以判祖先只需逐级截断路径前缀。
fn has_collapsed_ancestor(collapsed: &std::collections::HashSet<String>, dir: &str) -> bool {
    let mut prefix = dir;
    while let Some((parent, _)) = prefix.rsplit_once('/') {
        if collapsed.contains(parent) {
            return true;
        }
        prefix = parent;
    }
    false
}

impl ClientApp {
    pub(crate) fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 目录树折叠：祖先折叠时子分组整段跳过（见下方 `has_collapsed_ancestor`）。
        // 折叠状态只影响渲染，不改变数据源——展开后条目原样回来。
        // 按**完整目录路径**建分组，并按路径字典序输出。
        //
        // 旧实现按 `relative_path` 的父段分组、保持后端返回顺序（= 按更新时间），
        // 于是 `entities` 与 `preferences` 并列、`cases/failed_tasks` 与
        // `events/decisions` 各占一段——用户看不出它们同属哪些上层目录（记忆页因此
        // 「完全平铺」）。字典序保证同一父目录的子目录**相邻**；同时把路径的每个
        // 前缀也建成（可能为空的）分组，父目录才不会因为「没有直属条目」而消失。
        let mut by_dir: std::collections::BTreeMap<String, Vec<&MemoryRow>> =
            std::collections::BTreeMap::new();
        for m in &self.memories {
            let key = m
                .relative_path
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_else(|| "(根)".to_string());
            let mut prefix = key.as_str();
            while let Some((parent, _)) = prefix.rsplit_once('/') {
                by_dir.entry(parent.to_string()).or_default();
                prefix = parent;
            }
            by_dir.entry(key).or_default().push(m);
        }
        let groups: Vec<(String, Vec<&MemoryRow>)> = by_dir.into_iter().collect();
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
            // 祖先折叠 → 整段跳过：父目录收起时，子分组自然跟着藏起来
            if has_collapsed_ancestor(&self.memory_collapsed, dir) {
                continue;
            }
            let collapsed = self.memory_collapsed.contains(dir);
            // 层级标题：按 `/` 深度缩进、只显示末段目录名。**没有直属条目的父目录**
            // （items 为空）用更弱的颜色 + 结尾 `/` 标出「这只是导航层」，与真正的
            // 条目容器区分开——否则 `cases` 与 `cases/failed_tasks` 看起来一样。
            // 标题可点击折叠/展开（▸ 收起 / ▾ 展开）。
            let depth = dir.matches('/').count();
            let leaf = dir.rsplit('/').next().unwrap_or(dir.as_str());
            let indent = 8.0 + depth as f32 * 12.0;
            let parent_only = items.is_empty();
            let label_color = if parent_only {
                colors::TEXT_TERTIARY()
            } else {
                colors::TEXT_SECONDARY()
            };
            let toggle_key = dir.clone();
            let mut section = div().flex().flex_col().gap_1().child(
                div()
                    .id(("mem-dir", gi))
                    .pl(px(indent))
                    .pr_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .hover(|s| s.bg(rgb(colors::BG_TERTIARY())))
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if collapsed { "▸" } else { "▾" }),
                    )
                    .child(div().text_size(px(10.)).text_color(rgb(label_color)).child(
                        if parent_only {
                            format!("{leaf}/")
                        } else {
                            leaf.to_string()
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("· {}", items.len())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // 点标题切换折叠；条目在下方子元素上，各自处理自己的点击
                        if !this.memory_collapsed.remove(&toggle_key) {
                            this.memory_collapsed.insert(toggle_key.clone());
                        }
                        cx.notify();
                    })),
            );
            if collapsed {
                list = list.child(section);
                continue;
            }
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
                        // L2 同样是 markdown（正文里常有 `##` 小节与列表），
                        // 与 L0/L1 同一种渲染口径
                        body = body.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                .child(TextView::markdown(("mem-detail", 0usize), t.clone())),
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

#[cfg(test)]
mod tests {
    use super::has_collapsed_ancestor;
    use std::collections::HashSet;

    #[test]
    fn collapsed_ancestor_hides_children() {
        let mut collapsed = HashSet::new();
        collapsed.insert("cases".to_string());
        assert!(
            has_collapsed_ancestor(&collapsed, "cases/failed_tasks"),
            "父目录折叠时子分组必须隐藏"
        );
        assert!(!has_collapsed_ancestor(&collapsed, "events/decisions"));
        assert!(
            !has_collapsed_ancestor(&collapsed, "cases"),
            "自身折叠不算「祖先折叠」（自身由调用方单独判断）"
        );
    }

    #[test]
    fn nested_ancestor_counts() {
        let mut collapsed = HashSet::new();
        collapsed.insert("memory/cases".to_string());
        assert!(has_collapsed_ancestor(
            &collapsed,
            "memory/cases/failed_tasks"
        ));
        assert!(!has_collapsed_ancestor(&collapsed, "memory/facts"));
    }
}
