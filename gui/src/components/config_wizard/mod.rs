//! 配置向导组件
//!
//! 提供首次启动时的配置引导界面

pub mod api;
pub mod types;

use yew::prelude::*;

use types::*;

/// 配置向导组件属性
#[derive(Debug, Clone, PartialEq, Properties)]
pub struct ConfigWizardProps {
    /// 配置完成回调
    pub on_complete: Callback<()>,
    /// 核心默认系统提示词
    #[prop_or_default]
    pub default_system_prompt: String,
}

fn init_wizard_state(default_system_prompt: &str) -> WizardState {
    let mut state = WizardState::default();
    if !default_system_prompt.is_empty() {
        state.system_prompt = default_system_prompt.to_string();
    }
    state
}

/// 配置向导组件
#[function_component(ConfigWizard)]
pub fn config_wizard(props: &ConfigWizardProps) -> Html {
    let state = use_state(|| init_wizard_state(&props.default_system_prompt));
    let validation_errors = use_state(Vec::<String>::new);

    let on_next = {
        let state = state.clone();
        let validation_errors = validation_errors.clone();
        Callback::from(move |_| {
            let current_state = (*state).clone();
            match current_state.validate_step() {
                Ok(()) => {
                    validation_errors.set(Vec::new());
                    if let Some(next_step) = current_state.current_step.next() {
                        let mut new_state = current_state;
                        new_state.current_step = next_step;
                        state.set(new_state);
                    }
                }
                Err(errors) => {
                    validation_errors.set(errors);
                }
            }
        })
    };

    let on_previous = {
        let state = state.clone();
        Callback::from(move |_| {
            let current_state = (*state).clone();
            if let Some(prev_step) = current_state.current_step.previous() {
                let mut new_state = current_state;
                new_state.current_step = prev_step;
                state.set(new_state);
            }
        })
    };

    let on_complete_ref = props.on_complete.clone();
    let on_save = {
        let state = state.clone();
        Callback::from(move |_| {
            let current_state = (*state).clone();
            let request = current_state.to_save_request();

            // 设置保存中状态
            {
                let mut new_state = (*state).clone();
                new_state.is_saving = true;
                new_state.save_error = None;
                state.set(new_state);
            }

            let state_for_callback = state.clone();
            let on_complete = on_complete_ref.clone();
            api::save_config_async(request, move |result| {
                match result {
                    Ok(response) => {
                        if response.success {
                            on_complete.emit(());
                        } else {
                            // 保存失败，显示错误
                            let mut new_state = (*state_for_callback).clone();
                            new_state.is_saving = false;
                            new_state.save_error = Some(response.message);
                            state_for_callback.set(new_state);
                        }
                    }
                    Err(e) => {
                        let mut new_state = (*state_for_callback).clone();
                        new_state.is_saving = false;
                        new_state.save_error = Some(e);
                        state_for_callback.set(new_state);
                    }
                }
            });
        })
    };

    html! {
        <div class="config-wizard">
            <div class="wizard-header">
                <h1>{ "天演配置向导" }</h1>
                <div class="step-indicator">
                    {
                        (1..=WizardStep::total()).map(|i| {
                            let is_active = i == state.current_step.number();
                            let is_completed = i < state.current_step.number();
                            html! {
                                <div
                                    class={classes!(
                                        "step-dot",
                                        is_active.then_some("active"),
                                        is_completed.then_some("completed")
                                    )}
                                >
                                    { i }
                                </div>
                            }
                        }).collect::<Html>()
                    }
                </div>
                <h2>{ state.current_step.title() }</h2>
            </div>

            <div class="wizard-content">
                {
                    if !validation_errors.is_empty() {
                        html! {
                            <div class="validation-errors">
                                {
                                    validation_errors.iter().map(|error| {
                                        html! { <div class="error-item">{ error }</div> }
                                    }).collect::<Html>()
                                }
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }

                {
                    match state.current_step {
                        WizardStep::Welcome => html! {
                            <WelcomeStep />
                        },
                        WizardStep::ModelConfig => html! {
                            <ModelConfigStep state={state.clone()} />
                        },
                        WizardStep::DataConfig => html! {
                            <DataConfigStep state={state.clone()} />
                        },
                        WizardStep::AgentConfig => html! {
                            <AgentConfigStep state={state.clone()} />
                        },
                        WizardStep::SystemPrompt => html! {
                            <SystemPromptStep state={state.clone()} />
                        },
                        WizardStep::Confirm => html! {
                            <ConfirmStep state={state.clone()} />
                        },
                    }
                }
            </div>

            <div class="wizard-footer">
                {
                    if state.current_step != WizardStep::Welcome {
                        html! {
                            <button
                                class="btn btn-secondary"
                                onclick={on_previous}
                            >
                                { "上一步" }
                            </button>
                        }
                    } else {
                        html! { <div /> }
                    }
                }

                {
                    if state.current_step == WizardStep::Confirm {
                        html! {
                            <button
                                class="btn btn-primary"
                                onclick={on_save}
                                disabled={state.is_saving}
                            >
                                { if state.is_saving { "保存中..." } else { "保存并启动" } }
                            </button>
                        }
                    } else {
                        html! {
                            <button
                                class="btn btn-primary"
                                onclick={on_next}
                            >
                                { "下一步" }
                            </button>
                        }
                    }
                }
            </div>
        </div>
    }
}

/// 欢迎步骤
#[function_component(WelcomeStep)]
fn welcome_step() -> Html {
    html! {
        <div class="welcome-step">
            <div class="welcome-icon">
                <svg viewBox="0 0 24 24" width="64" height="64">
                    <path fill="currentColor" d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"/>
                </svg>
            </div>
            <h2>{ "欢迎使用天演" }</h2>
            <p>{ "天演是一个智能助手，可以帮助您回答各种问题、进行对话并提供有用的信息。" }</p>
            <p>{ "在首次使用之前，我们需要配置一些基本设置。请按照向导完成配置。" }</p>
            <div class="welcome-features">
                <div class="feature">
                    <span class="feature-icon">{ "🤖" }</span>
                    <span>{ "智能对话" }</span>
                </div>
                <div class="feature">
                    <span class="feature-icon">{ "🔍" }</span>
                    <span>{ "知识检索" }</span>
                </div>
                <div class="feature">
                    <span class="feature-icon">{ "💾" }</span>
                    <span>{ "记忆持久化" }</span>
                </div>
            </div>
        </div>
    }
}

/// 模型配置步骤属性
#[derive(Debug, Clone, PartialEq, Properties)]
struct ModelConfigStepProps {
    state: UseStateHandle<WizardState>,
}

/// 模型配置步骤
#[function_component(ModelConfigStep)]
fn model_config_step(props: &ModelConfigStepProps) -> Html {
    let state = props.state.clone();
    let service = match state.model_services.first() {
        Some(s) => s.clone(),
        None => {
            return html! {
                <div class="wizard-step">
                    <p>{ "请先添加至少一个模型服务" }</p>
                </div>
            };
        }
    };

    let on_name_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.model_services[0].name = input.value();
            state.set(new_state);
        })
    };

    let on_endpoint_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.model_services[0].endpoint = input.value();
            state.set(new_state);
        })
    };

    let on_api_key_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.model_services[0].api_key = input.value();
            state.set(new_state);
        })
    };

    let on_model_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.model_services[0].default_model = input.value();
            new_state.default_chat_model = input.value();
            state.set(new_state);
        })
    };

    html! {
        <div class="model-config-step">
            <div class="form-group">
                <label>{ "服务名称" }</label>
                <input
                    type="text"
                    value={service.name.clone()}
                    onchange={on_name_change}
                    placeholder="例如：百炼、OpenAI"
                />
            </div>

            <div class="form-group">
                <label>{ "API 端点 URL" }</label>
                <input
                    type="text"
                    value={service.endpoint.clone()}
                    onchange={on_endpoint_change}
                    placeholder="https://dashscope.aliyuncs.com/compatible-mode/v1"
                />
            </div>

            <div class="form-group">
                <label>{ "API 密钥" }</label>
                <input
                    type="password"
                    value={service.api_key.clone()}
                    onchange={on_api_key_change}
                    placeholder="sk-..."
                />
            </div>

            <div class="form-group">
                <label>{ "默认模型" }</label>
                <input
                    type="text"
                    value={service.default_model.clone()}
                    onchange={on_model_change}
                    placeholder="例如：qwen3.5-plus"
                />
            </div>
        </div>
    }
}

