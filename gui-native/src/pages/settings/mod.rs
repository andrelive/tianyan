//! 设置页：模型列表、provider 编辑、连接测试、外观。
//!

use crate::*;

pub(crate) mod fields;
pub(crate) mod mcp;
pub(crate) mod models;
pub(crate) mod soul;
pub(crate) mod storage;
pub(crate) mod usage;

// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::format::*;
use crate::theme::colors;
use crate::types::*;

impl ClientApp {
    /// 设置页（对齐 `gui-vite` 的 `SettingsPanel`）：左侧 14 个二级栏目 + 右侧内容。
    ///
    /// 之前是一个长页面（外观 → 提供商 → 模型全部堆在一起），用户一进去找不到东西，
    /// 也没法和 Web 版对上。栏目顺序 / 文案 / 图标与 vite 的 `TABS` 逐项一致。
    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(self.render_settings_nav(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    // 顶部：栏目名 + 全局状态行（状态是整页共用的，切栏目不丢）
                    .child(self.render_settings_header(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            // 必须自己是 flex 容器：每个栏目的根元素用 `flex_1` 撑满，
                            // 而 `flex_1` 只在 flex 父容器里生效（否则高度 = 内容高度，
                            // 表现为「人设编辑器被压成一行」「长栏目滚不动」）。
                            .flex()
                            .flex_col()
                            .child(self.render_settings_panel(cx)),
                    ),
            )
            .into_any_element()
    }

    /// 左侧二级栏目（对齐 vite `SettingsPanel` 的 `role="tablist"`）。
    ///
    /// 选中态用「左侧 2px 竖条 + accent 文字」，与 vite 的
    /// `border-l-2 border-accent text-accent` 同形。
    fn render_settings_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut nav = div()
            .w(px(196.))
            .h_full()
            .flex()
            .flex_col()
            .py_2()
            .bg(rgb(colors::BG_SECONDARY()))
            .border_r_1()
            .border_color(rgb(colors::BORDER()));
        for tab in SettingsTab::ALL {
            let active = self.settings_tab == tab;
            let fg = if active {
                colors::ACCENT()
            } else {
                colors::TEXT_SECONDARY()
            };
            let icon_fg = if active {
                colors::ACCENT()
            } else {
                colors::TEXT_TERTIARY()
            };
            nav = nav.child(
                div()
                    .id(("set-tab", tab as usize))
                    .w_full()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .border_l_2()
                    .border_color(rgb(if active {
                        colors::ACCENT()
                    } else {
                        colors::BG_SECONDARY()
                    }))
                    .text_color(rgb(fg))
                    .child(
                        svg()
                            .path(tab.icon().path())
                            .size(px(15.))
                            .text_color(rgb(icon_fg)),
                    )
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.settings_tab = tab;
                        // 配置段懒加载：没拉过就拉；拉过但还没回填进编辑器（例如
                        // `load_models` 顺手设了 `raw_config`）就补一次回填——
                        // 编辑器是空的话，保存会把所有文本字段当成「被清空」。
                        if tab.config_key().is_some() {
                            if this.raw_config.is_none() {
                                this.load_config_summary(cx);
                            } else if !this.settings_backfilled {
                                this.backfill_settings_fields(cx);
                            }
                        }
                        // 各栏目的数据源不同：用量/人设/MCP 各有自己的接口，同样懒加载
                        match tab {
                            SettingsTab::Usage => this.load_settings_usage(cx),
                            SettingsTab::Soul => this.load_soul(cx),
                            SettingsTab::Mcp => this.load_mcp_servers(cx),
                            SettingsTab::Connection | SettingsTab::About => this.load_health(cx),
                            _ => {}
                        }
                        cx.notify();
                    })),
            );
        }
        nav.into_any_element()
    }

    /// 右上角状态行 + 字段引擎的「保存 / 放弃」。
    ///
    /// 栏目分三类：① 字段引擎（存储 / Agent / 安全 / 日志 / 记忆 / 检索 / Web）——
    /// 顶部给保存条，脏计数实时算；② 自带保存（模型服务 / 人设 / MCP）——各自按钮；
    /// ③ 纯展示（外观 / 连接 / 用量 / 关于）。
    fn render_settings_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let editable = matches!(
            self.settings_tab,
            SettingsTab::Storage
                | SettingsTab::Agent
                | SettingsTab::Security
                | SettingsTab::Logging
                | SettingsTab::Memory
                | SettingsTab::Retrieval
                | SettingsTab::Web
        );
        let dirty = if editable {
            self.settings_dirty_count(cx)
        } else {
            0
        };
        // 状态文本：字段引擎的保存结果优先（它是最新动作），其次模型服务的加载结果
        let (text, is_err) = if !self.settings_msg.is_empty() {
            (self.settings_msg.clone(), self.settings_msg_err)
        } else if self.settings_tab == SettingsTab::Models {
            (self.settings_status.clone(), self.settings_error)
        } else {
            (String::new(), false)
        };
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors::BORDER()))
            .child(
                div()
                    .text_sm()
                    .child(format!("设置 · {}", self.settings_tab.label())),
            )
            .child(div().flex_1());
        if !text.is_empty() {
            bar = bar.child(
                div()
                    .max_w(px(560.))
                    .min_w_0()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(if is_err {
                        colors::ERROR()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(text),
            );
        }
        if editable {
            if dirty > 0 {
                bar = bar.child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgba(0xffffff14))
                        .text_size(px(10.))
                        .text_color(rgb(colors::WARNING()))
                        .child(format!("已修改 {dirty} 项")),
                );
            }
            bar = bar.child(
                div()
                    .id("set-discard")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(colors::BG_TERTIARY()))
                    .text_size(px(11.))
                    .text_color(rgb(if dirty > 0 {
                        colors::TEXT_PRIMARY()
                    } else {
                        colors::TEXT_TERTIARY()
                    }))
                    .child("放弃")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.discard_settings_changes(cx);
                    })),
            );
            bar = bar.child(
                div()
                    .id("set-save")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if dirty > 0 {
                        colors::ACCENT()
                    } else {
                        colors::BG_TERTIARY()
                    }))
                    .text_size(px(11.))
                    .text_color(rgb(if dirty > 0 {
                        0xffffff
                    } else {
                        colors::TEXT_TERTIARY()
                    }))
                    .child(if dirty > 0 {
                        format!("保存 {dirty} 项")
                    } else {
                        "保存".to_string()
                    })
                    .on_click(cx.listener(|this, _, _w, cx| {
                        this.save_settings_fields(cx);
                    })),
            );
        }
        bar.into_any_element()
    }

    /// 右侧内容区：按栏目分派。
    fn render_settings_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.settings_tab {
            SettingsTab::Models => self.render_settings_models(cx),
            SettingsTab::Appearance => self.render_settings_appearance(cx),
            SettingsTab::Connection => self.render_settings_connection(cx),
            SettingsTab::About => self.render_settings_about(),
            SettingsTab::Soul => self.render_settings_soul(cx),
            SettingsTab::Mcp => self.render_settings_mcp(cx),
            SettingsTab::Usage => self.render_settings_usage(cx),
            SettingsTab::Storage => self.render_settings_storage(cx),
            tab => self.render_settings_fields(tab, None, cx),
        }
    }

    /// 「连接」栏目：探活 `/health` 并展示服务的**真实**信息（版本 / 数据目录 / pid）。
    ///
    /// 比 vite 的 `ConnectionTab` 多一层：它只显示「服务地址 / 健康检查 / 上次交互」，
    /// 而 `/health` 的 `data_dir` 能一眼看出「我连的是不是我以为的那个服务」——
    /// 单 server 复用（ADR-051）下，连到另一个数据目录的实例是最隐蔽的坑。
    fn render_settings_connection(&self, cx: &mut Context<Self>) -> AnyElement {
        let port = self.port;
        let mut col = div().flex().flex_col().gap_3().p_4().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_TERTIARY()))
                .child("内嵌服务连通性"),
        );
        col = col.child(kv_line("服务地址", format!("http://{SERVER_HOST}:{port}")));
        match &self.health_info {
            Some(h) => {
                col = col.child(kv_line(
                    "健康状态",
                    format!("{} · {} · pid {}", h.status, h.service, h.pid),
                ));
                col = col.child(kv_line("服务版本", h.version.clone()));
                col = col.child(kv_line("服务数据目录", h.data_dir.clone()));
                let cfg_dir = self
                    .raw_config
                    .as_ref()
                    .and_then(|c| fields::json_get(c, "storage.data_dir"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !cfg_dir.is_empty() && !h.data_dir.is_empty() && !same_dir(cfg_dir, &h.data_dir)
                {
                    col = col.child(div().text_xs().text_color(rgb(colors::WARNING())).child(
                        format!(
                            "⚠ 配置里的数据目录（{cfg_dir}）与服务实际使用的不一致——
                                 通常表示改了配置还没重启。"
                        ),
                    ));
                }
            }
            None => {
                col = col.child(kv_line(
                    "健康检查",
                    if self.health_msg.is_empty() {
                        format!("/health · 端口 {port}（未探活）")
                    } else {
                        self.health_msg.clone()
                    },
                ));
            }
        }
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("health-refresh")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(colors::BG_TERTIARY()))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child("重新探活")
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.load_health(cx);
                        })),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(if self.status.contains("空闲") {
                            "上次交互正常（状态行显示空闲）".to_string()
                        } else {
                            self.status.clone()
                        }),
                ),
        );
        col.into_any_element()
    }

    /// 「关于」栏目：客户端/服务版本、构建形态、数据目录与协议关系。
    fn render_settings_about(&self) -> AnyElement {
        let mut col = div().flex().flex_col().gap_2().p_4();
        col = col.child(kv_line("客户端", "天演 · GPUI 原生端".to_string()));
        col = col.child(kv_line(
            "客户端版本",
            option_env!("CARGO_PKG_VERSION")
                .unwrap_or("unknown")
                .to_string(),
        ));
        col = col.child(kv_line(
            "构建形态",
            if cfg!(debug_assertions) {
                "debug（保留终端日志）"
            } else {
                "release（无控制台窗口）"
            }
            .to_string(),
        ));
        col = col.child(kv_line(
            "服务版本",
            match &self.health_info {
                Some(h) if !h.version.is_empty() => h.version.clone(),
                _ => "（未探活：去「连接」点重新探活）".to_string(),
            },
        ));
        col = col.child(kv_line("服务端口", self.port.to_string()));
        col = col.child(kv_line(
            "服务数据目录",
            match &self.health_info {
                Some(h) if !h.data_dir.is_empty() => h.data_dir.clone(),
                _ => "-".to_string(),
            },
        ));
        col = col.child(kv_line(
            "与 Web 版关系",
            "同一内嵌 server、同一套 HTTP/SSE 协议".to_string(),
        ));
        // 两条版本线是**预期**的：不解释就会被当成「装错了版本」
        col = col.child(kv_line(
            "版本说明",
            "客户端独立发版（gui-native 自带 workspace）；内嵌服务跟主 workspace \
            （tianyan-core / server 同一条版本线）。两者不一致是正常的。"
                .to_string(),
        ));
        col.into_any_element()
    }

    /// 「外观」栏目（原 `render_settings` 的主题块原样搬来）。
    fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut theme_block = div().flex().flex_col().gap_2().px_4().pt_3().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("外观"),
        );
        let mut theme_cards = div().flex().flex_row().gap_2().flex_wrap();
        for kind in ThemeKind::ALL {
            let is_cur = current_theme() == kind;
            let p = kind.palette();
            theme_cards = theme_cards.child(
                div()
                    .id(("theme", kind as usize))
                    .w(px(208.))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if is_cur {
                        colors::ACCENT()
                    } else {
                        colors::BORDER()
                    }))
                    .bg(rgb(p.bg_secondary))
                    .cursor_pointer()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // 三块色预览：页面底 / 强调块 / 强调色
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(div().flex_1().h(px(20.)).rounded_md().bg(rgb(p.bg_primary)))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(20.))
                                    .rounded_md()
                                    .bg(rgb(p.bg_tertiary)),
                            )
                            .child(div().flex_1().h(px(20.)).rounded_md().bg(rgb(p.accent))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.text_primary))
                                    .child(kind.label()),
                            )
                            .child(if is_cur {
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(p.accent))
                                    .child("✓ 当前")
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(p.text_tertiary))
                            .child(kind.hint()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_theme_with(kind, Some(window), cx);
                    })),
            );
        }
        theme_block = theme_block.child(theme_cards);
        theme_block.into_any_element()
    }

    /// 探活 `/health`（连接/关于栏目用）：拿服务真实版本、数据目录与 pid。
    ///
    /// 失败不谎报：保留错误文案并清掉上一次的 `health_info`——让「未探活」与
    /// 「探活成功但连不上」在 UI 上是两种可见状态。
    pub(crate) fn load_health(&mut self, cx: &mut Context<Self>) {
        let port = self.port;
        self.health_msg = "正在探活…".to_string();
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/health");
            let res = fetch_json(url).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        this.health_info = Some(HealthInfo::from_json(&v));
                        this.health_msg = String::new();
                    }
                    Err(e) => {
                        this.health_info = None;
                        this.health_msg = format!("探活失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉 `GET /api/v1/config`，只留 `config` 段（字段引擎与连接栏目的数据源）。
    pub(crate) fn load_config_summary(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config");
            if let Ok(body) = fetch_json(url).await {
                let cfg = body.get("config").cloned();
                if cfg.is_some() {
                    let _ = this.update(cx, |this, cx| {
                        // 首次加载才回填：已有编辑器内容时回填会覆盖用户正在输入的值
                        let first = this.raw_config.is_none();
                        this.raw_config = cfg;
                        if first {
                            this.backfill_settings_fields(cx);
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

/// 「左标签 / 右值」一行（连接栏目与关于栏目共用）。
fn kv_line(label: &'static str, value: String) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(120.))
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(colors::TEXT_PRIMARY()))
                .child(value),
        )
        .into_any_element()
}
