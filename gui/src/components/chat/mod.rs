use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlTextAreaElement;
use yew::prelude::*;

use crate::api::chat::{
    ChatMessage, ChatRequest, EditMessageRequest, MessageRole, RegenerateRequest, StreamChunkType,
    StreamHandler,
};
use crate::api::sessions;
use crate::state::{AppAction, AppState, StreamStatus};

const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_MAX_TOKENS: u32 = 2048;
const SCROLL_THRESHOLD: i32 = 200;

#[derive(Properties, PartialEq)]
pub struct ChatPanelProps {
    pub state: UseReducerHandle<AppState>,
}

#[function_component(ChatPanel)]
pub fn chat_panel(props: &ChatPanelProps) -> Html {
    let state = props.state.clone();
    let input_value = use_state(String::new);
    let textarea_ref = use_node_ref();
    let messages_end_ref = use_node_ref();
    let stream_handler = use_mut_ref(StreamHandler::new);

    // Auto-scroll to bottom when messages change
    {
        let messages_end_ref = messages_end_ref.clone();
        use_effect_with(state.messages.clone(), move |_| {
            if let Some(element) = messages_end_ref.cast::<web_sys::Element>() {
                element.scroll_into_view_with_bool(true);
            }
            || ()
        });
    }

    // Auto-resize textarea using gloo
    let on_input = {
        let input_value = input_value.clone();
        let textarea_ref = textarea_ref.clone();
        Callback::from(move |e: InputEvent| {
            e.prevent_default();
            if let Some(textarea) = textarea_ref.cast::<HtmlTextAreaElement>() {
                let value = textarea.value();
                input_value.set(value.clone());

                // Auto-resize using DOM API
                let _ = textarea.set_attribute("style", "height: auto;");
                let scroll_height = textarea.scroll_height();
                let new_height = scroll_height.min(SCROLL_THRESHOLD);
                let _ = textarea.set_attribute("style", &format!("height: {}px;", new_height));
            }
        })
    };

    let on_key_down = {
        let input_value = input_value.clone();
        let state = state.clone();
        let stream_handler = stream_handler.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" && !e.shift_key() {
                e.prevent_default();
                let value = (*input_value).clone();
                if !value.trim().is_empty() && matches!(state.stream_status, StreamStatus::Idle) {
                    send_message(value, state.clone(), stream_handler.clone());
                    input_value.set(String::new());
                }
            }
        })
    };

    let on_send_click = {
        let input_value = input_value.clone();
        let state = state.clone();
        let stream_handler = stream_handler.clone();
        Callback::from(move |_| {
            let value = (*input_value).clone();
            if !value.trim().is_empty() && matches!(state.stream_status, StreamStatus::Idle) {
                send_message(value, state.clone(), stream_handler.clone());
                input_value.set(String::new());
            }
        })
    };

    let on_stop_click = {
        let state = state.clone();
        let stream_handler = stream_handler.clone();
        Callback::from(move |_| {
            match stream_handler.try_borrow_mut() {
                Ok(mut handler) => handler.stop(),
                Err(_) => gloo_console::error!("stream_handler 已被借用，无法停止"),
            }
            state.dispatch(AppAction::SetStreamStatus(StreamStatus::Idle));
        })
    };

    let is_streaming = matches!(state.stream_status, StreamStatus::Streaming);
    let is_input_disabled = is_streaming;

    html! {
        <div class="chat-panel">
            <div class="chat-header">
                <h2 class="chat-title">
                    { state.current_session_id.as_ref()
                        .and_then(|id| state.sessions.iter().find(|s| &s.id == id))
                        .map(|s| s.title.clone())
                        .unwrap_or_else(|| "新对话".to_string()) }
                </h2>
            </div>

            <div class="messages-container">
                { if state.messages.is_empty() {
                    html! {
                        <div class="empty-state">
                            <div class="empty-icon">{"🔭"}</div>
                            <h3>{"开始与天演对话"}</h3>
                            <p>{"输入您的问题，我将为您提供帮助"}</p>
                        </div>
                    }
                } else {
                    html! {
                        <>
                            { for state.messages.iter().enumerate().map(|(idx, msg)| {
                                let is_last = idx == state.messages.len() - 1;
                                let is_streaming_msg = is_last && is_streaming && matches!(msg.role, MessageRole::Assistant);
                                html! {
                                    <MessageBubble
                                        key={idx}
                                        message={msg.clone()}
                                        is_streaming={is_streaming_msg}
                                        index={idx}
                                        state={state.clone()}
                                    />
                                }
                            })}
                            <div ref={messages_end_ref} />
                        </>
                    }
                }}
            </div>

            <div class="input-container">
                { if let StreamStatus::Error(ref err) = state.stream_status {
                    html! {
                        <div class="error-banner">
                            {err}
                        </div>
                    }
                } else {
                    html! {}
                }}

                <div class="input-wrapper">
                    <textarea
                        ref={textarea_ref}
                        class="chat-input"
                        placeholder={if is_streaming { "等待响应..." } else { "输入消息，按 Enter 发送，Shift+Enter 换行" }}
                        value={(*input_value).clone()}
                        oninput={on_input}
                        onkeydown={on_key_down}
                        disabled={is_input_disabled}
                        rows="1"
                    />

                    <div class="input-actions">
                        { if is_streaming {
                            html! {
                                <button
                                    class="btn btn-stop"
                                    onclick={on_stop_click}
                                    title="停止生成"
                                >
                                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                                        <rect x="6" y="6" width="12" height="12" rx="2" />
                                    </svg>
                                </button>
                            }
                        } else {
                            html! {
                                <button
                                    class={classes!("btn", "btn-send", (!input_value.trim().is_empty()).then_some("active"))}
                                    onclick={on_send_click}
                                    disabled={input_value.trim().is_empty()}
                                    title="发送消息"
                                >
                                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                                        <path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/>
                                    </svg>
                                </button>
                            }
                        }}
                    </div>
                </div>

                <div class="input-footer">
                    <span class="input-hint">
                        {if is_streaming { "正在生成..." } else { "Enter 发送 · Shift+Enter 换行" }}
                    </span>
                </div>
            </div>
        </div>
    }
}

