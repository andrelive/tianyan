//! 设置页「MCP」栏目：服务器列表 / 启停 / 测试连接 / 新增 / 删除。
//!
//! 比 Web 版多两处：① 每台服务器旁边直接显示**连接测试结果**（工具数 / 失败原因），
//! vite 版是弹 toast；② 新增表单带 `args` 与 `description`（vite 只有 name/command）。

use crate::theme::colors;
use crate::types::*;
use crate::widgets::field_wrap;
use crate::*;
// 滚动扩展方法（.overflow_y_scrollbar()）
use gpui_kit::component::scroll::ScrollableElement as _;

impl ClientApp {
    /// 拉 MCP 服务器列表（`GET /config/mcp/servers`）。
    pub(crate) fn load_mcp_servers(&mut self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/mcp/servers");
            match fetch_json(url).await {
                Ok(v) => {
                    let rows: Vec<McpServerRow> = v
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| {
                                    Some(McpServerRow {
                                        name: s.get("name")?.as_str()?.to_string(),
                                        command: s
                                            .get("command")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                        args: s
                                            .get("args")
                                            .and_then(|v| v.as_array())
                                            .map(|a| {
                                                a.iter()
                                                    .filter_map(|x| x.as_str())
                                                    .map(|x| x.to_string())
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                        enabled: s
                                            .get("enabled")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(true),
                                        description: s
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string(),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = this.update(cx, |this, cx| {
                        this.mcp_servers = rows;
                        this.mcp_msg = String::new();
                        this.mcp_msg_err = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.mcp_msg = format!("加载 MCP 服务器失败：{e}");
                        this.mcp_msg_err = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 启用/禁用（`PUT /config/mcp/servers/{name}`）。
    pub(crate) fn toggle_mcp_server(
        &mut self,
        name: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let port = self.port;
        self.mcp_msg = format!("正在{} {name}…", if enabled { "启用" } else { "禁用" });
        let name_for_msg = name.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/mcp/servers/{name}");
            let res = put_json(url, serde_json::json!({ "enabled": enabled })).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.mcp_msg =
                            format!("{name_for_msg} 已{}", if enabled { "启用" } else { "禁用" });
                        this.mcp_msg_err = false;
                        this.load_mcp_servers(cx);
                    }
                    Err(e) => {
                        this.mcp_msg = format!("切换失败：{e}");
                        this.mcp_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 测试连接（`POST /config/mcp/servers/{name}/test`，会起子进程握手）。
    pub(crate) fn test_mcp_server(&mut self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        let key = name.clone();
        self.mcp_test.insert(key.clone(), "测试中…".to_string());
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/mcp/servers/{name}/test");
            let res = post_json(url, serde_json::json!({})).await;
            let _ = this.update(cx, |this, cx| {
                let text = match res {
                    Ok(v) => {
                        let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                        if ok {
                            let tools = v.get("tools").and_then(|t| t.as_u64()).unwrap_or(0);
                            format!("连接成功 · {tools} 个工具")
                        } else {
                            let err = v
                                .get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("未知原因")
                                .to_string();
                            format!("失败：{err}")
                        }
                    }
                    Err(e) => format!("测试失败：{e}"),
                };
                this.mcp_test.insert(key.clone(), text);
                cx.notify();
            });
        })
        .detach();
    }

    /// 新增服务器（`POST /config/mcp/servers`）。
    pub(crate) fn add_mcp_server(&mut self, cx: &mut Context<Self>) {
        let name = self.mcp_name.read(cx).value().trim().to_string();
        let command = self.mcp_command.read(cx).value().trim().to_string();
        let args_raw = self.mcp_args.read(cx).value().to_string();
        let description = self.mcp_desc.read(cx).value().trim().to_string();
        if name.is_empty() || command.is_empty() {
            self.mcp_msg = "名称与命令都不能为空".to_string();
            self.mcp_msg_err = true;
            cx.notify();
            return;
        }
        let args: Vec<String> = args_raw.split_whitespace().map(|s| s.to_string()).collect();
        let port = self.port;
        self.mcp_msg = format!("正在添加 {name}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/mcp/servers");
            let body = serde_json::json!({
                "name": name,
                "command": command,
                "args": args,
                "enabled": true,
                "description": if description.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(description) },
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.mcp_msg = "已添加 MCP 服务器".to_string();
                        this.mcp_msg_err = false;
                        this.mcp_form_open = false;
                        this.load_mcp_servers(cx);
                    }
                    Err(e) => {
                        this.mcp_msg = format!("添加失败：{e}");
                        this.mcp_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除服务器（`DELETE /config/mcp/servers/{name}`）。
    pub(crate) fn remove_mcp_server(&mut self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        let key = name.clone();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/mcp/servers/{name}");
            let res = delete_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.mcp_msg = format!("{key} 已移除");
                        this.mcp_msg_err = false;
                        this.mcp_test.remove(&key);
                        this.load_mcp_servers(cx);
                    }
                    Err(e) => {
                        this.mcp_msg = format!("移除失败：{e}");
                        this.mcp_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 渲染「MCP」栏目。
    pub(crate) fn render_settings_mcp(&self, cx: &mut Context<Self>) -> AnyElement {
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
                        "MCP 服务器（stdio 传输）随客户端启动注入工具；改动会持久化到 \
                         tianyan.toml 并热重载。测试连接会起一次子进程完成握手。",
                    ),
            );
        if !self.mcp_msg.is_empty() {
            col = col.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(if self.mcp_msg_err {
                        colors::ERROR()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(self.mcp_msg.clone()),
            );
        }
        let mut list = div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .p_3()
            .flex()
            .flex_col()
            .gap_1();
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .pb_1()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(format!("已配置 {} 台", self.mcp_servers.len())),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("mcp-add-toggle")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(colors::BG_TERTIARY()))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(if self.mcp_form_open {
                            "收起新增"
                        } else {
                            "新增服务器"
                        })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.mcp_form_open = !this.mcp_form_open;
                            cx.notify();
                        })),
                ),
        );
        if self.mcp_servers.is_empty() {
            list = list.child(
                div()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("还没有配置 MCP 服务器。"),
            );
        }
        for (ix, s) in self.mcp_servers.iter().enumerate() {
            let name = s.name.clone();
            let name_toggle = name.clone();
            let name_test = name.clone();
            let name_remove = name.clone();
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .border_t_1()
                .border_color(rgb(colors::BORDER()))
                .child(
                    div()
                        .w(px(140.))
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(format!(
                            "{}{}{}",
                            s.command,
                            if s.args.is_empty() { "" } else { " " },
                            s.args.join(" ")
                        )),
                )
                .child(
                    div()
                        .w(px(150.))
                        .min_w_0()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child(self.mcp_test.get(&name).cloned().unwrap_or_default()),
                )
                // 启停开关
                .child(
                    div()
                        .id(("mcp-toggle", ix))
                        .w(px(36.))
                        .h(px(20.))
                        .rounded_full()
                        .bg(rgb(if s.enabled {
                            colors::ACCENT()
                        } else {
                            colors::BG_TERTIARY()
                        }))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .ml(px(if s.enabled { 18. } else { 2. }))
                                .size(px(16.))
                                .rounded_full()
                                .bg(rgb(0xffffff))
                                .into_any_element(),
                        )
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            let enabled = this
                                .mcp_servers
                                .iter()
                                .find(|x| x.name == name_toggle)
                                .map(|x| x.enabled)
                                .unwrap_or(false);
                            this.toggle_mcp_server(name_toggle.clone(), !enabled, cx);
                        })),
                )
                .child(
                    div()
                        .id(("mcp-test", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(colors::BG_TERTIARY()))
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child("测试")
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            this.test_mcp_server(name_test.clone(), cx);
                        })),
                )
                .child(
                    div()
                        .id(("mcp-remove", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(10.))
                        .text_color(rgb(colors::ERROR()))
                        .child("移除")
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            this.remove_mcp_server(name_remove.clone(), cx);
                        })),
                );
            list = list.child(row);
            if !s.description.is_empty() {
                list = list.child(
                    div()
                        .pl(px(140.))
                        .pb_1()
                        .text_size(px(9.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(s.description.clone()),
                );
            }
        }
        col = col.child(list);

        if self.mcp_form_open {
            let mut form = div()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_SECONDARY()))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_SECONDARY()))
                        .child("新增 MCP 服务器"),
                )
                .child(field_wrap(&self.mcp_name))
                .child(field_wrap(&self.mcp_command))
                .child(field_wrap(&self.mcp_args))
                .child(field_wrap(&self.mcp_desc))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .id("mcp-add")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(colors::ACCENT()))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(rgb(0xffffff))
                                .child("添加")
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.add_mcp_server(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(colors::TEXT_TERTIARY()))
                                .child("args 用空格分隔；命令如 npx / uvx / python"),
                        ),
                );
            form = form.child(
                div()
                    .text_size(px(9.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("四行依次为：名称 · 命令 · 参数（空格分隔）· 说明（可选）"),
            );
            col = col.child(form);
        }
        col.into_any_element()
    }
}