/// 数据配置步骤属性
#[derive(Debug, Clone, PartialEq, Properties)]
struct DataConfigStepProps {
    state: UseStateHandle<WizardState>,
}

/// 数据配置步骤
#[function_component(DataConfigStep)]
fn data_config_step(props: &DataConfigStepProps) -> Html {
    let state = props.state.clone();

    let on_data_dir_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.data_dir = input.value();
            state.set(new_state);
        })
    };

    let on_vector_url_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.vector_url = input.value();
            state.set(new_state);
        })
    };

    html! {
        <div class="data-config-step">
            <div class="form-group">
                <label>{ "数据存储目录" }</label>
                <input
                    type="text"
                    value={state.data_dir.clone()}
                    onchange={on_data_dir_change}
                    placeholder="C:\\Users\\...\\AppData\\Local\\tianyan"
                />
                <span class="help-text">{ "用于存储应用数据和向量索引" }</span>
            </div>

            <div class="form-group">
                <label>{ "向量数据库 URL" }</label>
                <input
                    type="text"
                    value={state.vector_url.clone()}
                    onchange={on_vector_url_change}
                    placeholder="http://localhost:6334"
                />
                <span class="help-text">{ "Qdrant 向量数据库服务地址" }</span>
            </div>
        </div>
    }
}

