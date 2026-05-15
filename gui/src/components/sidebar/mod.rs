use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::sessions::{self, Session};
use crate::state::{AppAction, AppState, View};

#[derive(Properties, PartialEq)]
pub struct SidebarProps {
    pub state: UseReducerHandle<AppState>,
}

#[function_component(Sidebar)]
pub fn sidebar(props: &SidebarProps) -> Html {
    let state = props.state.clone();

    // Load sessions on mount
    {
        let state = state.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match sessions::list_sessions().await {
                    Ok(response) => {
                        state.dispatch(AppAction::SetSessions(response.sessions));
                    }
                    Err(err) => {
                        gloo_console::error!(format!("Failed to load sessions: {}", err));
                    }
                }
            });
            || ()
        });
    }

    let on_new_chat = {
        let state = state.clone();
        Callback::from(move |_| {
            // 生成新的 session_id
            let new_session_id = Uuid::new_v4().to_string();
            state.dispatch(AppAction::SetCurrentSession(Some(new_session_id)));
            state.dispatch(AppAction::ClearMessages);
            state.dispatch(AppAction::SetView(View::Chat));
        })
    };

    let on_select_session = {
        let state = state.clone();
        Callback::from(move |session_id: String| {
            let state = state.clone();
            spawn_local(async move {
                match sessions::get_session(&session_id).await {
                    Ok(detail) => {
                        state.dispatch(AppAction::SetCurrentSession(Some(session_id)));
                        state.dispatch(AppAction::SetMessages(detail.messages));
                        state.dispatch(AppAction::SetView(View::Chat));
                    }
                    Err(err) => {
                        gloo_console::error!(format!("Failed to load session: {}", err));
                    }
                }
            });
        })
    };

    let on_delete_session = {
        let state = state.clone();
        Callback::from(move |session_id: String| {
            let state = state.clone();
            spawn_local(async move {
                match sessions::delete_session(&session_id).await {
                    Ok(_) => {
                        state.dispatch(AppAction::RemoveSession(session_id));
                    }
                    Err(err) => {
                        gloo_console::error!(format!("Failed to delete session: {}", err));
                    }
                }
            });
        })
    };

    let on_toggle_sidebar = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::ToggleSidebar);
        })
    };

    let on_open_skills = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(View::Skills));
        })
    };

    let on_open_knowledge = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(View::Knowledge));
        })
    };

    let on_open_settings = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(View::Settings));
        })
    };

    html! {
        <aside class={classes!(
            "sidebar",
            (!state.is_sidebar_open).then_some("collapsed")
        )}>
            <div class="sidebar-header">
                <div class="sidebar-brand">
                    <span class="brand-icon">{"🔭"}</span>
                    { if state.is_sidebar_open {
                        html! { <span class="brand-text">{"天演"}</span> }
                    } else {
                        html! {}
                    }}
                </div>
                <button
                    class="btn btn-icon btn-toggle"
                    onclick={on_toggle_sidebar}
                    title={if state.is_sidebar_open { "收起侧边栏" } else { "展开侧边栏" }}
                >
                    { if state.is_sidebar_open {
                        html! {
                            <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                                <path d="M15.41 7.41L14 6l-6 6 6 6 1.41-1.41L10.83 12z"/>
                            </svg>
                        }
                    } else {
                        html! {
                            <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                                <path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/>
                            </svg>
                        }
                    }}
                </button>
            </div>

            <div class="sidebar-content">
                <button
                    class="btn btn-new-chat"
                    onclick={on_new_chat}
                    title="新建对话"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/>
                    </svg>
                    { if state.is_sidebar_open {
                        html! { <span>{"新建对话"}</span> }
                    } else {
                        html! {}
                    }}
                </button>

                <div class="sessions-list">
                    { if state.is_sidebar_open {
                        html! {
                            <div class="sessions-header">
                                <span class="sessions-title">{"历史会话"}</span>
                                <span class="sessions-count">{state.sessions.len()}</span>
                            </div>
                        }
                    } else {
                        html! {}
                    }}

                    <div class="sessions-items">
                        { for state.sessions.iter().map(|session| {
                            let is_active = state.current_session_id.as_ref() == Some(&session.id);
                            html! {
                                <SessionItem
                                    key={session.id.clone()}
                                    session={session.clone()}
                                    is_active={is_active}
                                    is_collapsed={!state.is_sidebar_open}
                                    on_select={on_select_session.clone()}
                                    on_delete={on_delete_session.clone()}
                                />
                            }
                        })}
                    </div>
                </div>
            </div>

            <div class="sidebar-footer">
                <button
                    class={classes!(
                        "btn",
                        "btn-skills",
                        (state.current_view == View::Skills).then_some("active")
                    )}
                    onclick={on_open_skills}
                    title="技能"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M22.7 19l-9.1-9.1c.9-2.3.4-5-1.5-6.9-2-2-5-2.4-7.4-1.3L9 6 6 9 1.6 4.7C.4 7.1.9 10.1 2.9 12.1c1.9 1.9 4.6 2.4 6.9 1.5l9.1 9.1c.4.4 1 .4 1.4 0l2.3-2.3c.5-.4.5-1.1.1-1.4z"/>
                    </svg>
                    { if state.is_sidebar_open {
                        html! { <span>{"技能"}</span> }
                    } else {
                        html! {}
                    }}
                </button>

                <button
                    class={classes!(
                        "btn",
                        "btn-knowledge",
                        (state.current_view == View::Knowledge).then_some("active")
                    )}
                    onclick={on_open_knowledge}
                    title="知识库"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M4 6H2v14c0 1.1.9 2 2 2h14v-2H4V6zm16-4H8c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H8V4h12v12zM10 9h8v2h-8zm0 3h8v2h-8zm0 3h5v2h-5z"/>
                    </svg>
                    { if state.is_sidebar_open {
                        html! { <span>{"知识库"}</span> }
                    } else {
                        html! {}
                    }}
                </button>

                <button
                    class={classes!(
                        "btn",
                        "btn-settings",
                        (state.current_view == View::Settings).then_some("active")
                    )}
                    onclick={on_open_settings}
                    title="设置"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58c.18-.14.23-.41.12-.61l-1.92-3.32c-.12-.22-.37-.29-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54c-.04-.24-.24-.41-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L3.16 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58c-.18.14-.23.41-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.58 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z"/>
                    </svg>
                    { if state.is_sidebar_open {
                        html! { <span>{"设置"}</span> }
                    } else {
                        html! {}
                    }}
                </button>
            </div>
        </aside>
    }
}

