//! 设置页：模型列表、provider 编辑、连接测试、外观。
//!

use crate::*;
// 显式导入：gpui 也导出 `View`（trait）与 `AppRef`，glob 会与我们的同名枚举/别名歧义
use crate::theme::colors;
use gpui_kit::component::input::Textarea;
use crate::types::*;
use crate::format::*;

impl ClientApp {
    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // 外观：三套色板（石墨灰 / 羊皮纸 / 午夜蓝）。切换只改全局色板 + 组件库
        // 的 ThemeMode，随后 `cx.notify()` 全量重绘 —— 所有颜色都是**渲染时**读的，
        // 没有任何缓存，所以切完立刻生效。
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
        // 提供商区：endpoint / 密钥可编辑（保存时先重读配置再写，见 `save_provider`）
        let mut providers_block = div().flex().flex_col().gap_1().px_4().pt_3().child(
            div()
                .text_xs()
                .text_color(rgb(colors::TEXT_SECONDARY()))
                .child("提供商（端点 / 密钥）"),
        );
        for (ix, p) in self.providers.iter().enumerate() {
            let name = p.name.clone();
            providers_block = providers_block.child(
                div()
                    .id(("prov", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .child(div().w(px(110.)).text_xs().child(p.name.clone()))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(ellipsis(&p.endpoint, 44)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 模型", p.model_count)),
                    )
                    .child(
                        div()
                            .id(("prov-edit", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(colors::BG_TERTIARY()))
                            .cursor_pointer()
                            .text_xs()
                            .child("编辑")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_provider_editor(name.clone(), window, cx);
                            })),
                    ),
            );
        }
        if let Some(name) = self.editing_provider.clone() {
            providers_block = providers_block.child(
                div()
                    .mt_2()
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
                            .text_xs()
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(format!("编辑 {name}")),
                    )
                    .child(Textarea::new(&self.edit_endpoint).h(px(40.)))
                    .child(Textarea::new(&self.edit_api_key).h(px(40.)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id("prov-test")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::BG_TERTIARY()))
                                    .text_color(rgb(colors::TEXT_PRIMARY()))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("测试连接")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.test_provider_connection(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-save")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors::ACCENT()))
                                    .text_color(rgb(0xffffff))
                                    .cursor_pointer()
                                    .text_xs()
                                    .child("保存")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.save_provider(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("prov-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(colors::TEXT_TERTIARY()))
                                    .child("取消")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.editing_provider = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        let mut list = div().flex().flex_col().gap_1().p_4();
        if self.models.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无模型：请先在配置里添加 provider 与模型"),
            );
        }
        for (ix, m) in self.models.iter().enumerate() {
            let provider = m.provider.clone();
            let name = m.name.clone();
            let meta = {
                let mut parts: Vec<String> = Vec::new();
                if !m.capabilities.is_empty() {
                    parts.push(m.capabilities.clone());
                }
                if let Some(ctx_len) = m.context {
                    parts.push(format!("ctx {ctx_len}"));
                }
                parts.join(" · ")
            };
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md();
            if m.is_chat {
                row = row.bg(rgb(colors::BG_HOVER()));
            }
            let action = if m.is_chat {
                div()
                    .text_xs()
                    .text_color(rgb(colors::SUCCESS()))
                    .child("当前聊天模型")
                    .into_any_element()
            } else {
                div()
                    .id(("set-chat", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(colors::BG_TERTIARY()))
                    .text_color(rgb(colors::TEXT_PRIMARY()))
                    .cursor_pointer()
                    .child("设为聊天模型")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.switch_chat_model(provider.clone(), name.clone(), cx);
                    }))
                    .into_any_element()
            };
            list = list.child(
                row.child(div().w(px(110.)).text_xs().child(m.provider.clone()))
                    .child(div().flex_1().child(m.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(meta),
                    )
                    .child(action),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors::BORDER()))
                    .child(div().text_sm().child("设置 · 模型"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(if self.settings_error {
                                rgb(colors::ERROR())
                            } else {
                                rgb(colors::TEXT_SECONDARY())
                            })
                            .child(self.settings_status.clone()),
                    ),
            )
            .child(theme_block)
            .child(providers_block)
            .child(div().flex_1().min_h_0().child(list))
    }

