//! 首次启动引导（`GET /config/status` 报 `configured=false` 时全屏拦截）。
//!
//! 为什么不是复刻 Web 的 6 步向导：native 的「设置 · 模型服务」栏目已经能完整
//! 配置提供商 / 模型 / 偏好（新增提供商带首个模型、测试连接、行内设为对话模型）。
//! 再写一套 6 步表单等于把同一件事实现两遍（`AGENTS.md`：已有链路不叠加抽象），
//! 而且两套表单迟早漂移成「向导能配的、设置页配不了」。
//!
//! 这里做的是三件真正缺的事：**说清还差什么**、**一键跳到能配的地方**、**重新检测**。

use crate::*;
use crate::theme::colors;
use gpui_kit::assets::IconName;

impl ClientApp {
    /// 首次启动引导页：全屏铺满，替代主界面（此时主界面所有面板都拿不到数据）。
    pub(crate) fn render_setup_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_chat = self.models.iter().any(|m| m.is_chat);
        let status_line = if self.needs_setup {
            "还没有配置文件（~/.tianyan/tianyan.toml）—— 先配一个模型提供商才能对话。"
                .to_string()
        } else if !has_chat {
            format!("已加载 {} 个模型，但**没有标记为对话模型**的那个。", self.models.len())
        } else {
            "配置似乎完整，可以继续。".to_string()
        };
        div()
            // 全屏覆盖层（挂在根 `relative` 容器上，盖住主界面）
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(colors::BG_PRIMARY()))
            .child(
                div()
                    .w(px(560.))
                    .rounded_2xl()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_SECONDARY()))
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .path(IconName::Sparkles.path())
                                    .size(px(20.))
                                    .text_color(rgb(colors::ACCENT())),
                            )
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .child("欢迎使用天演"),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(status_line),
                    )
                    .child(
                        div()
                            .w_full()
                            .rounded_lg()
                            .bg(rgb(colors::BG_PRIMARY()))
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("1. 点「去配置模型服务」→ 「新增提供商」（填端点与密钥，同时带一个模型）")
                            .child("2. 在该模型上点「设为对话」")
                            .child("3. 回到这里点「重新检测」（或直接关窗重开）"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            // 去配置：切到设置页的模型服务栏目（设置页已有全部能力）
                            .child(
                                div()
                                    .id("setup-goto")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("仍然进入应用")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        // 逃生舱：配置文件存在但检测误判时，不该把人挡在门外
                                        this.setup_dismissed = true;
                                        this.recompute_setup();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("setup-recheck")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(colors::BORDER()))
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(colors::TEXT_SECONDARY()))
                                    .child("重新检测")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.load_models(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("setup-configure")
                                    .px_3()
                                    .py_1()
                                    .rounded_lg()
                                    .bg(rgb(colors::ACCENT()))
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(0xffffff))
                                    .child("去配置模型服务")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.switch_view(View::Settings, cx);
                                        this.settings_tab = SettingsTab::Models;
                                        this.load_models(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
