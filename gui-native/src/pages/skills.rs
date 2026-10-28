//! 技能页：已学习方法论技能的列表 + 使用统计 + L2 详情。
//!

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use gpui_kit::component::text::TextView;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    /// 技能页：顶部采纳统计 + 左列表（搜索）+ 右详情（markdown 正文）。
    ///
    /// 与 Web 版 `SkillsPanel` 同口径：技能是方法论文档，只读；不做「运行」按钮
    /// （执行语义归工具面板与对话里的工具卡）。
    pub(crate) fn render_skills(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.skills_query.trim().to_lowercase();
        let visible: Vec<SkillRow> = self
            .skills
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .skills
            .iter()
            .find(|s| self.skills_selected.as_deref() == Some(s.id.as_str()))
            .cloned();

        // 顶部：累计调用 + 采纳排行（方法论技能无「执行成败」语义，只看频率）
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .flex_wrap();
        if self.skills_total_calls > 0 {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_SECONDARY()))
                    .child(format!(
                        "技能累计调用 {} 次",
                        group_thousands(self.skills_total_calls)
                    )),
            );
            let mut chips = div().flex().flex_row().items_center().gap_1().flex_wrap();
            for st in self.skill_stats.iter().take(8) {
                chips = chips.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{} {} 次", st.skill_id, st.total_calls)),
                );
            }
            stats_bar = stats_bar.child(chips);
        } else {
            stats_bar = stats_bar.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无技能调用统计——智能体调用技能后，这里会展示采纳排行与平均耗时"),
            );
        }

        // 左：技能列表
        let mut list = div().flex_1().min_h_0().p_2().flex().flex_col().gap_1();
        if visible.is_empty() {
            list = list.child(
                div()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path(IconName::Lightbulb.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.skills.is_empty() {
                                "暂无学习方法论技能"
                            } else {
                                "未找到匹配技能"
                            }),
                    ),
            );
        }
        for (ix, skill) in visible.iter().enumerate() {
            let is_sel = self.skills_selected.as_deref() == Some(skill.id.as_str());
            let id = skill.id.clone();
            let stat = self.skill_stats.iter().find(|s| s.skill_id == skill.id);
            let mut row = div()
                .id(("skill", ix))
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
                        .text_size(px(13.))
                        .text_color(rgb(if is_sel {
                            colors::ACCENT()
                        } else {
                            colors::TEXT_PRIMARY()
                        }))
                        .child(skill.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(ellipsis(&skill.description, 52)),
                );
            if let Some(st) = stat.filter(|st| st.total_calls > 0) {
                let mut line = format!(
                    "调用 {} 次 · 均值 {}ms",
                    st.total_calls,
                    st.avg_time_ms.round() as u64
                );
                if st.total_calls > st.success_calls {
                    line.push_str(&format!(
                        " · 执行异常 {} 次",
                        st.total_calls - st.success_calls
                    ));
                }
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(line),
                );
            }
            if let Some(t) = &skill.updated_at {
                row = row.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!("更新于 {}", short_stamp(t))),
                );
            }
            list = list.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.skills_selected = Some(id.clone());
                this.skill_content = None;
                this.skills_status = "正在加载技能内容…".to_string();
                this.load_skill_detail(id.clone(), cx);
                cx.notify();
            })));
        }

        // 右：详情
        let detail = match &selected {
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    svg()
                        .path(IconName::Lightbulb.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个技能查看详情"),
                )
                .into_any_element(),
            Some(skill) => {
                let content = self.skill_content.clone().unwrap_or_default();
                let mut chips = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .bg(rgb(colors::BG_SECONDARY()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("v{}", skill.version)),
                    );
                if let Some(t) = &skill.updated_at {
                    chips = chips.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("更新于 {}", short_stamp(t))),
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
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child(skill.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(skill.description.clone()),
                            )
                            .child(chips),
                    )
                    .child(div().flex_1().min_h_0().p_4().child(if content.is_empty() {
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(
                                if self.skills_status.starts_with("加载技能详情失败")
                                    || self.skills_status.starts_with("正在加载")
                                {
                                    self.skills_status.clone()
                                } else {
                                    "该技能没有可查看的内容".to_string()
                                },
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .child(TextView::markdown(
                                ("skill-md", 0usize),
                                SharedString::from(content),
                            ))
                            .overflow_y_scrollbar()
                            .into_any_element()
                    }))
                    .into_any_element()
            }
        };

        div().size_full().flex().flex_col().child(stats_bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(
                    div()
                        .w(px(colors::LIST_WIDTH))
                        .h_full()
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .border_b_1()
                                .border_color(rgb(colors::BORDER()))
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
                                                .path(IconName::Lightbulb.path())
                                                .size(px(16.))
                                                .text_color(rgb(colors::TEXT_SECONDARY())),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(15.))
                                                .text_color(rgb(colors::TEXT_PRIMARY()))
                                                .child("技能"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(colors::TEXT_TERTIARY()))
                                        .child("自动学习方法论（GEPA 总结），只读查看"),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(colors::BORDER()))
                                        .bg(rgb(colors::BG_PRIMARY()))
                                        .px_2()
                                        .py_1()
                                        .child(Input::new(&self.skills_search).appearance(false)),
                                ),
                        )
                        .child(list.overflow_y_scrollbar()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(detail),
                ),
        )
    }

    /// 加载学习方法论技能（`GET /skills` + `GET /skills/stats`）。
    ///
    /// 只保留 `category == "custom"`：其余类是内置能力（文件/命令/网络），
    /// 那些由工具面板覆盖（与 Web 版 `SkillsPanel` 同口径）。
    /// 统计失败不阻塞面板（只影响右上统计条与列表里的调用次数）。
    pub(crate) fn load_skills(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let list_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills");
            let stats_url = format!("http://{SERVER_HOST}:{port}/api/v1/skills/stats");
            let list_res = fetch_json(list_url).await;
            let stats_res = fetch_json(stats_url).await;

            let rows: Vec<SkillRow> = list_res
                .ok()
                .filter(|body| body.get("skills").is_some())
                .map(|body| {
                    body.get("skills")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default()
                .iter()
                .filter(|s| s.get("category").and_then(|v| v.as_str()) == Some("custom"))
                .filter_map(|s| {
                    Some(SkillRow {
                        id: s.get("id")?.as_str()?.to_string(),
                        name: s
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        description: s
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        version: s
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        updated_at: s
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string()),
                    })
                })
                .collect();

            let (stats, total_calls) = match stats_res {
                Ok(body) => {
                    let total = body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let stats: Vec<SkillStatRow> = body
                        .get("skills")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    Some(SkillStatRow {
                                        skill_id: s.get("skill_id")?.as_str()?.to_string(),
                                        total_calls: s
                                            .get("total_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        success_calls: s
                                            .get("success_calls")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0),
                                        avg_time_ms: s
                                            .get("avg_time_ms")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (stats, total)
                }
                Err(_) => (Vec::new(), 0),
            };

            let n = rows.len();
            let _ = this.update(cx, |this, cx| {
                this.skills = rows;
                this.skill_stats = stats;
                this.skills_total_calls = total_calls;
                this.skills_status = format!("{n} 个方法论技能");
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉选中技能的完整 markdown 内容（`GET /skills/{id}`）。
    pub(crate) fn load_skill_detail(&self, id: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!(
                "http://{SERVER_HOST}:{port}/api/v1/skills/{}",
                url_encode(&id)
            );
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(body) => {
                        this.skill_content = body
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string());
                        this.skills_status = "已加载技能内容".to_string();
                    }
                    Err(e) => {
                        this.skill_content = None;
                        this.skills_status = format!("加载技能详情失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