fn send_message(
    content: String,
    state: UseReducerHandle<AppState>,
    stream_handler: Rc<RefCell<StreamHandler>>,
) {
    // Add user message
    let user_msg = ChatMessage::user(content.clone());
    state.dispatch(AppAction::AddMessage(user_msg));

    // Add placeholder for assistant response
    let assistant_msg = ChatMessage::assistant("");
    state.dispatch(AppAction::AddMessage(assistant_msg));

    // Set streaming status
    state.dispatch(AppAction::SetStreamStatus(StreamStatus::Streaming));

    // Prepare request - 包含历史消息和新消息
    // 不过滤空内容消息，避免索引错位导致 RegenerateFrom 指向错误消息
    let mut messages: Vec<ChatMessage> = state.messages.iter().cloned().collect();
    // 添加当前用户消息（因为dispatch是异步的，state中还没有这条消息）
    messages.push(ChatMessage::user(content.clone()));

    let request = ChatRequest {
        session_id: state.current_session_id.clone(),
        messages,
        stream: true,
        temperature: DEFAULT_TEMPERATURE,
        max_tokens: DEFAULT_MAX_TOKENS,
    };

    let state_for_message = state.clone();
    let state_for_error = state.clone();
    let state_for_complete = state.clone();

    spawn_local(async move {
        let mut handler = match stream_handler.try_borrow_mut() {
            Ok(h) => h,
            Err(_) => {
                gloo_console::error!("stream_handler 已被借用，无法启动流");
                return;
            }
        };

        handler.start(
            request,
            move |event| {
                match event.chunk_type {
                    StreamChunkType::Thought => {
                        // 思考过程：作为独立的过程消息追加
                        let thought_msg = ChatMessage::assistant(event.delta)
                            .with_chunk_type(StreamChunkType::Thought);
                        state_for_message.dispatch(AppAction::AddMessage(thought_msg));
                    }
                    StreamChunkType::ToolCall => {
                        // 工具调用：作为独立的过程消息追加
                        let tool_msg = ChatMessage::assistant(event.delta)
                            .with_chunk_type(StreamChunkType::ToolCall);
                        state_for_message.dispatch(AppAction::AddMessage(tool_msg));
                    }
                    StreamChunkType::Observation => {
                        // 观察结果：作为独立的过程消息追加
                        let obs_msg = ChatMessage::assistant(event.delta)
                            .with_chunk_type(StreamChunkType::Observation);
                        state_for_message.dispatch(AppAction::AddMessage(obs_msg));
                    }
                    StreamChunkType::Answer | StreamChunkType::Clarification => {
                        // 最终回答：追加到最后一条助手消息
                        state_for_message.dispatch(AppAction::UpdateLastMessage(event.delta));
                    }
                    StreamChunkType::Error => {
                        // 错误：追加到最后一条助手消息
                        state_for_message.dispatch(AppAction::UpdateLastMessage(event.delta));
                    }
                }
                if let Some(calls) = event.skill_calls {
                    state_for_message.dispatch(AppAction::AppendSkillCalls(calls));
                }
            },
            Some(Box::new(move |err| {
                state_for_error.dispatch(AppAction::SetStreamStatus(StreamStatus::Error(err)));
            })),
            Some(Box::new(move || {
                state_for_complete.dispatch(AppAction::SetStreamStatus(StreamStatus::Idle));
                let state_for_refresh = state_for_complete.clone();
                spawn_local(async move {
                    match sessions::list_sessions().await {
                        Ok(response) => {
                            state_for_refresh.dispatch(AppAction::SetSessions(response.sessions));
                        }
                        Err(err) => {
                            gloo_console::error!(format!("刷新会话列表失败: {}", err));
                        }
                    }
                });
            })),
        );
    });
}

