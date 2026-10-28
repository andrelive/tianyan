//! 子智能体页：角色列表 + 调用统计 + 定义详情。
//!

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use crate::types::*;

impl ClientApp {
    pub(crate) fn render_roles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.roles_query.trim().to_lowercase();
        let visible: Vec<RoleRow> = self
            .roles
            .iter()
            .filter(|r| {
                query.is_empty()
                    || r.name.to_lowercase().contains(&query)
                    || r.purpose.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        // 左：角色列表
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
                            .path(IconName::Bot.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.roles.is_empty() {
                                "暂无角色"
                            } else {
                                "未找到匹配子智能体"
                            }),
                    ),
            );
        } else {
            for (ix, role) in visible.iter().enumerate() {
                list = list.child(self.render_role_item(ix, role, cx));
            }
        }

        // 右：统计条 + 详情
        let mut stats_bar = div()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .flex_wrap()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("累计委托"),
                    )
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!("{}", self.roles_stats.total_calls)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child("成功率"),
                    )
                    .child(
                        div()
                            .text_color(rgb(if self.roles_stats.success_rate < 0.5 {
                                colors::ERROR()
                            } else {
                                colors::SUCCESS()
                            }))
                            .child(format!(
                                "{}%",
                                (self.roles_stats.success_rate * 100.0).round() as u32
                            )),
                    ),
            );
        if !self.roles_stats.by_task_type.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("任务类型："),
                );
            for (kind, count) in &self.roles_stats.by_task_type {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{kind} {count}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }
        if !self.roles_stats.by_role.is_empty() {
            let mut chips = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("按角色："),
                );
            for (name, calls) in &self.roles_stats.by_role {
                chips = chips.child(
                    div()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors::BORDER()))
                        .bg(rgb(colors::BG_SECONDARY()))
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(format!("{name} {calls}")),
                );
            }
            stats_bar = stats_bar.child(chips);
        }

        let detail = match self
            .roles_selected
            .as_ref()
            .and_then(|name| self.roles.iter().find(|r| &r.name == name))
        {
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    svg()
                        .path(IconName::Bot.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个子智能体查看详情"),
                )
                .into_any_element(),
            Some(role) => {
                let (source_label, source_color) = source_of(&role.source);
                let card = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_6()
                            .py_4()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
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
                                            .text_size(px(16.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child(role.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(source_color))
                                            .text_size(px(11.))
                                            .text_color(rgb(source_color))
                                            .child(source_label),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(format!("v{}", role.version)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(match role.tool_count {
                                                Some(n) => format!("{n} 工具"),
                                                None => "工具不限".to_string(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_TERTIARY()))
                                            .child(format!(
                                                "模型：{}",
                                                role.model
                                                    .clone()
                                                    .unwrap_or_else(|| "回落主 Agent".to_string())
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(role.purpose.clone()),
                            ),
                    )
                    // 操作栏（对齐 Web 版 RolesPanel）：回退内置种子 / 退役删除
                    .child(self.render_role_actions(role, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("系统提示"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_SECONDARY()))
                                    .p_3()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(match &self.role_detail {
                                        Some(d) if !d.system_prompt.is_empty() => {
                                            d.system_prompt.clone()
                                        }
                                        Some(_) => "（空）".to_string(),
                                        None => "加载中…".to_string(),
                                    }),
                            )
                            .children(self.role_detail.as_ref().map(|d| {
                                if d.tools.is_empty() {
                                    div().into_any_element()
                                } else {
                                    let mut chips = div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_1()
                                        .flex_wrap()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                                .child("工具白名单"),
                                        );
                                    for tool in &d.tools {
                                        chips = chips.child(
                                            div()
                                                .px_1()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(rgb(colors::BORDER()))
                                                .text_size(px(11.))
                                                .text_color(rgb(colors::TEXT_SECONDARY()))
                                                .child(tool.clone()),
                                        );
                                    }
                                    chips.into_any_element()
                                }
                            }))
                            .overflow_y_scrollbar(),
                    );
                card.into_any_element()
            }
        };

        div()
            .size_full()
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
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors::BORDER()))
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
                                        svg()
                                            .path(IconName::Users.path())
                                            .size(px(18.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("子智能体"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("角色化分工（内置 / 配置 / 学习三源平级），自演化更新"),
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
                                    .child(Input::new(&self.roles_search).appearance(false)),
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
                    .child(stats_bar)
                    .child(detail),
            )
    }

    pub(crate) fn render_role_actions(&self, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let reset_name = role.name.clone();
        let delete_name = role.name.clone();
        let reset_disabled = role.source == "builtin" && role.version <= 1;
        let reset_color = if reset_disabled {
            colors::TEXT_TERTIARY()
        } else {
            colors::TEXT_SECONDARY()
        };
        div()
            .px_6()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("role-reset")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(reset_color))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::RotateCcw.path())
                            .size(px(12.))
                            .text_color(rgb(reset_color)),
                    )
                    .child("回退内置种子")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if reset_disabled {
                            return;
                        }
                        this.dialog = Some(Dialog::ResetRole {
                            name: reset_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("role-delete")
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors::ERROR()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors::ERROR()))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(IconName::Trash.path())
                            .size(px(12.))
                            .text_color(rgb(colors::ERROR())),
                    )
                    .child("退役删除")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dialog = Some(Dialog::DeleteRole {
                            name: delete_name.clone(),
                        });
                        cx.notify();
                    })),
            )
            .children(self.roles_action_msg.clone().map(|m| {
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(m)
            }))
            .into_any_element()
    }

    /// 角色列表项：名称 + 来源徽章 + 职责 + 版本 / 工具数 / 成功率。
    pub(crate) fn render_role_item(&self, ix: usize, role: &RoleRow, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.roles_selected.as_deref() == Some(role.name.as_str());
        let (source_label, source_color) = source_of(&role.source);
        let name = role.name.clone();
        let usage_text = role
            .usage
            .filter(|(calls, _)| *calls > 0)
            .map(|(calls, rate)| {
                (
                    format!("成功率 {}%", (rate * 100.0).round() as u32),
                    rate < 0.5,
                    calls,
                )
            });
        div()
            .id(("role", ix))
            .w_full()
            .px_2()
            .py_2()
            .rounded_lg()
            .bg(rgb(if selected {
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
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(13.))
                            .text_color(rgb(if selected {
                                colors::ACCENT()
                            } else {
                                colors::TEXT_PRIMARY()
                            }))
                            .child(role.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(source_color))
                            .text_size(px(10.))
                            .text_color(rgb(source_color))
                            .child(source_label),
                    )
                    .child(if role.status == "experimental" {
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(colors::WARNING()))
                            .text_size(px(10.))
                            .text_color(rgb(colors::WARNING()))
                            .child("试验性")
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .overflow_hidden()
                    .child(role.purpose.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(format!("v{}", role.version))
                    .child(match role.tool_count {
                        Some(n) => format!("{n} 工具"),
                        None => "工具不限".to_string(),
                    })
                    .children(usage_text.map(|(text, low, calls)| {
                        div()
                            .text_color(rgb(if low {
                                colors::ERROR()
                            } else {
                                colors::TEXT_TERTIARY()
                            }))
                            .child(format!("{text}（{calls} 次）"))
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.roles_selected = Some(name.clone());
                this.role_detail = None;
                this.load_role_detail(name.clone(), cx);
                cx.notify();
            }))
            .into_any_element()
    }

    pub(crate) fn load_roles(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            match fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles")).await {
                Ok(body) => {
                    let rows: Vec<RoleRow> = body
                        .get("roles")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|r| RoleRow {
                            name: r
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            source: r
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            status: r
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            version: r.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            purpose: r
                                .get("purpose")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            tool_count: r.get("tool_count").and_then(|v| v.as_u64()),
                            model: r
                                .get("model")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                            usage: r.get("usage").map(|u| {
                                (
                                    u.get("calls").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                                    u.get("success_rate")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or(0.0) as f32,
                                )
                            }),
                        })
                        .collect();
                    let n = rows.len();
                    let _ = this.update(cx, |this, cx| {
                        this.roles = rows;
                        this.roles_status = format!("{n} 个子智能体");
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.roles_status = format!("加载子智能体失败：{e}");
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 加载子智能体统计（`GET /roles/stats`）：页面顶部那条汇总。
    pub(crate) fn load_roles_stats(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            if let Ok(body) =
                fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/roles/stats")).await
            {
                let stats = RolesStats {
                    total_calls: body
                        .get("total_calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    success_rate: body
                        .get("success_rate")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0) as f32,
                    by_task_type: body
                        .get("by_task_type")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|pair| {
                                    let p = pair.as_array()?;
                                    Some((
                                        p.first()?.as_str()?.to_string(),
                                        p.get(1)?.as_u64()? as usize,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    by_role: body
                        .get("by_role")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|r| {
                                    Some((
                                        r.get("name")?.as_str()?.to_string(),
                                        r.get("calls")?.as_u64()? as u32,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.roles_stats = stats;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 加载角色详情（`GET /roles/{name}`）。
    pub(crate) fn load_role_detail(&self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/roles/{name}");
            if let Ok(body) = fetch_json(url).await {
                let detail = RoleDetail {
                    system_prompt: body
                        .get("system_prompt")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tools: body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let _ = this.update(cx, |this, cx| {
                    this.role_detail = Some(detail);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
