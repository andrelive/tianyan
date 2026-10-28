//! 设置页「人设编辑」栏目：soul 全文（`tianyan://agent/soul`）的读取 / 编辑 / 恢复默认。
//!
//! 与 Web 版 `SoulTab` 的差别：vite 版只有一个 textarea + 保存按钮，看不到「当前内容
//! 是不是默认值」「改了多少字」；这里补上字数、与默认值的差异提示，以及「恢复默认」
//! （只填入编辑器、不直接落盘——让用户先看一眼再决定保存）。

use crate::theme::colors;
use crate::*;
use gpui_kit::component::input::Textarea;

impl ClientApp {
    /// 把人设正文写进编辑器。
    ///
    /// 不依赖 `render`：经窗口句柄现取 `&mut Window`（隐藏窗口不会触发渲染，
    /// 早期「置 pending 标志、由 render 消费」的写法在自动化验收里直接失效）。
    pub(crate) fn set_soul_content(&mut self, text: String, cx: &mut Context<Self>) {
        let editor = self.soul_editor.clone();
        let handle = self.window_handle;
        let _ = handle.update(cx, move |_, window, cx| {
            editor.update(cx, |state, cx| state.set_value(text, window, cx));
        });
        cx.notify();
    }

    /// 拉人设正文 + 默认正文（成功加载过一次就不再重复拉：用户可能正在编辑）。
    pub(crate) fn load_soul(&mut self, cx: &mut Context<Self>) {
        if self.soul_loaded {
            return;
        }
        let port = self.port;
        self.soul_msg = "正在加载人设…".to_string();
        self.soul_msg_err = false;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let soul = fetch_json(format!("{base}/config/soul")).await;
            let def = fetch_json(format!("{base}/config/soul/default")).await;
            let _ = this.update(cx, |this, cx| {
                match soul {
                    Ok(body) => {
                        let content = body
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        this.set_soul_content(content, cx);
                        this.soul_loaded = true;
                        this.soul_msg = String::new();
                        this.soul_msg_err = false;
                    }
                    Err(e) => {
                        this.soul_msg = format!("加载人设失败：{e}");
                        this.soul_msg_err = true;
                    }
                }
                if let Ok(body) = def {
                    this.soul_default = body
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保存人设（`PUT /config/soul`）。
    pub(crate) fn save_soul(&mut self, cx: &mut Context<Self>) {
        let content = self.soul_editor.read(cx).value().to_string();
        if content.trim().is_empty() {
            self.soul_msg = "人设内容不能为空".to_string();
            self.soul_msg_err = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.soul_msg = "正在保存人设…".to_string();
        self.soul_msg_err = false;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/soul");
            match put_json(url, serde_json::json!({ "content": content })).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.soul_msg = "人设已保存（下一个会话生效）".to_string();
                        this.soul_msg_err = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.soul_msg = format!("保存失败：{e}");
                        this.soul_msg_err = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 把编辑器内容替换成内置默认人设（**不落盘**：先让用户看再决定保存）。
    pub(crate) fn fill_soul_default(&mut self, cx: &mut Context<Self>) {
        if self.soul_default.is_empty() {
            self.soul_msg = "默认人设尚未加载".to_string();
            self.soul_msg_err = true;
            cx.notify();
            return;
        }
        let text = self.soul_default.clone();
        self.set_soul_content(text, cx);
        self.soul_msg = "已填入默认人设（点「保存」才生效）".to_string();
        self.soul_msg_err = false;
        cx.notify();
    }

    /// 渲染「人设编辑」栏目。
    pub(crate) fn render_settings_soul(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.soul_editor.read(cx).value().to_string();
        let chars = text.chars().count();
        let lines = text.lines().count();
        let same_as_default = !self.soul_default.is_empty() && text == self.soul_default;
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("智能体核心人设（soul）"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{chars} 字 · {lines} 行")),
                    )
                    .child(if same_as_default {
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(rgba(0xffffff14))
                            .text_size(px(9.))
                            .text_color(rgb(colors::SUCCESS()))
                            .child("与内置默认一致")
                            .into_any_element()
                    } else {
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(rgba(0xffffff14))
                            .text_size(px(9.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("与默认不同")
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child(
                        "写入 `tianyan://agent/soul`（VFS），下一个会话组装上下文时生效；\
                         默认人设来自编译期内置（`DEFAULT_SOUL`），改坏了可以一键填回。",
                    ),
            );
        if !self.soul_msg.is_empty() {
            col = col.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(if self.soul_msg_err {
                        colors::ERROR()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(self.soul_msg.clone()),
            );
        }
        // 正文：占满剩余高度（多行，自带滚动）
        col = col
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .p_2()
                    .child(Textarea::new(&self.soul_editor).appearance(false).h_full()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("保存后不影响进行中的会话，只影响之后新建/唤醒的轮次"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("soul-fill-default")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::BG_TERTIARY()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("填入默认")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = window;
                                this.fill_soul_default(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("soul-save")
                            .px_4()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::ACCENT()))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(rgb(0xffffff))
                            .child("保存人设")
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.save_soul(cx);
                            })),
                    ),
            );
        col.into_any_element()
    }
}
