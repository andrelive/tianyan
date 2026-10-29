//! 设置页「模型服务」栏目：提供商（端点/密钥/增删）+ 模型（偏好切换/增删）。
//!
//! 比 Web 版强在哪（都是它「不大好使」的地方）：
//! - **偏好切换即时生效**：vite 要改三个 select 再滚到底点「保存设置」（整份提交）；
//!   这里每个模型直接给「设为对话 / 设为嵌入 / 设为视觉」按钮，点击即时落库
//!   （同一套「读完整配置 → 只改这个字段 → 整份 PUT」，窗口一次往返）。
//! - **偏好可见**：行内直接标「当前对话 / 当前嵌入 / 当前视觉」，不用回头对三个下拉。
//! - **能增删**：提供商与模型都能加、能删（行内二次确认），vite 只有加模型。
//!
//! ⚠️ `/config/models/switch` 只覆盖 `preferences.chat`（服务端实现里 capability
//! 参数不参与写入），所以嵌入/视觉偏好统一走 `/config` 的整份 PUT，不新增接口。

use crate::theme::colors;
use crate::types::*;
use crate::widgets::field_wrap;
use crate::*;
use gpui_kit::component::input::Textarea;
// 滚动扩展方法（.overflow_y_scrollbar()）
use gpui_kit::component::scroll::ScrollableElement as _;

/// 模型能力标签（kebab-case，与 `core::config::ModelCapability` 的序列化口径一致）。
pub(crate) const CAPS: [(&str, &str); 4] = [
    ("chat", "对话"),
    ("vision", "视觉"),
    ("text-embedding", "文本嵌入"),
    ("multimodal-embedding", "多模态嵌入"),
];

