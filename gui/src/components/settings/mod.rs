use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api::config::{fetch_models, switch_chat_model, ModelsResponse};
use crate::state::{AppAction, AppState, FontSize, Settings, Theme};

#[derive(Properties, PartialEq)]
pub struct SettingsPanelProps {
    pub state: UseReducerHandle<AppState>,
}

#[function_component(SettingsPanel)]
pub fn settings_panel(props: &SettingsPanelProps) -> Html {
    let state = props.state.clone();
    let settings = state.settings.clone();

    let models = use_state(|| None::<ModelsResponse>);
    let switch_result = use_state(|| None::<Result<String, String>>);

    {
        let models = models.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_models().await {
                    Ok(data) => models.set(Some(data)),
                    Err(e) => {
                        web_sys::console::error_1(&format!("获取模型列表失败: {}", e).into());
                    }
                }
            });
            || ()
        });
    }

    let on_theme_change = {
        let state = state.clone();
        let settings = settings.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<web_sys::HtmlSelectElement>() {
                let theme = match select.value().as_str() {
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    _ => Theme::System,
                };
                let new_settings = Settings {
                    theme,
                    ..settings.clone()
                };
                state.dispatch(AppAction::UpdateSettings(new_settings));
            }
        })
    };

    let on_font_size_change = {
        let state = state.clone();
        let settings = settings.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<web_sys::HtmlSelectElement>() {
                let font_size = match select.value().as_str() {
                    "small" => FontSize::Small,
                    "large" => FontSize::Large,
                    _ => FontSize::Medium,
                };
                let new_settings = Settings {
                    font_size,
                    ..settings.clone()
                };
                state.dispatch(AppAction::UpdateSettings(new_settings));
            }
        })
    };

    let on_api_url_change = {
        let state = state.clone();
        let settings = settings.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                let api_base_url = input.value();
                let new_settings = Settings {
                    api_base_url,
                    ..settings.clone()
                };
                state.dispatch(AppAction::UpdateSettings(new_settings));
            }
        })
    };

    let on_back_click = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(crate::state::View::Chat));
        })
    };

    let on_switch_model = {
        let switch_result = switch_result.clone();
        let models_state = models.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<web_sys::HtmlSelectElement>() {
                let selected = select.value();
                if selected.is_empty() {
                    return;
                }
                let switch_result = switch_result.clone();
                let models_state = models_state.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match switch_chat_model(&selected).await {
                        Ok(resp) => {
                            switch_result.set(Some(Ok(resp.message)));
                            if let Ok(data) = fetch_models().await {
                                models_state.set(Some(data));
                            }
                        }
                        Err(e) => {
                            switch_result.set(Some(Err(e.to_string())));
                        }
                    }
                });
            }
        })
    };

    html! {
        <div class="settings-panel">
            <div class="settings-header">
                <button
                    class="btn btn-icon btn-back"
                    onclick={on_back_click}
                    title="返回"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="24" height="24">
                        <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
                    </svg>
                </button>
                <h2>{"设置"}</h2>
            </div>

            <div class="settings-content">
                <div class="settings-section">
                    <h3>{"模型"}</h3>

                    if let Some(ref data) = *models {
                        <div class="setting-item">
                            <label>{"当前聊天模型"}</label>
                            <div class="setting-info">
                                <span class="model-name">{ &data.default_chat_model }</span>
                            </div>
                        </div>

                        <div class="setting-item">
                            <label for="model-switch">{"切换聊天模型"}</label>
                            <select
                                id="model-switch"
                                onchange={on_switch_model}
                            >
                                <option value="">{"选择模型..."}</option>
                                {
                                    data.services.iter().filter(|s| s.enabled).map(|s| {
                                        let selected = s.default_model == data.default_chat_model;
                                        html! {
                                            <option value={s.default_model.clone()} selected={selected}>
                                                {format!("{} ({})", s.default_model, s.name)}
                                            </option>
                                        }
                                    }).collect::<Vec<Html>>()
                                }
                            </select>
                        </div>

                        if let Some(ref result) = *switch_result {
                            <div class={classes!(
                                "switch-result",
                                result.is_ok().then_some("success"),
                                result.is_err().then_some("error"),
                            )}>
                                {
                                    match result {
                                        Ok(msg) => html! { <span class="success-msg">{ msg }</span> },
                                        Err(err) => html! { <span class="error-msg">{ err }</span> },
                                    }
                                }
                            </div>
                        }

                        <div class="setting-item">
                            <label>{"已配置的服务"}</label>
                            <div class="services-list">
                                {
                                    data.services.iter().map(|s| {
                                        html! {
                                            <div class="service-card">
                                                <div class="service-header">
                                                    <span class="service-name">{ &s.name }</span>
                                                    <span class={classes!(
                                                        "service-status",
                                                        s.enabled.then_some("enabled"),
                                                        (!s.enabled).then_some("disabled"),
                                                    )}>
                                                        { if s.enabled { "启用" } else { "禁用" } }
                                                    </span>
                                                </div>
                                                <div class="service-detail">
                                                    <span class="service-endpoint">{ &s.endpoint }</span>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Html>()
                                }
                            </div>
                        </div>
                    }
                </div>

                <div class="settings-section">
                    <h3>{"外观"}</h3>

                    <div class="setting-item">
                        <label for="theme">{"主题"}</label>
                        <select
                            id="theme"
                            value={match settings.theme {
                                Theme::Light => "light",
                                Theme::Dark => "dark",
                                Theme::System => "system",
                            }}
                            onchange={on_theme_change}
                        >
                            <option value="light">{"浅色"}</option>
                            <option value="dark">{"深色"}</option>
                            <option value="system">{"跟随系统"}</option>
                        </select>
                    </div>

                    <div class="setting-item">
                        <label for="font-size">{"字体大小"}</label>
                        <select
                            id="font-size"
                            value={match settings.font_size {
                                FontSize::Small => "small",
                                FontSize::Medium => "medium",
                                FontSize::Large => "large",
                            }}
                            onchange={on_font_size_change}
                        >
                            <option value="small">{"小"}</option>
                            <option value="medium">{"中"}</option>
                            <option value="large">{"大"}</option>
                        </select>
                    </div>
                </div>

                <div class="settings-section">
                    <h3>{"连接"}</h3>

                    <div class="setting-item">
                        <label for="api-url">{"API 地址"}</label>
                        <input
                            id="api-url"
                            type="text"
                            value={settings.api_base_url.clone()}
                            oninput={on_api_url_change}
                            placeholder="http://localhost:3000"
                        />
                        <span class="setting-hint">
                            {"后端服务器地址，修改后需要刷新页面生效"}
                        </span>
                    </div>
                </div>

                <div class="settings-section">
                    <h3>{"关于"}</h3>

                    <div class="about-content">
                        <div class="about-logo">
                            <span class="about-icon">{"🔭"}</span>
                            <h4>{"天演 Tianyan"}</h4>
                        </div>
                        <p class="about-version">{"版本 0.1.0"}</p>
                        <p class="about-description">
                            {"天演是一个基于 Rust 的智能助手系统，提供知识管理、对话和检索功能。"}
                        </p>
                        <div class="about-links">
                            <a
                                href="https://github.com/tianyan/tianyan"
                                target="_blank"
                                rel="noopener noreferrer"
                            >
                                {"GitHub"}
                            </a>
                            <a
                                href="#"
                                target="_blank"
                                rel="noopener noreferrer"
                            >
                                {"文档"}
                            </a>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}