    /// 加载模型服务列表（`GET /api/v1/config/models`）。
    ///
    /// 只取展示与切换需要的字段：provider / 模型名 / 能力标签 / 上下文窗口 /
    /// 是否为当前 chat 首选。`preferences.chat` 是 `{model, provider}`，provider
    /// 可能缺省（同名模型只注册在一个 provider 上时）。
    pub(crate) fn load_models(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models");
            match fetch_json(url).await {
                Ok(body) => {
                    let pref = body.get("preferences").and_then(|p| p.get("chat"));
                    let chat_model = pref
                        .and_then(|c| c.get("model"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let chat_provider = pref
                        .and_then(|c| c.get("provider"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut rows: Vec<ModelRow> = Vec::new();
                    if let Some(models) = body.get("models").and_then(|v| v.as_array()) {
                        for m in models {
                            let name = m
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let provider = m
                                .get("provider")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let capabilities = m
                                .get("capabilities")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/")
                                })
                                .unwrap_or_default();
                            let context = m.get("context_length").and_then(|v| v.as_u64());
                            // 该模型声明的思考档位（空 = 不支持思考 → 选择器隐藏）
                            let reasoning_efforts = m
                                .get("reasoning_efforts")
                                .and_then(|v| v.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str())
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            let is_chat = name == chat_model
                                && (chat_provider.is_empty() || provider == chat_provider);
                            rows.push(ModelRow {
                                provider,
                                name,
                                capabilities,
                                context,
                                reasoning_efforts,
                                is_chat,
                            });
                        }
                    }
                    let n = rows.len();
                    // 再拉一次整份配置：providers 的 endpoint / api_key 只在 `/config`
                    // 里（`/config/models` 只给 name/endpoint/enabled/model_count）。
                    // 编辑回填需要它们，而回填只能在同步点击处理器里做（要 `&mut Window`）。
                    let cfg_res =
                        fetch_json(format!("http://{SERVER_HOST}:{port}/api/v1/config")).await;
                    let mut providers: Vec<ProviderRow> = Vec::new();
                    if let Ok(body) = cfg_res {
                        if let Some(arr) = body
                            .get("config")
                            .and_then(|c| c.get("models"))
                            .and_then(|m| m.get("providers"))
                            .and_then(|p| p.as_array())
                        {
                            for p in arr {
                                providers.push(ProviderRow {
                                    name: p
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    endpoint: p
                                        .get("endpoint")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    api_key: p
                                        .get("api_key")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    model_count: p
                                        .get("models")
                                        .and_then(|v| v.as_array())
                                        .map(|a| a.len())
                                        .unwrap_or(0),
                                });
                            }
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.models = rows;
                        this.providers = providers;
                        this.settings_status = format!("模型 {n} 个");
                        this.settings_error = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("加载模型列表失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 切换默认聊天模型（`POST /api/v1/config/models/switch`，持久化 preferences.chat）。
    pub(crate) fn switch_chat_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        let port = self.port;
        self.settings_status = format!("正在切换到 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/models/switch");
            // capability 用 kebab-case（core 的 ModelCapability 序列化口径）
            let body = serde_json::json!({
                "model": model,
                "capability": "chat",
                "provider": provider,
            });
            match post_json(url, body).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("已把 {model} 设为聊天模型");
                        this.settings_error = false;
                        this.load_models(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("切换失败：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 打开 provider 编辑器：把该 provider 的 endpoint / api_key 回填进输入框。
    ///
    /// 必须**同步**做（`set_value` 要 `&mut Window`），所以值取自随列表缓存的那份
    /// （见 [`ProviderRow`] 的注释：缓存只用于回填，不用于写）。
    pub(crate) fn open_provider_editor(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.providers.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        self.edit_endpoint
            .update(cx, |s, cx| s.set_value(row.endpoint, window, cx));
        self.edit_api_key
            .update(cx, |s, cx| s.set_value(row.api_key, window, cx));
        self.editing_provider = Some(name);
        self.settings_status = "编辑 provider：改完点「保存」（会先重读配置再写）".to_string();
        self.settings_error = false;
        cx.notify();
    }

    /// 测试 provider 连通性（`POST /api/v1/config/test-connection`）。
    ///
    /// 用**当前编辑框里的** endpoint / api_key 测——这样用户可以在保存前先验证，
    /// 避免把错误的 key 写进配置再回滚。model 取该 provider 的第一个模型。
    pub(crate) fn test_provider_connection(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let model = self
            .editing_provider
            .as_ref()
            .and_then(|name| {
                self.models
                    .iter()
                    .find(|m| &m.provider == name && m.capabilities.contains("chat"))
                    .or_else(|| self.models.iter().find(|m| &m.provider == name))
                    .map(|m| m.name.clone())
            })
            .unwrap_or_default();
        if endpoint.trim().is_empty() || api_key.trim().is_empty() || model.is_empty() {
            self.settings_status = "测试连接需要 endpoint / api_key / 模型名都不为空".to_string();
            self.settings_error = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.settings_status = format!("正在测试 {model}…");
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/test-connection");
            let body = serde_json::json!({
                "endpoint": endpoint,
                "api_key": api_key,
                "model": model,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        let ok = v.get("success").and_then(|b| b.as_bool()).unwrap_or(false);
                        let msg = v
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();
                        this.settings_error = !ok;
                        this.settings_status = if ok {
                            format!("连接成功：{msg}")
                        } else {
                            format!("连接失败：{msg}")
                        };
                    }
                    Err(e) => {
                        this.settings_error = true;
                        this.settings_status = format!("测试失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保存 provider 的 endpoint / api_key。
    ///
    /// ⚠️ `PUT /config` 是**整份替换**（`persist_and_reload`），不是按段合并
    /// （段保全只保「程序未建模的键」）。所以这里**先重新 GET 一份最新配置**，
    /// 只改目标 provider 的两个字段，再整体提交——把「读-改-写」的覆盖窗口压到
    /// 最短（一次往返），不要改成「用启动时缓存的配置提交」，那会冲掉期间别人
    /// 改过的建模字段（例如 agent 刚切的模型、MCP 预热注入的东西）。
    pub(crate) fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing_provider.clone() else {
            return;
        };
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let port = self.port;
        self.settings_status = format!("正在保存 {name}…");
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let mut cfg = match fetch_json(format!("{base}/config")).await {
                Ok(body) => body
                    .get("config")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.settings_status = format!("保存失败（读取配置）：{e}");
                        this.settings_error = true;
                        cx.notify();
                    });
                    return;
                }
            };
            let mut found = false;
            if let Some(arr) = cfg
                .get_mut("models")
                .and_then(|m| m.get_mut("providers"))
                .and_then(|p| p.as_array_mut())
            {
                for p in arr.iter_mut() {
                    if p.get("name").and_then(|v| v.as_str()) == Some(name.as_str()) {
                        p["endpoint"] = serde_json::Value::String(endpoint.clone());
                        p["api_key"] = serde_json::Value::String(api_key.clone());
                        found = true;
                    }
                }
            }
            if !found {
                let _ = this.update(cx, |this, cx| {
                    this.settings_status = format!("保存失败：配置里没有 provider {name}");
                    this.settings_error = true;
                    cx.notify();
                });
                return;
            }
            let res = put_json(
                format!("{base}/config"),
                serde_json::json!({ "config": cfg }),
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.settings_status = format!("{name} 已保存");
                        this.settings_error = false;
                        this.editing_provider = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.settings_status = format!("保存失败：{e}");
                        this.settings_error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