/// 智能体配置步骤属性
#[derive(Debug, Clone, PartialEq, Properties)]
struct AgentConfigStepProps {
    state: UseStateHandle<WizardState>,
}

/// 智能体配置步骤
#[function_component(AgentConfigStep)]
fn agent_config_step(props: &AgentConfigStepProps) -> Html {
    let state = props.state.clone();
    let show_advanced = use_state(|| false);

    let on_max_history_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            if let Ok(value) = input.value().parse() {
                let mut new_state = (*state).clone();
                new_state.max_history_messages = value;
                state.set(new_state);
            }
        })
    };

    let on_max_tokens_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            if let Ok(value) = input.value().parse() {
                let mut new_state = (*state).clone();
                new_state.max_context_tokens = value;
                state.set(new_state);
            }
        })
    };

    let on_retrieval_toggle = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.enable_retrieval = input.checked();
            state.set(new_state);
        })
    };

    let on_memory_toggle = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.enable_memory = input.checked();
            state.set(new_state);
        })
    };

    let on_streaming_toggle = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.stream_responses = input.checked();
            state.set(new_state);
        })
    };

    let on_thinking_toggle = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.enable_thinking = input.checked();
            state.set(new_state);
        })
    };

    html! {
        <div class="agent-config-step">
            <div class="form-group toggle-group">
                <label>
                    <input
                        type="checkbox"
                        checked={state.enable_retrieval}
                        onchange={on_retrieval_toggle}
                    />
                    { "启用检索增强" }
                </label>
                <span class="help-text">{ "允许智能体从知识库中检索相关信息" }</span>
            </div>

            <div class="form-group toggle-group">
                <label>
                    <input
                        type="checkbox"
                        checked={state.enable_memory}
                        onchange={on_memory_toggle}
                    />
                    { "启用记忆持久化" }
                </label>
                <span class="help-text">{ "保存对话历史以便后续使用" }</span>
            </div>

            <div class="form-group toggle-group">
                <label>
                    <input
                        type="checkbox"
                        checked={state.stream_responses}
                        onchange={on_streaming_toggle}
                    />
                    { "流式输出响应" }
                </label>
                <span class="help-text">{ "实时显示模型生成的内容" }</span>
            </div>

            <div class="form-group toggle-group">
                <label>
                    <input
                        type="checkbox"
                        checked={state.enable_thinking}
                        onchange={on_thinking_toggle}
                    />
                    { "启用思考模式" }
                </label>
                <span class="help-text">{ "模型会进行更深入的推理，但首字符响应会变慢" }</span>
            </div>

            <div class="advanced-toggle">
                <button
                    class="btn btn-text"
                    onclick={{
                        let show_advanced = show_advanced.clone();
                        Callback::from(move |_| show_advanced.set(!*show_advanced))
                    }}
                >
                    { if *show_advanced { "隐藏高级选项 ▲" } else { "显示高级选项 ▼" } }
                </button>
            </div>

            {
                if *show_advanced {
                    html! {
                        <div class="advanced-options">
                            <div class="form-group">
                                <label>{ "最大历史消息数" }</label>
                                <input
                                    type="number"
                                    value={state.max_history_messages.to_string()}
                                    onchange={on_max_history_change}
                                    min="1"
                                    max="100"
                                />
                            </div>

                            <div class="form-group">
                                <label>{ "最大上下文 Token 数" }</label>
                                <input
                                    type="number"
                                    value={state.max_context_tokens.to_string()}
                                    onchange={on_max_tokens_change}
                                    min="1000"
                                    max="32000"
                                    step="1000"
                                />
                            </div>
                        </div>
                    }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

/// 系统提示词步骤属性
#[derive(Debug, Clone, PartialEq, Properties)]
struct SystemPromptStepProps {
    state: UseStateHandle<WizardState>,
}

/// 系统提示词步骤
#[function_component(SystemPromptStep)]
fn system_prompt_step(props: &SystemPromptStepProps) -> Html {
    let state = props.state.clone();

    let on_prompt_change = {
        let state = state.clone();
        Callback::from(move |e: Event| {
            let textarea: web_sys::HtmlTextAreaElement = e.target_unchecked_into();
            let mut new_state = (*state).clone();
            new_state.system_prompt = textarea.value();
            state.set(new_state);
        })
    };

    html! {
        <div class="system-prompt-step">
            <div class="form-group">
                <label>{ "系统提示词" }</label>
                <textarea
                    value={state.system_prompt.clone()}
                    onchange={on_prompt_change}
                    rows="10"
                    placeholder="定义智能体的行为和角色..."
                />
                <span class="help-text">
                    { "系统提示词定义了智能体的行为和角色。您可以稍后修改此设置。" }
                </span>
            </div>
        </div>
    }
}

/// 确认步骤属性
#[derive(Debug, Clone, PartialEq, Properties)]
struct ConfirmStepProps {
    state: UseStateHandle<WizardState>,
}

/// 确认步骤
#[function_component(ConfirmStep)]
fn confirm_step(props: &ConfirmStepProps) -> Html {
    let state = props.state.clone();
    let service = match state.model_services.first() {
        Some(s) => s.clone(),
        None => {
            return html! {
                <div class="confirm-step">
                    <p>{ "未配置模型服务" }</p>
                </div>
            };
        }
    };

    html! {
        <div class="confirm-step">
            <h3>{ "配置摘要" }</h3>

            <div class="summary-section">
                <h4>{ "模型服务" }</h4>
                <div class="summary-item">
                    <span class="label">{ "服务名称:" }</span>
                    <span class="value">{ &service.name }</span>
                </div>
                <div class="summary-item">
                    <span class="label">{ "API 端点:" }</span>
                    <span class="value">{ &service.endpoint }</span>
                </div>
                <div class="summary-item">
                    <span class="label">{ "默认模型:" }</span>
                    <span class="value">{ &service.default_model }</span>
                </div>
            </div>

            <div class="summary-section">
                <h4>{ "数据存储" }</h4>
                <div class="summary-item">
                    <span class="label">{ "数据目录:" }</span>
                    <span class="value">{ &state.data_dir }</span>
                </div>
                <div class="summary-item">
                    <span class="label">{ "向量数据库:" }</span>
                    <span class="value">{ &state.vector_url }</span>
                </div>
            </div>

            <div class="summary-section">
                <h4>{ "智能体设置" }</h4>
                <div class="summary-item">
                    <span class="label">{ "检索增强:" }</span>
                    <span class="value">{ if state.enable_retrieval { "启用" } else { "禁用" } }</span>
                </div>
                <div class="summary-item">
                    <span class="label">{ "记忆持久化:" }</span>
                    <span class="value">{ if state.enable_memory { "启用" } else { "禁用" } }</span>
                </div>
                <div class="summary-item">
                    <span class="label">{ "流式输出:" }</span>
                    <span class="value">{ if state.stream_responses { "启用" } else { "禁用" } }</span>
                </div>
            </div>

            {
                if let Some(ref error) = state.save_error {
                    html! {
                        <div class="save-error">
                            { error }
                        </div>
                    }
                } else {
                    html! {}
                }
            }
        </div>
    }
}
