//! 工具页：工具目录（与 LLM 收到的 tools 列表同源）。
//!

use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）来自这个 trait，必须 in scope
use gpui_kit::component::scroll::ScrollableElement as _;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    /// 工具页：左列表（搜索 + 名称/描述）+ 右详情（参数 JSON Schema）。
    ///
    /// 与 Web 版 `ToolsPanel` 同口径：工具是**原子操作**，与「技能」（方法论）两套概念。
    /// 参数不做表单（工具是给 LLM 调用的，人只读 schema）。
    pub(crate) fn render_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.tools_query.trim().to_lowercase();
        let visible: Vec<ToolRow> = self
            .tools
            .iter()
            .filter(|t| {
                query.is_empty()
                    || t.name.to_lowercase().contains(&query)
                    || t.description.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .tools
            .iter()
            .find(|t| self.tools_selected.as_deref() == Some(t.name.as_str()))
            .cloned();

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
                            .path(IconName::Wrench.path())
                            .size(px(32.))
                            .text_color(rgb(colors::TEXT_TERTIARY())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(if self.tools.is_empty() {
                                "暂无可用工具"
                            } else {
                                "未找到匹配工具"
                            }),
                    ),
            );
        }
        for (ix, tool) in visible.iter().enumerate() {
            let is_sel = self.tools_selected.as_deref() == Some(tool.name.as_str());
            let name = tool.name.clone();
            list = list.child(
                div()
                    .id(("tool", ix))
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
                            .child(tool.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(ellipsis(&tool.description, 56)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tools_selected = Some(name.clone());
                        cx.notify();
                    })),
            );
        }

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
                        .path(IconName::Wrench.path())
                        .size(px(40.))
                        .text_color(rgb(colors::TEXT_TERTIARY())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("选择一个工具查看详情"),
                )
                .into_any_element(),
            Some(tool) => {
                let schema = serde_json::to_string_pretty(&tool.parameters)
                    .unwrap_or_else(|_| "（参数无法序列化）".to_string());
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
                                    .child(tool.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child(tool.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("参数 Schema"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .bg(rgb(colors::BG_SECONDARY()))
                                    .p_3()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(colors::TEXT_SECONDARY()))
                                            .child(schema)
                                            .overflow_y_scrollbar(),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
        };

        div()
            .size_full()
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
                                            .path(IconName::Wrench.path())
                                            .size(px(16.))
                                            .text_color(rgb(colors::TEXT_SECONDARY())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(colors::TEXT_PRIMARY()))
                                            .child("工具"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child(format!(
                                        "系统内置原子操作（{} 个），LLM 通过 tool call 调用",
                                        self.tools.len()
                                    )),
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
                                    .child(Input::new(&self.tools_search).appearance(false)),
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
            )
    }

    /// 加载工具目录（`GET /api/v1/tools`：与 LLM 收到的 tools 列表同源）。
    pub(crate) fn load_tools(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/tools");
            match fetch_json(url).await {
                Ok(body) => {
                    let rows: Vec<ToolRow> = body
                        .get("tools")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|t| {
                                    Some(ToolRow {
                                        name: t.get("name")?.as_str()?.to_string(),
                                        description: t
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        parameters: t
                                            .get("parameters")
                                            .cloned()
                                            .unwrap_or(serde_json::Value::Null),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = this.update(cx, |this, cx| {
                        this.tools = rows;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("加载工具失败：{e}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