impl ClientApp {
    /// 渲染「模型服务」栏目。
    pub(crate) fn render_settings_models(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_4()
            .overflow_y_scrollbar();

        // 状态行（本栏目最近一次动作的结果）
        if !self.models_msg.is_empty() {
            col = col.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(if self.models_msg_err {
                        colors::ERROR()
                    } else {
                        colors::TEXT_SECONDARY()
                    }))
                    .child(self.models_msg.clone()),
            );
        }

        // ── 提供商 ──
        let mut prov_card = div()
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
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!("提供商（{}）", self.providers.len())),
                    )
                    .child(div().flex_1())
                    .child(button(
                        "prov-add-toggle",
                        if self.prov_form_open {
                            "收起新增"
                        } else {
                            "新增提供商"
                        },
                        cx.listener(|this, _, _w, cx| {
                            this.prov_form_open = !this.prov_form_open;
                            cx.notify();
                        }),
                    )),
            );
        for (ix, p) in self.providers.iter().enumerate() {
            let name = p.name.clone();
            let name_edit = name.clone();
            let name_del = name.clone();
            let confirming = self
                .delete_confirm
                .as_ref()
                .map(|(kind, n, _m)| kind == "provider" && *n == name)
                .unwrap_or(false);
            let mut actions = div().flex().flex_row().items_center().gap_1();
            if confirming {
                actions = actions
                    .child(button_danger(
                        ("prov-del-ok", ix),
                        "确认删除",
                        cx.listener(move |this, _, _w, cx| {
                            this.remove_provider(name_del.clone(), cx);
                        }),
                    ))
                    .child(button(
                        ("prov-del-no", ix),
                        "取消",
                        cx.listener(|this, _, _w, cx| {
                            this.delete_confirm = None;
                            cx.notify();
                        }),
                    ));
            } else {
                actions = actions
                    .child(button(
                        ("prov-edit", ix),
                        "编辑",
                        cx.listener(move |this, _, window, cx| {
                            this.open_provider_editor(name_edit.clone(), window, cx);
                        }),
                    ))
                    .child(button(
                        ("prov-del", ix),
                        "删除",
                        cx.listener(move |this, _, _w, cx| {
                            this.delete_confirm =
                                Some(("provider".to_string(), name.clone(), String::new()));
                            cx.notify();
                        }),
                    ));
            }
            prov_card = prov_card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .border_t_1()
                    .border_color(rgb(colors::BORDER()))
                    .child(
                        div()
                            .w(px(120.))
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(p.name.clone()),
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
                            .child(if p.endpoint.is_empty() {
                                "（端点留空：按预置解析）".to_string()
                            } else {
                                p.endpoint.clone()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child(format!("{} 模型", p.model_count)),
                    )
                    .child(actions),
            );
        }
        if self.providers.is_empty() {
            prov_card = prov_card.child(
                div()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("还没有配置提供商。"),
            );
        }
        if let Some(name) = self.editing_provider.clone() {
            prov_card = prov_card.child(
                div()
                    .mt_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors::BORDER()))
                    .bg(rgb(colors::BG_PRIMARY()))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors::TEXT_SECONDARY()))
                            .child(format!("编辑 {name} · 端点与密钥")),
                    )
                    .child(Textarea::new(&self.edit_endpoint).h(px(36.)))
                    .child(Textarea::new(&self.edit_api_key).h(px(36.)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(button(
                                "prov-test",
                                "测试连接",
                                cx.listener(|this, _, _w, cx| {
                                    this.test_provider_connection(cx);
                                }),
                            ))
                            .child(button_accent(
                                "prov-save",
                                "保存",
                                cx.listener(|this, _, _w, cx| {
                                    this.save_provider(cx);
                                }),
                            ))
                            .child(button(
                                "prov-cancel",
                                "取消",
                                cx.listener(|this, _, _w, cx| {
                                    this.editing_provider = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        if self.prov_form_open {
            let form = div()
                .mt_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_PRIMARY()))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(
                            "名称 · 端点（可留空按预置解析）· 密钥（可选）· 首个模型（必填，\
                             后端要求提供商至少有一个模型）",
                        ),
                )
                .child(field_wrap(&self.prov_form_name))
                .child(field_wrap(&self.prov_form_endpoint))
                .child(field_wrap(&self.prov_form_key))
                .child(field_wrap(&self.prov_form_model))
                .child({
                    let mut caps_row = div().flex().flex_row().gap_1().flex_wrap();
                    for (key, label) in CAPS {
                        let on = self.prov_form_caps.contains(&key);
                        caps_row = caps_row.child(
                            div()
                                .id(("prov-form-cap", key.len()))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .border_1()
                                .border_color(rgb(if on {
                                    colors::ACCENT()
                                } else {
                                    colors::BORDER()
                                }))
                                .bg(rgb(if on {
                                    colors::BG_TERTIARY()
                                } else {
                                    colors::BG_PRIMARY()
                                }))
                                .text_size(px(10.))
                                .text_color(rgb(if on {
                                    colors::TEXT_PRIMARY()
                                } else {
                                    colors::TEXT_SECONDARY()
                                }))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    if let Some(pos) =
                                        this.prov_form_caps.iter().position(|c| *c == key)
                                    {
                                        this.prov_form_caps.remove(pos);
                                    } else {
                                        this.prov_form_caps.push(key);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    caps_row
                })
                .child(button_accent(
                    "prov-create",
                    "添加提供商",
                    cx.listener(|this, _, _w, cx| {
                        this.add_provider(cx);
                    }),
                ));
            prov_card = prov_card.child(form);
        }
        col = col.child(prov_card);

        // ── 模型 ──
        let mut model_card = div()
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
                            .text_size(px(12.))
                            .text_color(rgb(colors::TEXT_PRIMARY()))
                            .child(format!("模型（{}）", self.models.len())),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(colors::TEXT_TERTIARY()))
                            .child("点按钮即时切换首选，无需「保存设置」"),
                    )
                    .child(div().flex_1())
                    .child(button(
                        "model-add-toggle",
                        if self.model_form_open {
                            "收起新增"
                        } else {
                            "新增模型"
                        },
                        cx.listener(|this, _, _w, cx| {
                            this.model_form_open = !this.model_form_open;
                            if this.model_form_provider.is_none() {
                                this.model_form_provider =
                                    this.providers.first().map(|p| p.name.clone());
                            }
                            cx.notify();
                        }),
                    )),
            );
        if self.models.is_empty() {
            model_card = model_card.child(
                div()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors::TEXT_TERTIARY()))
                    .child("暂无模型：先在下方「新增模型」里添加。"),
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
            let has_chat = m.caps.iter().any(|c| c == "chat");
            let has_vision = m.caps.iter().any(|c| c == "vision");
            let has_embed = m
                .caps
                .iter()
                .any(|c| c == "text-embedding" || c == "multimodal-embedding");
            let mut badges = div().flex().flex_row().items_center().gap_1();
            for (flag, label) in [
                (m.is_chat, "对话"),
                (m.is_embedding, "嵌入"),
                (m.is_vision, "视觉"),
            ] {
                if flag {
                    badges = badges.child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(rgba(0xffffff14))
                            .text_size(px(9.))
                            .text_color(rgb(colors::SUCCESS()))
                            .child(format!("当前{label}")),
                    );
                }
            }
            // 偏好按钮：只在模型确实具备该能力时出现（避免「设为嵌入」把对话模型
            // 写成嵌入首选——那会让嵌入服务直接报错）
            let mut pref_actions = div().flex().flex_row().items_center().gap_1();
            if has_chat && !m.is_chat {
                pref_actions = pref_actions.child(button_small(
                    ("pref-chat", ix),
                    "设为对话",
                    cx.listener({
                        let p = provider.clone();
                        let n = name.clone();
                        move |this, _, _w, cx| {
                            this.set_model_preference(p.clone(), n.clone(), "chat", cx);
                        }
                    }),
                ));
            }
            if has_embed && !m.is_embedding {
                pref_actions = pref_actions.child(button_small(
                    ("pref-emb", ix),
                    "设为嵌入",
                    cx.listener({
                        let p = provider.clone();
                        let n = name.clone();
                        move |this, _, _w, cx| {
                            this.set_model_preference(p.clone(), n.clone(), "embedding", cx);
                        }
                    }),
                ));
            }
            if has_vision && !m.is_vision {
                pref_actions = pref_actions.child(button_small(
                    ("pref-vis", ix),
                    "设为视觉",
                    cx.listener({
                        let p = provider.clone();
                        let n = name.clone();
                        move |this, _, _w, cx| {
                            this.set_model_preference(p.clone(), n.clone(), "vision", cx);
                        }
                    }),
                ));
            }
            let confirming = self
                .delete_confirm
                .as_ref()
                .map(|(kind, p, mm)| kind == "model" && *p == provider && *mm == name)
                .unwrap_or(false);
            if confirming {
                pref_actions = pref_actions
                    .child(button_danger(
                        ("model-del-ok", ix),
                        "确认删除",
                        cx.listener({
                            let p = provider.clone();
                            let n = name.clone();
                            move |this, _, _w, cx| {
                                this.remove_model(p.clone(), n.clone(), cx);
                            }
                        }),
                    ))
                    .child(button_small(
                        ("model-del-no", ix),
                        "取消",
                        cx.listener(|this, _, _w, cx| {
                            this.delete_confirm = None;
                            cx.notify();
                        }),
                    ));
            } else {
                pref_actions = pref_actions.child(button_small(
                    ("model-del", ix),
                    "删除",
                    cx.listener({
                        let p = provider.clone();
                        let n = name.clone();
                        move |this, _, _w, cx| {
                            this.delete_confirm = Some(("model".to_string(), p.clone(), n.clone()));
                            cx.notify();
                        }
                    }),
                ));
            }
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .border_t_1()
                .border_color(rgb(colors::BORDER()));
            if m.is_chat {
                row = row.bg(rgb(colors::BG_HOVER()));
            }
            model_card = model_card.child(
                row.child(
                    div()
                        .w(px(96.))
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(m.provider.clone()),
                )
                .child(
                    div()
                        .w(px(240.))
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_size(px(11.))
                        .text_color(rgb(colors::TEXT_PRIMARY()))
                        .child(m.name.clone()),
                )
                .child(
                    div()
                        .w(px(214.))
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .overflow_hidden()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child(meta),
                )
                .child(badges)
                .child(div().flex_1())
                .child(pref_actions),
            );
        }
        if self.model_form_open {
            let mut form = div()
                .mt_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(colors::BORDER()))
                .bg(rgb(colors::BG_PRIMARY()))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(colors::TEXT_TERTIARY()))
                        .child("提供商（点选）· 模型名（与上游一致）· 能力标签"),
                );
            let mut prov_row = div().flex().flex_row().gap_1().flex_wrap();
            for (ix, p) in self.providers.iter().enumerate() {
                let selected = self.model_form_provider.as_deref() == Some(p.name.as_str());
                let name = p.name.clone();
                prov_row = prov_row.child(
                    div()
                        .id(("model-form-prov", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .border_1()
                        .border_color(rgb(if selected {
                            colors::ACCENT()
                        } else {
                            colors::BORDER()
                        }))
                        .bg(rgb(if selected {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_PRIMARY()
                        }))
                        .text_size(px(10.))
                        .text_color(rgb(if selected {
                            colors::TEXT_PRIMARY()
                        } else {
                            colors::TEXT_SECONDARY()
                        }))
                        .child(p.name.clone())
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            this.model_form_provider = Some(name.clone());
                            cx.notify();
                        })),
                );
            }
            form = form
                .child(prov_row)
                .child(field_wrap(&self.model_form_name));
            let mut caps_row = div().flex().flex_row().gap_1().flex_wrap();
            for (key, label) in CAPS {
                let on = self.model_form_caps.contains(&key);
                caps_row = caps_row.child(
                    div()
                        .id(("model-form-cap", key.len()))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .border_1()
                        .border_color(rgb(if on {
                            colors::ACCENT()
                        } else {
                            colors::BORDER()
                        }))
                        .bg(rgb(if on {
                            colors::BG_TERTIARY()
                        } else {
                            colors::BG_PRIMARY()
                        }))
                        .text_size(px(10.))
                        .text_color(rgb(if on {
                            colors::TEXT_PRIMARY()
                        } else {
                            colors::TEXT_SECONDARY()
                        }))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            if let Some(pos) = this.model_form_caps.iter().position(|c| *c == key) {
                                this.model_form_caps.remove(pos);
                            } else {
                                this.model_form_caps.push(key);
                            }
                            cx.notify();
                        })),
                );
            }
            form = form.child(caps_row).child(button_accent(
                "model-create",
                "添加模型",
                cx.listener(|this, _, _w, cx| {
                    this.add_model_to_provider(cx);
                }),
            ));
            model_card = model_card.child(form);
        }
        col = col.child(model_card);
        col.into_any_element()
    }

    /// 切换某个偏好（`chat` / `embedding` / `vision`）到指定模型。
    ///
    /// 为什么走 `/config` 整份 PUT 而不是 `/config/models/switch`：后者的服务端实现
    /// 只写 `preferences.chat`（`capability` 参数不参与写入），没法切嵌入/视觉。
    /// 这里沿用「GET 最新 → 只改这一个字段 → PUT」的保守写法。
    pub(crate) fn set_model_preference(
        &mut self,
        provider: String,
        model: String,
        kind: &str,
        cx: &mut Context<Self>,
    ) {
        let kind = kind.to_string();
        let label = match kind.as_str() {
            "chat" => "对话",
            "embedding" => "嵌入",
            "vision" => "视觉",
            other => {
                self.models_msg = format!("未知偏好：{other}");
                self.models_msg_err = true;
                cx.notify();
                return;
            }
        };
        let port = self.port;
        self.models_msg = format!("正在把 {model} 设为{label}模型…");
        self.models_msg_err = false;
        // 闭包会 move `model`（拼进 preferences），回调里还要用它做文案 → 先留一份
        let model_echo = model.clone();
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let res = read_modify_write(&base, move |cfg| {
                if let Some(prefs) = cfg
                    .get_mut("models")
                    .and_then(|m| m.get_mut("preferences"))
                    .and_then(|p| p.as_object_mut())
                {
                    prefs.insert(
                        kind.clone(),
                        serde_json::json!({ "provider": provider.clone(), "model": model.clone() }),
                    );
                    Ok(())
                } else {
                    Err("配置里没有 models.preferences".to_string())
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.models_msg = format!("{model_echo} 已设为{label}模型");
                        this.models_msg_err = false;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("切换失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 添加模型（`POST /config/providers/add-model`）。
    pub(crate) fn add_model_to_provider(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.model_form_provider.clone() else {
            self.models_msg = "请先选择提供商（没有提供商就先新增一个）".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        };
        let name = self.model_form_name.read(cx).value().trim().to_string();
        let caps: Vec<String> = self.model_form_caps.iter().map(|c| c.to_string()).collect();
        if name.is_empty() {
            self.models_msg = "模型名不能为空".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        if caps.is_empty() {
            self.models_msg = "至少选一个能力标签".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        let endpoint = self
            .providers
            .iter()
            .find(|p| p.name == provider)
            .map(|p| p.endpoint.clone())
            .unwrap_or_default();
        let port = self.port;
        self.models_msg = format!("正在添加 {name}…");
        self.models_msg_err = false;
        cx.spawn(async move |this, cx| {
            let url = format!("http://{SERVER_HOST}:{port}/api/v1/config/providers/add-model");
            let body = serde_json::json!({
                "provider_name": provider,
                "endpoint": endpoint,
                "model_name": name,
                "capabilities": caps,
            });
            let res = post_json(url, body).await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.models_msg = "模型已添加".to_string();
                        this.models_msg_err = false;
                        this.model_form_open = false;
                        // 清空模型名输入框（要 `&mut Window`：经窗口句柄现取）
                        let editor = this.model_form_name.clone();
                        let handle = this.window_handle;
                        let _ = handle.update(cx, move |_, window, cx| {
                            editor
                                .update(cx, |state, cx| state.set_value(String::new(), window, cx));
                        });
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("添加失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除模型：从配置里移除该 provider 下的这个模型。
    pub(crate) fn remove_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        let port = self.port;
        self.models_msg = format!("正在删除 {model}…");
        self.models_msg_err = false;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let p2 = provider.clone();
            let m2 = model.clone();
            let res = read_modify_write(&base, move |cfg| {
                let arr = cfg
                    .get_mut("models")
                    .and_then(|m| m.get_mut("providers"))
                    .and_then(|p| p.as_array_mut())
                    .ok_or_else(|| "配置里没有 models.providers".to_string())?;
                let entry = arr
                    .iter_mut()
                    .find(|p| p.get("name").and_then(|v| v.as_str()) == Some(p2.as_str()))
                    .ok_or_else(|| format!("找不到提供商 {p2}"))?;
                let models = entry
                    .get_mut("models")
                    .and_then(|m| m.as_array_mut())
                    .ok_or_else(|| "该提供商没有 models 数组".to_string())?;
                let before = models.len();
                models.retain(|m| m.get("name").and_then(|v| v.as_str()) != Some(m2.as_str()));
                if models.len() == before {
                    return Err(format!("{p2} 下没有模型 {m2}"));
                }
                Ok(())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.models_msg = format!("{model} 已删除");
                        this.models_msg_err = false;
                        this.delete_confirm = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("删除失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 添加提供商（只写 name/endpoint/api_key，其余字段走 serde 默认）。
    pub(crate) fn add_provider(&mut self, cx: &mut Context<Self>) {
        let name = self.prov_form_name.read(cx).value().trim().to_string();
        let endpoint = self.prov_form_endpoint.read(cx).value().trim().to_string();
        let key = self.prov_form_key.read(cx).value().trim().to_string();
        let model = self.prov_form_model.read(cx).value().trim().to_string();
        let caps: Vec<String> = self.prov_form_caps.iter().map(|c| c.to_string()).collect();
        if name.is_empty() {
            self.models_msg = "提供商名称不能为空".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        if model.is_empty() {
            self.models_msg = "首个模型名不能为空（后端要求提供商至少有一个模型）".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        if caps.is_empty() {
            self.models_msg = "请为首个模型选至少一个能力标签".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.models_msg = format!("正在添加提供商 {name}…");
        self.models_msg_err = false;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let n = name.clone();
            let res = read_modify_write(&base, move |cfg| {
                let arr = cfg
                    .get_mut("models")
                    .and_then(|m| m.get_mut("providers"))
                    .and_then(|p| p.as_array_mut())
                    .ok_or_else(|| "配置里没有 models.providers".to_string())?;
                if arr
                    .iter()
                    .any(|p| p.get("name").and_then(|v| v.as_str()) == Some(n.as_str()))
                {
                    return Err(format!("提供商 {n} 已存在"));
                }
                let mut entry = serde_json::json!({
                    "name": n,
                    "endpoint": endpoint,
                    "models": [{ "name": model, "capabilities": caps }],
                });
                if !key.is_empty() {
                    entry["api_key"] = serde_json::Value::String(key);
                }
                arr.push(entry);
                Ok(())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.models_msg = format!("提供商 {name} 已添加（含首个模型）");
                        this.models_msg_err = false;
                        this.prov_form_open = false;
                        this.prov_form_caps.clear();
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("添加失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 删除提供商（连同其下模型）。
    pub(crate) fn remove_provider(&mut self, name: String, cx: &mut Context<Self>) {
        let port = self.port;
        self.models_msg = format!("正在删除提供商 {name}…");
        self.models_msg_err = false;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let n = name.clone();
            let res = read_modify_write(&base, move |cfg| {
                let arr = cfg
                    .get_mut("models")
                    .and_then(|m| m.get_mut("providers"))
                    .and_then(|p| p.as_array_mut())
                    .ok_or_else(|| "配置里没有 models.providers".to_string())?;
                let before = arr.len();
                arr.retain(|p| p.get("name").and_then(|v| v.as_str()) != Some(n.as_str()));
                if arr.len() == before {
                    return Err(format!("没有提供商 {n}"));
                }
                Ok(())
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.models_msg = format!("提供商 {name} 已删除");
                        this.models_msg_err = false;
                        this.delete_confirm = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("删除失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开 provider 编辑器：把该 provider 的 endpoint / api_key 回填进输入框。
    pub(crate) fn open_provider_editor(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.providers.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        self.edit_endpoint
            .update(cx, |s, cx| s.set_value(row.endpoint, window, cx));
        self.edit_api_key
            .update(cx, |s, cx| s.set_value(row.api_key, window, cx));
        self.editing_provider = Some(name);
        self.models_msg = "编辑提供商：改完点「保存」（会先重读配置再写）".to_string();
        self.models_msg_err = false;
        cx.notify();
    }

    /// 测试提供商连通性（`POST /config/test-connection`，用编辑框里的值）。
    pub(crate) fn test_provider_connection(&mut self, cx: &mut Context<Self>) {
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let model = self
            .editing_provider
            .as_ref()
            .and_then(|name| {
                self.models
                    .iter()
                    .find(|m| &m.provider == name && m.caps.iter().any(|c| c == "chat"))
                    .or_else(|| self.models.iter().find(|m| &m.provider == name))
                    .map(|m| m.name.clone())
            })
            .unwrap_or_default();
        if endpoint.trim().is_empty() || api_key.trim().is_empty() || model.is_empty() {
            self.models_msg = "测试连接需要端点 / 密钥 / 模型名都不为空".to_string();
            self.models_msg_err = true;
            cx.notify();
            return;
        }
        let port = self.port;
        self.models_msg = format!("正在测试 {model}…");
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
                        this.models_msg_err = !ok;
                        this.models_msg = if ok {
                            format!("连接成功：{msg}")
                        } else {
                            format!("连接失败：{msg}")
                        };
                    }
                    Err(e) => {
                        this.models_msg_err = true;
                        this.models_msg = format!("测试失败：{e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保存 provider 的 endpoint / api_key（GET 最新 → 改两个字段 → PUT）。
    pub(crate) fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing_provider.clone() else {
            return;
        };
        let endpoint = self.edit_endpoint.read(cx).value().to_string();
        let api_key = self.edit_api_key.read(cx).value().to_string();
        let port = self.port;
        self.models_msg = format!("正在保存 {name}…");
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let n = name.clone();
            let res = read_modify_write(&base, move |cfg| {
                let arr = cfg
                    .get_mut("models")
                    .and_then(|m| m.get_mut("providers"))
                    .and_then(|p| p.as_array_mut())
                    .ok_or_else(|| "配置里没有 models.providers".to_string())?;
                let mut found = false;
                for p in arr.iter_mut() {
                    if p.get("name").and_then(|v| v.as_str()) == Some(n.as_str()) {
                        p["endpoint"] = serde_json::Value::String(endpoint.clone());
                        p["api_key"] = serde_json::Value::String(api_key.clone());
                        found = true;
                    }
                }
                if found {
                    Ok(())
                } else {
                    Err(format!("配置里没有 provider {n}"))
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.models_msg = format!("{name} 已保存");
                        this.models_msg_err = false;
                        this.editing_provider = None;
                        this.load_models(cx);
                    }
                    Err(e) => {
                        this.models_msg = format!("保存失败：{e}");
                        this.models_msg_err = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 加载模型服务列表（`GET /config/models` + `GET /config` 的 providers）。
    ///
    /// 三件事一起做：
    /// 1. 模型行（含能力标签原值 `caps`、三种偏好的命中标记）；
    /// 2. provider 行（端点/密钥/模型数，供编辑回填——密钥只在 `/config` 里）；
    /// 3. 同时刷新 `raw_config`：设置页的其他栏目（字段引擎）依赖它。
    pub(crate) fn load_models(&self, cx: &mut Context<Self>) {
        let port = self.port;
        cx.spawn(async move |this, cx| {
            let base = format!("http://{SERVER_HOST}:{port}/api/v1");
            let models_res = fetch_json(format!("{base}/config/models")).await;
            let cfg_res = fetch_json(format!("{base}/config")).await;
            let _ = this.update(cx, |this, cx| {
                match models_res {
                    Ok(body) => {
                        let pref_of = |key: &str| -> (String, String) {
                            let pref = body
                                .get("preferences")
                                .and_then(|p| p.get(key))
                                .filter(|v| !v.is_null());
                            (
                                pref.and_then(|c| c.get("model"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                pref.and_then(|c| c.get("provider"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            )
                        };
                        let (chat_model, chat_provider) = pref_of("chat");
                        let (emb_model, emb_provider) = pref_of("embedding");
                        let (vis_model, vis_provider) = pref_of("vision");
                        // 命中判定：模型名相同且（provider 缺省或也相同）
                        let hit = |m: &str, p: &str, want_model: &str, want_provider: &str| {
                            !want_model.is_empty()
                                && m == want_model
                                && (want_provider.is_empty() || p == want_provider)
                        };
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
                                let caps: Vec<String> = m
                                    .get("capabilities")
                                    .and_then(|v| v.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|x| x.as_str())
                                            .map(|x| x.to_string())
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let context = m.get("context_length").and_then(|v| v.as_u64());
                                let reasoning_efforts = m
                                    .get("reasoning_efforts")
                                    .and_then(|v| v.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|x| x.as_str())
                                            .map(|x| x.to_string())
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                rows.push(ModelRow {
                                    is_chat: hit(&name, &provider, &chat_model, &chat_provider),
                                    is_embedding: hit(&name, &provider, &emb_model, &emb_provider),
                                    is_vision: hit(&name, &provider, &vis_model, &vis_provider),
                                    capabilities: caps.join("/"),
                                    caps,
                                    context,
                                    reasoning_efforts,
                                    name,
                                    provider,
                                });
                            }
                        }
                        let n = rows.len();
                        let mut providers: Vec<ProviderRow> = Vec::new();
                        if let Ok(body) = cfg_res {
                            let cfg = body.get("config").cloned();
                            if let Some(arr) = cfg
                                .as_ref()
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
                            if let Some(cfg) = cfg {
                                this.raw_config = Some(cfg);
                            }
                        }
                        this.models = rows;
                        this.providers = providers;
                        if this.model_form_provider.is_none() {
                            this.model_form_provider =
                                this.providers.first().map(|p| p.name.clone());
                        }
                        this.settings_status = format!("模型 {n} 个");
                        this.settings_error = false;
                        // 首次启动引导的判据就在这里：**有没有可用对话模型**。
                        // 不用 `config/status` 的 `configured` —— 内嵌 server 起来后
                        // 配置文件可能已存在（甚至自定义 TIANYAN_CONFIG 路径根本没被
                        // 那个判据看到），而「一个对话模型都没有」才是真的不能用。
                        this.recompute_setup();
                        cx.notify();
                    }
                    Err(e) => {
                        this.settings_status = format!("加载模型列表失败：{e}");
                        this.settings_error = true;
                        this.models_msg = format!("加载失败：{e}");
                        this.models_msg_err = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// 「读最新配置 → 局部修改 → 整份 PUT」的统一实现。
///
/// `PUT /config` 是整份替换（服务端 `persist_and_reload`），所以每个写操作都必须
/// 先拿最新配置再改，否则会把期间别处改过的字段冲回去。`f` 只负责改自己关心的
/// 那一小块，失败返回错误文案。
async fn read_modify_write<F>(base: &str, f: F) -> Result<(), String>
where
    F: FnOnce(&mut serde_json::Value) -> Result<(), String>,
{
    let body = fetch_json(format!("{base}/config")).await?;
    let mut cfg = body
        .get("config")
        .cloned()
        .ok_or_else(|| "后端未返回 config".to_string())?;
    if !cfg.is_object() {
        return Err("后端返回的 config 不是对象".to_string());
    }
    f(&mut cfg)?;
    put_json(
        format!("{base}/config"),
        serde_json::json!({ "config": cfg }),
    )
    .await
    .map(|_| ())
}

/// 普通按钮（灰底）。
fn button(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(colors::BG_TERTIARY()))
        .cursor_pointer()
        .text_size(px(10.))
        .text_color(rgb(colors::TEXT_PRIMARY()))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 主色按钮。
fn button_accent(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(colors::ACCENT()))
        .cursor_pointer()
        .text_size(px(10.))
        .text_color(rgb(0xffffff))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 危险按钮（删除确认）。
fn button_danger(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(colors::ERROR()))
        .cursor_pointer()
        .text_size(px(10.))
        .text_color(rgb(0xffffff))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 行内小按钮（偏好切换 / 取消 / 删除入口）。
fn button_small(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(colors::BORDER()))
        .cursor_pointer()
        .text_size(px(9.))
        .text_color(rgb(colors::TEXT_SECONDARY()))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

