//! 设置页「Agent」栏目：**命令执行环境的事实卡片**（只读）+ 可编辑字段。
//!
//! 卡片存在的理由：`[executor].shell = "auto"` 时「实际用了哪个 shell」此前只写在
//! 服务启动日志里。实测踩过一次——用户装了 PowerShell 7，却在一处报错里看到 5.1，
//! 只能翻日志才排除 auto 选错（真因是别处裸写了 `powershell`，与 auto 无关）。
//! 现在把**服务进程里已安装的 spec** 直接显示出来：数据源是
//! `GET /api/v1/config/executor`，与执行侧同源（`shell::current()`），不是前端
//! 按平台二次推算——探测逻辑只能有一份。
//!
//! 为什么不做成字段表里的一项：字段表的语义是「可编辑 + 参与保存」，而这个值是
//! **运行时事实**（配置写 auto 时它与配置值本就不同）。硬塞进字段表会让脏检测
//! 与保存逻辑都多一条无意义的分支。

use crate::theme::colors;
use crate::types::*;
use crate::*;

impl ClientApp {
    /// 拉 `GET /api/v1/config/executor`（生效的执行底层）。
    ///
    /// 失败时清掉旧值并保留错误文案：界面必须能区分「没拉过」与「拉失败」——
    /// 显示一个过期的 shell 会让用户以为切换没生效。
    pub(crate) fn load_executor_shell(&mut self, cx: &mut Context<Self>) {
        let port = self.port;
        self.executor_shell_msg = "正在读取…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/executor");
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        this.executor_shell = Some(ExecutorShellInfo::from_json(&v));
                        this.executor_shell_msg = String::new();
                    }
                    Err(e) => {
                        this.executor_shell = None;
                        this.executor_shell_msg = format!("读取执行底层失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 「当前生效」卡片：服务进程真正在用的 shell（身份 / 可执行路径 / 参数模板）。
    pub(crate) fn render_executor_shell_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut card = div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors::BORDER()))
            .bg(rgb(colors::BG_SECONDARY()))
            .p_3()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .pb_1()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child("命令执行环境（当前生效）"),
                    )
                    .child(
                        div()
                            .id("exec-shell-refresh")
                            .px_2()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(colors::BORDER()))
                            .cursor_pointer()
                            .text_size(px(9.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("刷新")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_executor_shell(cx);
                            })),
                    ),
            );
        // 说明：这个值来自服务进程，不是配置里的字符串。
        card = card.child(
            div()
                .pb_1()
                .text_size(px(10.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child("读的是服务进程里已生效的 spec（配置为「自动」时即探测命中的那个）。"),
        );
        match &self.executor_shell {
            Some(info) => {
                card = card
                    .child(fact_line("Shell", info.kind_label(), "exec-shell-kind", cx))
                    .child(fact_line(
                        "可执行",
                        info.executable.clone(),
                        "exec-shell-path",
                        cx,
                    ))
                    .child(fact_line(
                        "参数模板",
                        info.args.join(" "),
                        "exec-shell-args",
                        cx,
                    ));
                // 配置值 vs 生效值：不一致就是「改了没生效」，必须显式说，
                // 否则用户只会觉得「我明明选了 5.1，怎么还是 7」。
                let configured = info.configured.trim();
                if !configured.is_empty() && configured != "auto" && configured != info.kind {
                    card = card.child(
                        div()
                            .pt_1()
                            .text_size(px(10.))
                            .text_color(rgb(colors::WARNING()))
                            .child(format!(
                                "⚠ 配置写的是「{configured}」，实际生效「{}」——通常是保存还没过、\
                                 或服务刚改配置尚未重载。保存后点「刷新」核对。",
                                info.kind
                            )),
                    );
                }
            }
            None => {
                let text = if self.executor_shell_msg.is_empty() {
                    "尚未读取（切到本栏目会自动拉取）。".to_string()
                } else {
                    self.executor_shell_msg.clone()
                };
                let is_err = self.executor_shell.is_none()
                    && self.executor_shell_msg.starts_with("读取执行底层失败");
                card = card.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(if is_err {
                            colors::ERROR()
                        } else {
                            colors::TEXT_TERTIARY()
                        }))
                        .child(text),
                );
            }
        }
        card.into_any_element()
    }
}

/// 「标签 / 值 / 复制」一行（长路径没法用鼠标选中，必须给复制按钮）。
fn fact_line(
    label: &'static str,
    value: String,
    copy_id: &'static str,
    cx: &mut Context<ClientApp>,
) -> AnyElement {
    let copy = value.clone();
    let has_value = !value.trim().is_empty();
    let mut row = div()
        .flex()
        .flex_row()
        .items_start()
        .gap_3()
        .py_0p5()
        .child(
            div()
                .w(px(72.))
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(11.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(if has_value {
                    value
                } else {
                    "（空）".to_string()
                }),
        );
    if has_value {
        row = row.child(
            div()
                .id(copy_id)
                .px_2()
                .rounded_sm()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .cursor_pointer()
                .text_size(px(9.))
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child("复制")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy.clone()));
                    this.push_notice("已复制到剪贴板", false);
                    cx.notify();
                })),
        );
    }
    row.into_any_element()
}