#[derive(Properties, PartialEq)]
struct SessionItemProps {
    session: Session,
    is_active: bool,
    is_collapsed: bool,
    on_select: Callback<String>,
    on_delete: Callback<String>,
}

#[function_component(SessionItem)]
fn session_item(props: &SessionItemProps) -> Html {
    let is_hovered = use_state(|| false);

    let on_mouse_enter = {
        let is_hovered = is_hovered.clone();
        Callback::from(move |_| {
            is_hovered.set(true);
        })
    };

    let on_mouse_leave = {
        let is_hovered = is_hovered.clone();
        Callback::from(move |_| {
            is_hovered.set(false);
        })
    };

    let on_click = {
        let session_id = props.session.id.clone();
        let on_select = props.on_select.clone();
        Callback::from(move |_| {
            on_select.emit(session_id.clone());
        })
    };

    let on_delete_click = {
        let session_id = props.session.id.clone();
        let on_delete = props.on_delete.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            on_delete.emit(session_id.clone());
        })
    };

    html! {
        <div
            class={classes!(
                "session-item",
                props.is_active.then_some("active")
            )}
            onmouseenter={on_mouse_enter}
            onmouseleave={on_mouse_leave}
            onclick={on_click}
            title={props.session.title.clone()}
        >
            <div class="session-icon">
                <svg viewBox="0 0 24 24" fill="currentColor" width="18" height="18">
                    <path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z"/>
                </svg>
            </div>

            { if !props.is_collapsed {
                html! {
                    <>
                        <div class="session-info">
                            <div class="session-title">{&props.session.title}</div>
                            <div class="session-meta">
                                { crate::utils::format_date(&props.session.updated_at) }
                                {" · "}
                                {props.session.message_count}
                                {" 条消息"}
                            </div>
                        </div>

                        { if *is_hovered || props.is_active {
                            html! {
                                <button
                                    class="btn btn-icon btn-delete"
                                    onclick={on_delete_click}
                                    title="删除会话"
                                >
                                    <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
                                        <path d="M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z"/>
                                    </svg>
                                </button>
                            }
                        } else {
                            html! {}
                        }}
                    </>
                }
            } else {
                html! {}
            }}
        </div>
    }
}
