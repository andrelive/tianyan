mod api;
mod components;
mod state;
mod utils;

use uuid::Uuid;
use yew::prelude::*;

use components::config_wizard::api::fetch_config_status_async;
use components::{ChatPanel, ConfigWizard, KnowledgePanel, SettingsPanel, Sidebar, SkillsPanel};
use state::{AppAction, AppState, View};

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
        },
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
