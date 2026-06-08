//! GUI 前端入口。

mod api;
mod components;
mod state;
mod utils;

use uuid::Uuid;
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;
use yew::prelude::*;

use components::config_wizard::api::fetch_config_status_async;
use components::{ChatPanel, ConfigWizard, KnowledgePanel, SettingsPanel, Sidebar, SkillsPanel};
use state::{AppAction, AppState, FontSize, Theme, ToastType, View};

/// 应用模式
#[derive(Debug, Clone, PartialEq)]
enum AppMode {
    /// 加载中
    Loading,
    /// 显示配置向导
    Wizard,
    /// 显示主应用
    Main,
}

#[function_component(App)]
fn app() -> Html {
    let mode = use_state(|| AppMode::Loading);
    let state = use_reducer(AppState::default);
    let default_system_prompt = use_state(String::new);

    // 检查配置状态
    {
        let mode = mode.clone();
        let state = state.clone();
        let default_system_prompt = default_system_prompt.clone();
        use_effect_with((), move |_| {
            fetch_config_status_async(move |result| match result {
                Ok(config_status) => {
                    if config_status.configured {
                        let new_session_id = Uuid::new_v4().to_string();
                        state.dispatch(AppAction::SetCurrentSession(Some(new_session_id)));
                        mode.set(AppMode::Main);
                    } else {
                        if let Some(prompt) = config_status.default_system_prompt {
                            default_system_prompt.set(prompt);
                        }
                        mode.set(AppMode::Wizard);
                    }
                }
                Err(_) => {
                    mode.set(AppMode::Wizard);
                }
            });
            || ()
        });
    }

    let on_wizard_complete = {
        let mode = mode.clone();
        Callback::from(move |_| {
            mode.set(AppMode::Main);
        })
    };

    // 将主题和字号设置应用到 DOM
    {
        let settings = state.settings.clone();
        use_effect_with(settings, move |settings| {
            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    if let Some(html) = doc.document_element() {
                        if let Ok(el) = html.dyn_into::<HtmlElement>() {
                            // 应用主题
                            let old_class = el.class_name();
                            let mut classes: Vec<&str> = old_class.split_whitespace().collect();
                            classes.retain(|c| !c.starts_with("theme-") && !c.starts_with("font-"));
                            match settings.theme {
                                Theme::Light => classes.push("theme-light"),
                                Theme::Dark => classes.push("theme-dark"),
                                Theme::System => {} // 不移任何 theme class
                            }
                            match settings.font_size {
                                FontSize::Small => classes.push("font-small"),
                                FontSize::Medium => classes.push("font-medium"),
                                FontSize::Large => classes.push("font-large"),
                            }
                            el.set_class_name(&classes.join(" "));
                        }
                    }
                }
            }
            || ()
        });
    }

    // Toast 自动消失（3秒后）
    {
        let state = state.clone();
        use_effect_with(state.toast.clone(), move |toast| {
            if toast.is_some() {
                let state = state.clone();
                let handle = gloo_timers::callback::Timeout::new(3000, move || {
                    state.dispatch(AppAction::HideToast);
                });
                handle.forget();
            }
            || ()
        });
    }

    // 全局键盘快捷键
    {
        let state = state.clone();
        use_effect_with((), move |_| {
            let state = state.clone();
            let window = gloo_utils::window();
            let handler = gloo_events::EventListener::new(
                &window,
                "keydown",
                move |e: &Event| {
                    if let Some(ke) = e.dyn_ref::<KeyboardEvent>() {
                        let ctrl = ke.ctrl_key() || ke.meta_key();
                        match (ctrl, ke.shift_key(), ke.key().as_str()) {
                            (true, false, "n") => {
                                ke.prevent_default();
                                state.dispatch(AppAction::SetCurrentSession(None));
                                state.dispatch(AppAction::ClearMessages);
                            }
                            (true, true, "Delete") | (true, true, "Backspace") => {
                                ke.prevent_default();
                                state.dispatch(AppAction::ClearMessages);
                            }
                            (true, false, ",") => {
                                ke.prevent_default();
                                state.dispatch(AppAction::SetView(View::Settings));
                            }
                            _ => {
                                gloo_console::debug!("Keyboard event: unhandled key combination");
                            }
                        }
                    }
                },
            );
            handler.forget();
            || ()
        });
    }

    match *mode {
        AppMode::Loading => html! {
            <div class="loading-screen">
                <div class="loading-spinner" />
                <p>{ "正在加载..." }</p>
            </div>
        },
        AppMode::Wizard => html! {
            <ConfigWizard
                on_complete={on_wizard_complete}
                default_system_prompt={(*default_system_prompt).clone()}
            />
        },
        AppMode::Main => html! {
            <>
            { toast_overlay(&state) }
            <div class={classes!(
                "app",
                (!state.is_sidebar_open).then_some("sidebar-collapsed")
            )}>
                <Sidebar state={state.clone()} />

                <main class="main-content">
                    {
                        match state.current_view {
                            View::Chat => html! {
                                <ChatPanel state={state.clone()} />
                            },
                            View::Skills => html! {
                                <SkillsPanel state={state.clone()} />
                            },
                            View::Knowledge => html! {
                                <KnowledgePanel state={state.clone()} />
                            },
                            View::Settings => html! {
                                <SettingsPanel state={state.clone()} />
                            },
                        }
                    }
                </main>
            </div>
            </>
        },
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}

/// Toast 浮层组件
fn toast_overlay(state: &UseReducerHandle<AppState>) -> Html {
    match &state.toast {
        Some((message, toast_type)) => {
            let class = match toast_type {
                ToastType::Error => "toast toast-error",
                ToastType::Success => "toast toast-success",
                ToastType::Info => "toast toast-info",
            };
            let state = state.clone();
            let on_click = move |_| state.dispatch(AppAction::HideToast);
            html! {
                <div class={class} onclick={on_click}>
                    <span class="toast-message">{ message }</span>
                    <button class="toast-close">{ "✕" }</button>
                </div>
            }
        }
        None => html! {},
    }
}

/// 便捷函数：显示错误提示
pub fn show_error(state: &UseReducerHandle<AppState>, message: impl Into<String>) {
    state.dispatch(AppAction::ShowToast {
        message: message.into(),
        toast_type: ToastType::Error,
    });
}

/// 便捷函数：显示成功提示
pub fn show_success(state: &UseReducerHandle<AppState>, message: impl Into<String>) {
    state.dispatch(AppAction::ShowToast {
        message: message.into(),
        toast_type: ToastType::Success,
    });
}