/// 启动流式对话，将通用 StreamHandler 编排为共享辅助函数
fn start_stream_for_regenerate(request: ChatRequest, state: UseReducerHandle<AppState>) {
    state.dispatch(AppAction::AddMessage(ChatMessage::assistant("")));
    state.dispatch(AppAction::SetStreamStatus(StreamStatus::Streaming));

    let state_for_message = state.clone();
    let state_for_error = state.clone();
    let state_for_complete = state.clone();

    spawn_local(async move {
        let mut handler = StreamHandler::new();
        handler.start(
            request,
            move |event| {
                state_for_message.dispatch(AppAction::UpdateLastMessage(event.delta));
                if let Some(calls) = event.skill_calls {
                    state_for_message.dispatch(AppAction::AppendSkillCalls(calls));
                }
            },
            Some(Box::new(move |err| {
                state_for_error.dispatch(AppAction::SetStreamStatus(StreamStatus::Error(err)));
            })),
            Some(Box::new(move || {
                state_for_complete.dispatch(AppAction::SetStreamStatus(StreamStatus::Idle));
            })),
        );
    });
}

#[derive(Properties, PartialEq)]
struct MessageBubbleProps {
    message: ChatMessage,
    is_streaming: bool,
    index: usize,
    state: UseReducerHandle<AppState>,
}

