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

/// 把一条记忆归到 `(命名空间, 目录)`。
///
/// 命名空间取自 URI 第一段（`relative_path` 不含它），目录取自 `relative_path`
/// 去掉最后一段（`cases/failed_tasks/foo` → `cases/failed_tasks`；根级条目归「(根)」）。
fn memory_category(m: &MemoryRow) -> (String, String) {
    let ns = m
        .uri
        .trim_start_matches("tianyan://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    let dir = m
        .relative_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_else(|| "(根)".to_string());
    (ns, dir)
}

/// 左栏分组顺序：记忆 → 画像 → 规则 → 其它（未知命名空间排最后，不隐藏）。
fn memory_ns_rank(ns: &str) -> u8 {
    match ns {
        "memory" => 0,
        "user" => 1,
        "agent" => 2,
        _ => 3,
    }
}

/// 命名空间显示名。
fn memory_ns_label(ns: &str) -> &str {
    match ns {
        "memory" => "记忆",
        "user" => "画像",
        "agent" => "规则",
        other => other,
    }
}

/// 目录显示名。**未知目录回落原路径**——宁可显示 `made/up` 也不要隐藏条目。
///
/// 返回 `String` 而不是 `&str`：未知目录要回借用入参（来自 `cats` 局部变量），
/// 而调用点把它塞进渲染树（活到函数结束之后），借不出去。
fn memory_dir_label(dir: &str) -> String {
    match dir {
        "facts" => "事实".to_string(),
        "events/decisions" => "决策".to_string(),
        "events/evolution_reports" => "演化报告".to_string(),
        "events/task_states" => "任务状态".to_string(),
        "cases/failed_tasks" => "案例 / 失败".to_string(),
        "cases/successful_tasks" => "案例 / 成功".to_string(),
        "preferences" => "偏好".to_string(),
        "entities" => "实体".to_string(),
        "patterns" => "模式".to_string(),
        other => other.to_string(),
    }
}

impl ClientApp {
    pub(crate) fn render_memory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // ── 方案 B：分类独占左栏 ──────────────────────────────────────
        //
        // 键 = `命名空间/目录`（如 `memory/cases/failed_tasks`）。此前把分类混在
        // 条目列表里、靠缩进表达层级，用户实测「分类看不清」——这里把分类彻底抽出来
        // 做成独立导航栏，条目列表只显示当前类目。
        let mut by_cat: std::collections::BTreeMap<String, (String, String, usize, f32)> =
            std::collections::BTreeMap::new();
        for m in &self.memories {
            let (ns, dir) = memory_category(m);
            let e = by_cat
                .entry(format!("{ns}/{dir}"))
                .or_insert((ns, dir, 0, 0.0));
            e.2 += 1;
            e.3 += m.importance;
        }
        let mut cats: Vec<(String, String, String, usize, f32)> = by_cat
            .into_iter()
            .map(|(key, (ns, dir, n, sum))| (key, ns, dir, n, sum / n as f32))
            .collect();
        // 命名空间固定顺序（记忆 → 画像 → 规则），同命名空间内按目录名字典序
        cats.sort_by(|a, b| {
            memory_ns_rank(&a.1)
                .cmp(&memory_ns_rank(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        // 当前类目：记住的选择若仍在（刷新后目录可能消失）就沿用，否则取第一个
        let current = self
            .memory_selected_cat
            .clone()
            .filter(|k| cats.iter().any(|(key, ..)| key == k))
            .or_else(|| cats.first().map(|(k, ..)| k.clone()));

        let selected = self
            .memories
            .iter()
            .find(|m| self.memory_selected.as_deref() == Some(m.uri.as_str()))
            .cloned();

        // 左栏：分类导航（命名空间分组 + 每类条数 + 平均重要性小条）
        //
        // **左中之间不画分隔线**，分区完全靠色差：未选中行 = `BG_TERTIARY`（与栏底同色，
        // 所以整片是连续的一条深色带）、选中行 = `BG_SECONDARY`（与中栏同色，
        // 于是它看起来像中栏向左延伸出来的一条）、中栏 = `BG_SECONDARY`。
        //
        // 为什么不给选中行做圆角（试过 6 版都退回）：圆角削掉的缺口露出的是**父容器**
        // （栏底），而栏底与未选中行同色 ⇒ 直角与圆角在视觉上没有任何差别；
        // 要让弧显现必须把栏底再压深一档，左栏就变成「栏底 / 未选中 / 选中」三档深浅，
        // 层次反而更脏。色差 + 直角是确定的，不依赖"缺口露出什么颜色"。
        let mut nav = div()
            .w(px(190.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(colors::BG_TERTIARY()))
            .py_2()
            .overflow_y_scrollbar();
        let mut last_ns = String::new();
        for (cat_ix, (key, ns, dir, n, avg)) in cats.iter().enumerate() {
            if *ns != last_ns {
                nav = nav.child(
                    div()
                        .px_3()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("{} · {}", memory_ns_label(ns), ns)),
                );
                last_ns = ns.clone();
            }
            let is_cur = current.as_deref() == Some(key.as_str());
            let label_color = if is_cur {
                colors::ACCENT()
            } else {
                colors::TEXT_SECONDARY()
            };
            let imp_color = if *avg >= 0.8 {
                colors::SUCCESS()
            } else if *avg >= 0.6 {
                colors::TEXT_SECONDARY()
            } else {
                colors::TEXT_TERTIARY()
            };
            let bar_w = avg.clamp(0.0, 1.0) * 26.0;
            let key_c = key.clone();
            let mut row = div()
                .id(("mem-cat", cat_ix))
                // 左侧留 6px：色带不顶到栏目左边，留出呼吸空间。
                // 右侧**不设**外边距——色带必须顶到中栏，选中行才能与中栏连成一条
                // （右侧一旦留边距，两者之间就夹一条深色缝，"连成一整条"立刻断掉）。
                .ml(px(6.))
                .px_2()
                .py_1()
                .cursor_pointer()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                // 未选中 = 深（与栏底同色）；选中 = 浅（与中栏同色）
                .bg(rgb(if is_cur {
                    colors::BG_SECONDARY()
                } else {
                    colors::BG_TERTIARY()
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .text_color(rgb(label_color))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(memory_dir_label(dir)),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("{n}")),
                )
                .child(
                    div()
                        .w(px(26.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(rgb(colors::BORDER()))
                        .child(
                            div()
                                .w(px(bar_w))
                                .h(px(3.))
                                .rounded_full()
                                .bg(rgb(imp_color)),
                        ),
                );
            if is_cur {
                // 选中行：只圆左侧两角（`rounded_l` = 左上 + 左下）——缺口露出的是
                // 深色栏底，所以圆角看得见；右侧保持直角，与中栏同色、无缝隙地相接。
                row = row.rounded_l(px(6.));
            }
            nav = nav.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.memory_selected_cat = Some(key_c.clone());
                cx.notify();
            })));
        }

        // 中栏：当前类目的条目（后端已按更新时间倒排，这里只过滤）
        let mut mid = div()
            .w(px(330.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .py_2()
            .overflow_y_scrollbar();
        if self.memories.is_empty() {
            mid = mid.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Brain.path())
                            .size(px(28.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .px_3()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("暂无记忆——自演化综述任务会在会话结束后写入"),
                    ),
            );
        }
        if let Some(cur) = &current {
            let items: Vec<&MemoryRow> = self
                .memories
                .iter()
                .filter(|m| {
                    let (ns, dir) = memory_category(m);
                    format!("{ns}/{dir}") == *cur
                })
                .collect();
            for (ix, m) in items.iter().enumerate() {
                let is_sel = self.memory_selected.as_deref() == Some(m.uri.as_str());
                let uri = m.uri.clone();
                let imp_color = if m.importance >= 0.8 {
                    colors::SUCCESS()
                } else if m.importance >= 0.6 {
                    colors::TEXT_SECONDARY()
                } else {
                    colors::TEXT_TERTIARY()
                };
                mid = mid.child(
                    div()
                        .id(("mem", ix))
                        .mx_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .bg(rgb(if is_sel {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_SECONDARY()
                        }))
                        .child(
                            // 重要性色条：不占文字宽度（原实现是右侧一个恒为 0.50 的数字）
                            div().w(px(3.)).h(px(26.)).rounded_full().bg(rgb(imp_color)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(12.5))
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
                                        .text_size(px(10.5))
                                        .text_color(rgb(colors::TEXT_TERTIARY()))
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(
                                            m.abstract_text
                                                .as_ref()
                                                .map(|t| ellipsis(t, 40))
                                                .unwrap_or_else(|| "（无摘要）".to_string()),
                                        ),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_memory(uri.clone(), cx);
                        })),
                );
            }
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
                    .child(nav)
                    .child(mid)
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
    use super::{memory_category, memory_dir_label, memory_ns_rank};
    use crate::types::MemoryRow;

    fn row(uri: &str, rel: &str) -> MemoryRow {
        MemoryRow {
            uri: uri.to_string(),
            name: "x".to_string(),
            relative_path: rel.to_string(),
            importance: 0.5,
            abstract_text: None,
            overview_text: None,
            detail_text: None,
            updated_at: String::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn category_splits_ns_and_dir() {
        let (ns, dir) = memory_category(&row(
            "tianyan://memory/cases/failed_tasks/foo",
            "cases/failed_tasks/foo",
        ));
        assert_eq!(ns, "memory");
        assert_eq!(dir, "cases/failed_tasks");

        let (ns2, dir2) =
            memory_category(&row("tianyan://user/preferences/bar", "preferences/bar"));
        assert_eq!(ns2, "user");
        assert_eq!(dir2, "preferences");
    }

    #[test]
    fn ns_rank_orders_memory_then_user_then_agent() {
        assert!(memory_ns_rank("memory") < memory_ns_rank("user"));
        assert!(memory_ns_rank("user") < memory_ns_rank("agent"));
        assert!(memory_ns_rank("agent") < memory_ns_rank("unknown"));
    }

    #[test]
    fn known_dirs_have_labels_unknown_falls_back() {
        assert_eq!(memory_dir_label("cases/failed_tasks"), "案例 / 失败");
        assert_eq!(memory_dir_label("preferences"), "偏好");
        assert_eq!(memory_dir_label("made/up"), "made/up");
    }
}
