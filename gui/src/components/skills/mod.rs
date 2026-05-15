use std::collections::HashMap;

use serde_json::Value;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api::skills::{self, ExecuteSkillRequest, Skill, SkillExecutionStatus};
use crate::state::{AppAction, AppState, View};

#[derive(Properties, PartialEq)]
pub struct SkillsPanelProps {
    pub state: UseReducerHandle<AppState>,
}

#[function_component(SkillsPanel)]
pub fn skills_panel(props: &SkillsPanelProps) -> Html {
    let state = props.state.clone();
    let selected_skill = use_state(|| None::<Skill>);
    let param_values = use_state(HashMap::<String, String>::new);
    let execution_result = use_state(|| None::<SkillExecutionStatus>);
    let is_executing = use_state(|| false);

    // Load skills on mount
    {
        let state = state.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match skills::list_skills().await {
                    Ok(response) => {
                        state.dispatch(AppAction::SetSkills(response.skills));
                    }
                    Err(err) => {
                        gloo_console::error!(format!("Failed to load skills: {}", err));
                    }
                }
            });
            || ()
        });
    }

    let on_select_skill = {
        let selected_skill = selected_skill.clone();
        let param_values = param_values.clone();
        let execution_result = execution_result.clone();
        Callback::from(move |skill: Skill| {
            let mut defaults = HashMap::new();
            if let Some(params) = &skill.parameters {
                for param in params {
                    if let Some(default) = &param.default_value {
                        if let Some(s) = default.as_str() {
                            defaults.insert(param.name.clone(), s.to_string());
                        } else {
                            defaults.insert(param.name.clone(), default.to_string());
                        }
                    }
                }
            }
            param_values.set(defaults);
            execution_result.set(None);
            selected_skill.set(Some(skill));
        })
    };

    let on_back = {
        let selected_skill = selected_skill.clone();
        Callback::from(move |_| {
            selected_skill.set(None);
        })
    };

    let on_param_change = {
        let param_values = param_values.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                let name = input.id();
                let value = input.value();
                let mut new_values = (*param_values).clone();
                new_values.insert(name, value);
                param_values.set(new_values);
            }
        })
    };

    let on_execute = {
        let selected_skill = selected_skill.clone();
        let param_values = param_values.clone();
        let execution_result = execution_result.clone();
        let is_executing = is_executing.clone();
        Callback::from(move |_| {
            if let Some(skill) = &*selected_skill {
                let skill_id = skill.id.clone();
                let params = if param_values.is_empty() {
                    None
                } else {
                    let map: serde_json::Map<String, Value> = param_values
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect();
                    Some(Value::Object(map))
                };
                let request = ExecuteSkillRequest {
                    parameters: params,
                    context: None,
                };
                is_executing.set(true);
                execution_result.set(None);
                let execution_result = execution_result.clone();
                let is_executing = is_executing.clone();
                spawn_local(async move {
                    match skills::execute_skill(&skill_id, &request).await {
                        Ok(response) => {
                            if response.success {
                                // Poll for status
                                let job_id = response.job_id.clone();
                                let skill_id = response.skill_id.clone();
                                let execution_result = execution_result.clone();
                                let is_executing = is_executing.clone();
                                spawn_local(async move {
                                    // Simple polling: wait a bit then fetch status
                                    gloo_timers::future::TimeoutFuture::new(500).await;
                                    match skills::get_skill_status(&skill_id, &job_id).await {
                                        Ok(status) => {
                                            execution_result.set(Some(status));
                                        }
                                        Err(err) => {
                                            gloo_console::error!(format!(
                                                "Failed to get skill status: {}",
                                                err
                                            ));
                                        }
                                    }
                                    is_executing.set(false);
                                });
                            } else {
                                execution_result.set(Some(SkillExecutionStatus {
                                    job_id: response.job_id,
                                    skill_id: response.skill_id,
                                    status: "error".to_string(),
                                    progress: 0.0,
                                    result: None,
                                    error: response.error,
                                }));
                                is_executing.set(false);
                            }
                        }
                        Err(err) => {
                            gloo_console::error!(format!("Failed to execute skill: {}", err));
                            is_executing.set(false);
                        }
                    }
                });
            }
        })
    };

    let on_go_chat = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(View::Chat));
        })
    };

    html! {
        <div class="skills-panel">
            <div class="skills-header">
                <h2>{"技能中心"}</h2>
                <button
                    class="btn btn-icon btn-back"
                    onclick={on_go_chat}
                    title="返回对话"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
                    </svg>
                </button>
            </div>

            <div class="skills-content">
                {
                    if let Some(skill) = &*selected_skill {
                        html! {
                            <SkillDetail
                                skill={skill.clone()}
                                param_values={(*param_values).clone()}
                                execution_result={(*execution_result).clone()}
                                is_executing={*is_executing}
                                on_back={on_back.clone()}
                                on_param_change={on_param_change.clone()}
                                on_execute={on_execute.clone()}
                            />
                        }
                    } else {
                        html! {
                            <SkillList
                                skills={state.skills.clone()}
                                on_select={on_select_skill.clone()}
                            />
                        }
                    }
                }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct SkillListProps {
    skills: Vec<Skill>,
    on_select: Callback<Skill>,
}

#[function_component(SkillList)]
fn skill_list(props: &SkillListProps) -> Html {
    let categories = {
        let mut cats: Vec<String> = props
            .skills
            .iter()
            .map(|s| s.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cats.sort();
        cats
    };

    html! {
        <div class="skill-list">
            <div class="skill-list-header">
                <p class="skill-list-desc">{"选择技能执行特定任务"}</p>
            </div>

            {
                if props.skills.is_empty() {
                    html! {
                        <div class="empty-state">
                            <div class="empty-icon">{"🛠️"}</div>
                            <h3>{"暂无可用技能"}</h3>
                            <p>{"技能将在后续版本中添加"}</p>
                        </div>
                    }
                } else {
                    html! {
                        <>
                            {
                                categories.into_iter().map(|category| {
                                    let category_skills: Vec<Skill> = props.skills
                                        .iter()
                                        .filter(|s| s.category == category)
                                        .cloned()
                                        .collect();
                                    html! {
                                        <div class="skill-category" key={category.clone()}>
                                            <h3 class="skill-category-title">
                                                {format_category(&category)}
                                            </h3>
                                            <div class="skill-cards">
                                                {
                                                    for category_skills.iter().map(|skill| {
                                                        let on_select = props.on_select.clone();
                                                        let skill_clone = skill.clone();
                                                        let onclick = Callback::from(move |_| {
                                                            on_select.emit(skill_clone.clone());
                                                        });
                                                        html! {
                                                            <SkillCard
                                                                key={skill.id.clone()}
                                                                skill={skill.clone()}
                                                                onclick={onclick}
                                                            />
                                                        }
                                                    })
                                                }
                                            </div>
                                        </div>
                                    }
                                }).collect::<Html>()
                            }
                        </>
                    }
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct SkillCardProps {
    skill: Skill,
    onclick: Callback<MouseEvent>,
}

#[function_component(SkillCard)]
fn skill_card(props: &SkillCardProps) -> Html {
    let icon = match props.skill.icon.as_deref() {
        Some("search") => "🔍",
        Some("code") => "💻",
        Some("file") => "📄",
        Some("database") => "🗄️",
        _ => "🛠️",
    };

    html! {
        <div
            class="skill-card"
            onclick={props.onclick.clone()}
        >
            <div class="skill-card-icon">{icon}</div>
            <div class="skill-card-info">
                <div class="skill-card-name">{&props.skill.name}</div>
                <div class="skill-card-desc">{&props.skill.description}</div>
                <div class="skill-card-meta">
                    <span class="skill-version">{format!("v{}", &props.skill.version)}</span>
                    {
                        if props.skill.enabled {
                            html! { <span class="skill-badge enabled">{"可用"}</span> }
                        } else {
                            html! { <span class="skill-badge disabled">{"禁用"}</span> }
                        }
                    }
                </div>
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct SkillDetailProps {
    skill: Skill,
    param_values: HashMap<String, String>,
    execution_result: Option<SkillExecutionStatus>,
    is_executing: bool,
    on_back: Callback<MouseEvent>,
    on_param_change: Callback<InputEvent>,
    on_execute: Callback<MouseEvent>,
}

#[function_component(SkillDetail)]
fn skill_detail(props: &SkillDetailProps) -> Html {
    html! {
        <div class="skill-detail">
            <div class="skill-detail-header">
                <button
                    class="btn btn-icon"
                    onclick={props.on_back.clone()}
                    title="返回列表"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
                    </svg>
                </button>
                <div class="skill-detail-title">
                    <h3>{&props.skill.name}</h3>
                    <p>{&props.skill.description}</p>
                </div>
            </div>

            {
                if let Some(params) = &props.skill.parameters {
                    html! {
                        <div class="skill-params">
                            <h4>{"参数"}</h4>
                            {
                                for params.iter().map(|param| {
                                    let value = props.param_values.get(&param.name).cloned().unwrap_or_default();
                                    html! {
                                        <div class="skill-param" key={param.name.clone()}>
                                            <label for={param.name.clone()}>
                                                {&param.name}
                                                {
                                                    if param.required {
                                                        html! { <span class="required">{" *"}</span> }
                                                    } else {
                                                        html! {}
                                                    }
                                                }
                                            </label>
                                            <input
                                                id={param.name.clone()}
                                                type="text"
                                                value={value}
                                                placeholder={param.description.clone()}
                                                oninput={props.on_param_change.clone()}
                                            />
                                            <span class="param-desc">{&param.description}</span>
                                        </div>
                                    }
                                })
                            }
                        </div>
                    }
                } else {
                    html! {}
                }
            }

            <div class="skill-actions">
                <button
                    class={classes!("btn", "btn-primary", props.is_executing.then_some("loading"))}
                    onclick={props.on_execute.clone()}
                    disabled={props.is_executing}
                >
                    {
                        if props.is_executing {
                            html! { "执行中..." }
                        } else {
                            html! { "执行技能" }
                        }
                    }
                </button>
            </div>

            {
                if let Some(result) = &props.execution_result {
                    html! {
                        <div class="skill-result">
                            <h4>{"执行结果"}</h4>
                            <div class={classes!(
                                "result-card",
                                (result.status == "error").then_some("error")
                            )}>
                                <div class="result-status">
                                    {format!("状态: {}", &result.status)}
                                </div>
                                {
                                    if let Some(res) = &result.result {
                                        html! {
                                            <pre class="result-data">
                                                {serde_json::to_string_pretty(res).unwrap_or_default()}
                                            </pre>
                                        }
                                    } else {
                                        html! {}
                                    }
                                }
                                {
                                    if let Some(err) = &result.error {
                                        html! {
                                            <div class="result-error">{err}</div>
                                        }
                                    } else {
                                        html! {}
                                    }
                                }
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

fn format_category(category: &str) -> String {
    match category {
        "search" => "搜索".to_string(),
        "code" => "代码".to_string(),
        "file" => "文件".to_string(),
        "knowledge" => "知识库".to_string(),
        _ => category.to_string(),
    }
}