#[function_component(MessageBubble)]
fn message_bubble(props: &MessageBubbleProps) -> Html {
    let is_user = matches!(props.message.role, MessageRole::User);
    let is_assistant = matches!(props.message.role, MessageRole::Assistant);
    let is_editing = use_state(|| false);
    let edit_value = use_state(|| props.message.content.clone());

    let on_regenerate = {
        let state = props.state.clone();
        let index = props.index;
        let is_user = is_user;
        Callback::from(move |_| {
            if !is_user {
                return;
            }
            let state_clone = state.clone();
            state.dispatch(AppAction::RegenerateFrom(index));

            let messages: Vec<ChatMessage> = state_clone
                .messages
                .iter()
                .take(index + 1)
                .cloned()
                .collect();
            let session_id = state_clone.current_session_id.clone();

            // 先调用后端 regenerate 端点更新服务端状态
            if let Some(ref sid) = session_id {
                let sid = sid.clone();
                spawn_local(async move {
                    let _ = crate::api::chat::regenerate_message(RegenerateRequest {
                        session_id: sid,
                        message_index: index,
                    })
                    .await;
                });
            }

            spawn_local(async move {
                let request = ChatRequest {
                    session_id: session_id.clone(),
                    messages: messages.clone(),
                    stream: true,
                    temperature: DEFAULT_TEMPERATURE,
                    max_tokens: DEFAULT_MAX_TOKENS,
                };
                start_stream_for_regenerate(request, state_clone);
            });
        })
    };

    let on_copy = {
        let content = props.message.content.clone();
        Callback::from(move |_| {
            let window = web_sys::window();
            if let Some(win) = window {
                let clipboard = win.navigator().clipboard();
                let _ = clipboard.write_text(&content);
            }
        })
    };

    let on_edit_start = {
        let is_editing = is_editing.clone();
        let edit_value = edit_value.clone();
        let content = props.message.content.clone();
        Callback::from(move |_| {
            edit_value.set(content.clone());
            is_editing.set(true);
        })
    };

    let on_edit_cancel = {
        let is_editing = is_editing.clone();
        Callback::from(move |_| {
            is_editing.set(false);
        })
    };

    let on_edit_save = {
        let state = props.state.clone();
        let index = props.index;
        let is_editing = is_editing.clone();
        let edit_value = edit_value.clone();
        Callback::from(move |_| {
            let new_content = (*edit_value).clone();
            is_editing.set(false);

            state.dispatch(AppAction::EditMessage {
                index,
                new_content: new_content.clone(),
            });
            state.dispatch(AppAction::DeleteMessagesFrom(index + 1));

            let state_clone = state.clone();
            let session_id = state_clone.current_session_id.clone();
            let messages: Vec<ChatMessage> = state_clone
                .messages
                .iter()
                .take(index + 1)
                .cloned()
                .collect();

            // 调用后端 edit_message 端点更新服务端状态
            if let Some(ref sid) = session_id {
                let sid = sid.clone();
                let content = new_content;
                spawn_local(async move {
                    let _ = crate::api::chat::edit_message(EditMessageRequest {
                        session_id: sid,
                        message_index: index,
                        new_content: content,
                    })
                    .await;
                });
            }

            spawn_local(async move {
                let request = ChatRequest {
                    session_id: session_id.clone(),
                    messages: messages.clone(),
                    stream: true,
                    temperature: DEFAULT_TEMPERATURE,
                    max_tokens: DEFAULT_MAX_TOKENS,
                };

                start_stream_for_regenerate(request, state_clone);
            });
        })
    };

    let on_edit_input = {
        let edit_value = edit_value.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<HtmlTextAreaElement>() {
                    edit_value.set(input.value());
                }
            }
        })
    };

    // 判断是否为流式过程消息
    let is_stream_message = props.message.chunk_type.is_some();
    let stream_type_class = match props.message.chunk_type {
        Some(StreamChunkType::Thought) => Some("stream-thought"),
        Some(StreamChunkType::ToolCall) => Some("stream-tool-call"),
        Some(StreamChunkType::Observation) => Some("stream-observation"),
        _ => None,
    };

    html! {
        <div class={classes!(
            "message-bubble",
            is_user.then_some("user"),
            is_assistant.then_some("assistant"),
            is_stream_message.then_some("stream-message"),
            stream_type_class,
        )}>
            <div class="message-avatar">
                { if is_user {
                    "👤"
                } else if is_stream_message {
                    match props.message.chunk_type {
                        Some(StreamChunkType::Thought) => "💭",
                        Some(StreamChunkType::ToolCall) => "🔧",
                        Some(StreamChunkType::Observation) => "👁️",
                        _ => "🔭",
                    }
                } else {
                    "🔭"
                }}
            </div>
            <div class="message-content">
                <div class="message-header">
                    <span class="message-author">
                        { if is_user {
                            "你"
                        } else if is_stream_message {
                            match props.message.chunk_type {
                                Some(StreamChunkType::Thought) => "思考中",
                                Some(StreamChunkType::ToolCall) => "工具调用",
                                Some(StreamChunkType::Observation) => "观察结果",
                                _ => "天演",
                            }
                        } else {
                            "天演"
                        }}
                    </span>
                    { if let Some(ref timestamp) = props.message.timestamp {
                        html! {
                            <span class="message-time">
                                { crate::utils::format_datetime(timestamp) }
                            </span>
                        }
                    } else {
                        html! {}
                    }}
                </div>

                { if *is_editing {
                    html! {
                        <div class="message-edit-area">
                            <textarea
                                class="message-edit-input"
                                value={(*edit_value).clone()}
                                oninput={on_edit_input}
                                rows="3"
                            />
                            <div class="message-edit-actions">
                                <button class="btn-edit-cancel" onclick={on_edit_cancel}>{"取消"}</button>
                                <button class="btn-edit-save" onclick={on_edit_save}>{"保存并重新生成"}</button>
                            </div>
                        </div>
                    }
                } else {
                    html! {
                        <div class="message-text markdown-content">
                            { if props.message.content.is_empty() && props.is_streaming && !is_stream_message {
                                html! { <span class="typing-indicator">{"●●●"}</span> }
                            } else {
                                html! { format_message(&props.message.content) }
                            }}
                        </div>
                    }
                }}

                { if let Some(ref skill_calls) = props.message.skill_calls {
                    html! {
                        <div class="skill-calls">
                            { for skill_calls.iter().map(|call| {
                                let status_class = if call.success { "skill-success" } else { "skill-error" };
                                let status_icon = if call.success { "✓" } else { "✗" };
                                html! {
                                    <div class={classes!("skill-call-card", status_class)} key={call.skill_id.clone()}>
                                        <div class="skill-call-header">
                                            <span class="skill-call-icon">{"⚡"}</span>
                                            <span class="skill-call-name">{ &call.skill_name }</span>
                                            <span class="skill-call-status">{ status_icon }</span>
                                        </div>
                                        <div class="skill-call-meta">
                                            <span class="skill-call-id">{ &call.skill_id }</span>
                                            <span class="skill-call-time">{ format!("{}ms", call.execution_time_ms) }</span>
                                        </div>
                                        { if let Some(ref error) = call.error {
                                            html! {
                                                <div class="skill-call-error">{ error }</div>
                                            }
                                        } else {
                                            html! {}
                                        }}
                                    </div>
                                }
                            })}
                        </div>
                    }
                } else {
                    html! {}
                }}

                { if !props.is_streaming && !*is_editing {
                    html! {
                        <div class="message-actions">
                            { if is_user {
                                html! {
                                    <>
                                        <button class="msg-action-btn" onclick={on_edit_start} title="编辑">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" width="14" height="14">
                                                <path d="M11 4H4a2 2 0 00-2 2v14a2 2 0 002 2h14a2 2 0 002-2v-7"/>
                                                <path d="M18.5 2.5a2.121 2.121 0 013 3L12 15l-4 1 1-4 9.5-9.5z"/>
                                            </svg>
                                        </button>
                                        <button class="msg-action-btn" onclick={on_regenerate} title="重新生成">
                                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" width="14" height="14">
                                                <path d="M23 4v6h-6M1 20v-6h6"/>
                                                <path d="M3.51 9a9 9 0 0114.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0020.49 15"/>
                                            </svg>
                                        </button>
                                    </>
                                }
                            } else {
                                html! {
                                    <button class="msg-action-btn" onclick={on_copy} title="复制">
                                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" width="14" height="14">
                                            <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                                            <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1"/>
                                        </svg>
                                    </button>
                                }
                            }}
                        </div>
                    }
                } else {
                    html! {}
                }}
            </div>
        </div>
    }
}

fn format_message(content: &str) -> Html {
    use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(content, options);

    // Wrap code blocks with language class for syntax highlighting
    let processed: Vec<Event> = parser
        .map(|event| match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                let lang_str = lang.to_string();
                if lang_str.is_empty() {
                    Event::Html(CowStr::Boxed("<pre><code>".into()))
                } else {
                    Event::Html(CowStr::Boxed(
                        format!("<pre><code class=\"language-{}\">", lang_str).into(),
                    ))
                }
            }
            Event::End(Tag::CodeBlock(_)) => {
                Event::Html(CowStr::Boxed("</code></pre>".into()))
            }
            _ => event,
        })
        .collect();

    let mut body = String::new();
    html::push_html(&mut body, processed.into_iter());

    // 将生成的 HTML 转为 Yew VNode
    let div = gloo_utils::document()
        .create_element("div")
        .expect("DOM createElement('div') should be available in all browsers");
    div.set_inner_html(&body);
    let node = yew::virtual_dom::VNode::VRef(div.into());
    node
}
