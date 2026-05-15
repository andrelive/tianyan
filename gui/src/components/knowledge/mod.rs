use gloo_timers::callback::Timeout;
use wasm_bindgen_futures::spawn_local;
use web_sys::{File, HtmlInputElement};
use yew::prelude::*;

use crate::api::knowledge::{self, KnowledgeSearchResult};
use crate::state::{AppAction, AppState, View};

const DEBOUNCE_MS: u32 = 300;

#[derive(Clone, PartialEq)]
enum KnowledgeTab {
    Search,
    Ingest,
}

#[derive(Properties, PartialEq)]
pub struct KnowledgePanelProps {
    pub state: UseReducerHandle<AppState>,
}

#[function_component(KnowledgePanel)]
pub fn knowledge_panel(props: &KnowledgePanelProps) -> Html {
    let state = props.state.clone();
    let active_tab = use_state(|| KnowledgeTab::Search);

    let search_query = use_state(String::new);
    let search_results = use_state(Vec::<KnowledgeSearchResult>::new);
    let search_total = use_state(|| 0usize);
    let is_searching = use_state(|| false);
    let suggestions = use_state(Vec::<String>::new);
    let show_suggestions = use_state(|| false);
    let selected_result = use_state(|| None::<KnowledgeSearchResult>);
    let debounce_handle = use_mut_ref(|| None::<Timeout>);

    let ingest_files = use_state(Vec::<File>::new);
    let ingest_tags = use_state(String::new);
    let ingest_statuses = use_state(Vec::<IngestJobState>::new);
    let is_ingesting = use_state(|| false);

    let on_back = {
        let state = state.clone();
        Callback::from(move |_| {
            state.dispatch(AppAction::SetView(View::Chat));
        })
    };

    let on_switch_tab = {
        let active_tab = active_tab.clone();
        Callback::from(move |tab: KnowledgeTab| {
            active_tab.set(tab);
        })
    };

    let on_search_input = {
        let search_query = search_query.clone();
        let debounce_handle = debounce_handle.clone();
        let suggestions = suggestions.clone();
        let show_suggestions = show_suggestions.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                let value = input.value();
                search_query.set(value.clone());

                if let Some(handle) = debounce_handle.borrow_mut().take() {
                    handle.cancel();
                }

                if value.trim().len() >= 2 {
                    let suggestions = suggestions.clone();
                    let show_suggestions = show_suggestions.clone();
                    let handle = Timeout::new(DEBOUNCE_MS, move || {
                        spawn_local(async move {
                            match knowledge::get_suggestions(&value).await {
                                Ok(resp) => {
                                    suggestions.set(resp.suggestions);
                                    show_suggestions.set(true);
                                }
                                Err(_) => {}
                            }
                        });
                    });
                    *debounce_handle.borrow_mut() = Some(handle);
                } else {
                    suggestions.set(Vec::new());
                    show_suggestions.set(false);
                }
            }
        })
    };

    let on_clear_search = {
        let search_query = search_query.clone();
        let search_results = search_results.clone();
        let search_total = search_total.clone();
        let suggestions = suggestions.clone();
        let show_suggestions = show_suggestions.clone();
        let selected_result = selected_result.clone();
        Callback::from(move |_| {
            search_query.set(String::new());
            search_results.set(Vec::new());
            search_total.set(0);
            suggestions.set(Vec::new());
            show_suggestions.set(false);
            selected_result.set(None);
        })
    };

    let do_search = {
        let search_query = search_query.clone();
        let search_results = search_results.clone();
        let search_total = search_total.clone();
        let is_searching = is_searching.clone();
        let suggestions = suggestions.clone();
        let show_suggestions = show_suggestions.clone();
        let selected_result = selected_result.clone();
        Callback::from(move |_: ()| {
            let q = (*search_query).clone();
            if q.trim().is_empty() {
                return;
            }
            is_searching.set(true);
            show_suggestions.set(false);
            selected_result.set(None);
            let search_results = search_results.clone();
            let search_total = search_total.clone();
            let is_searching = is_searching.clone();
            let suggestions = suggestions.clone();
            spawn_local(async move {
                match knowledge::search(&q, 20, 0, None, None).await {
                    Ok(resp) => {
                        search_results.set(resp.results);
                        search_total.set(resp.total);
                    }
                    Err(err) => {
                        gloo_console::error!(format!("搜索失败: {}", err));
                        search_results.set(Vec::new());
                        search_total.set(0);
                    }
                }
                is_searching.set(false);
                suggestions.set(Vec::new());
            });
        })
    };

    let on_search_keydown = {
        let do_search = do_search.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                do_search.emit(());
            }
        })
    };

    let on_suggestion_click = {
        let search_query = search_query.clone();
        let show_suggestions = show_suggestions.clone();
        let do_search = do_search.clone();
        Callback::from(move |suggestion: String| {
            search_query.set(suggestion.clone());
            show_suggestions.set(false);
            do_search.emit(());
        })
    };

    let on_select_result = {
        let selected_result = selected_result.clone();
        Callback::from(move |result: KnowledgeSearchResult| {
            selected_result.set(Some(result));
        })
    };

    let on_back_to_results = {
        let selected_result = selected_result.clone();
        Callback::from(move |_| {
            selected_result.set(None);
        })
    };

    let on_file_select = {
        let ingest_files = ingest_files.clone();
        Callback::from(move |e: Event| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                if let Some(file_list) = input.files() {
                    let mut current = (*ingest_files).clone();
                    for i in 0..file_list.length() {
                        if let Some(file) = file_list.item(i) {
                            current.push(file);
                        }
                    }
                    ingest_files.set(current);
                }
            }
        })
    };

    let on_remove_file = {
        let ingest_files = ingest_files.clone();
        Callback::from(move |index: usize| {
            let mut current = (*ingest_files).clone();
            if index < current.len() {
                current.remove(index);
                ingest_files.set(current);
            }
        })
    };

    let on_drop_files = {
        let ingest_files = ingest_files.clone();
        Callback::from(move |dropped: Vec<File>| {
            let mut current = (*ingest_files).clone();
            current.extend(dropped);
            ingest_files.set(current);
        })
    };

    let on_tags_change = {
        let ingest_tags = ingest_tags.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                ingest_tags.set(input.value());
            }
        })
    };

    let on_ingest = {
        let ingest_files = ingest_files.clone();
        let ingest_tags = ingest_tags.clone();
        let ingest_statuses = ingest_statuses.clone();
        let is_ingesting = is_ingesting.clone();
        Callback::from(move |_| {
            let files = (*ingest_files).clone();
            if files.is_empty() {
                return;
            }
            is_ingesting.set(true);
            let tags_str = (*ingest_tags).clone();
            let tags: Option<Vec<String>> = if tags_str.trim().is_empty() {
                None
            } else {
                Some(tags_str.split(',').map(|s| s.trim().to_string()).collect())
            };
            let is_ingesting = is_ingesting.clone();
            let ingest_files_state = ingest_files.clone();
            let ingest_statuses = ingest_statuses.clone();
            spawn_local(async move {
                match knowledge::ingest_files(files, tags, None).await {
                    Ok(resp) => {
                        let statuses: Vec<IngestJobState> = resp
                            .files
                            .iter()
                            .map(|f| IngestJobState {
                                filename: f.filename.clone(),
                                status: f.status.clone(),
                                progress: if f.status == "queued" { 0.0 } else { 1.0 },
                                error: f.error.clone(),
                                job_id: Some(resp.job_id.clone()),
                            })
                            .collect();
                        ingest_statuses.set(statuses.clone());
                        ingest_files_state.set(Vec::new());
                        is_ingesting.set(false);

                        if resp.success && !resp.job_id.is_empty() {
                            let job_id = resp.job_id.clone();
                            let ingest_statuses = ingest_statuses.clone();
                            spawn_local(async move {
                                poll_ingest_status(&job_id, ingest_statuses, 0).await;
                            });
                        }
                    }
                    Err(err) => {
                        gloo_console::error!(format!("摄入失败: {}", err));
                        is_ingesting.set(false);
                    }
                }
            });
        })
    };

    html! {
        <div class="knowledge-panel">
            <div class="knowledge-header">
                <h2>{"知识库"}</h2>
                <button
                    class="btn btn-icon btn-back"
                    onclick={on_back}
                    title="返回对话"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
                    </svg>
                </button>
            </div>

            <div class="knowledge-tabs">
                <TabButton
                    label="搜索"
                    is_active={*active_tab == KnowledgeTab::Search}
                    onclick={on_switch_tab.reform(move |_| KnowledgeTab::Search)}
                />
                <TabButton
                    label="摄入"
                    is_active={*active_tab == KnowledgeTab::Ingest}
                    onclick={on_switch_tab.reform(move |_| KnowledgeTab::Ingest)}
                />
            </div>

            <div class="knowledge-content">
                {
                    if *active_tab == KnowledgeTab::Search {
                        html! {
                            if let Some(result) = &*selected_result {
                                <SearchResultDetail
                                    result={result.clone()}
                                    on_back={on_back_to_results}
                                />
                            } else {
                                <SearchTab
                                    search_query={(*search_query).clone()}
                                    search_results={(*search_results).clone()}
                                    search_total={*search_total}
                                    is_searching={*is_searching}
                                    suggestions={(*suggestions).clone()}
                                    show_suggestions={*show_suggestions}
                                    on_input={on_search_input}
                                    on_keydown={on_search_keydown}
                                    on_search={do_search.reform(move |_| ())}
                                    on_clear={on_clear_search}
                                    on_suggestion_click={on_suggestion_click}
                                    on_select_result={on_select_result}
                                />
                            }
                        }
                    } else {
                        html! {
                            <IngestTab
                                files={(*ingest_files).clone()}
                                tags={(*ingest_tags).clone()}
                                statuses={(*ingest_statuses).clone()}
                                is_ingesting={*is_ingesting}
                                on_file_select={on_file_select}
                                on_drop_files={on_drop_files}
                                on_remove_file={on_remove_file}
                                on_tags_change={on_tags_change}
                                on_ingest={on_ingest}
                            />
                        }
                    }
                }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct TabButtonProps {
    label: String,
    is_active: bool,
    onclick: Callback<MouseEvent>,
}

#[function_component(TabButton)]
fn tab_button(props: &TabButtonProps) -> Html {
    html! {
        <button
            class={classes!("knowledge-tab", props.is_active.then_some("active"))}
            onclick={props.onclick.clone()}
        >
            {&props.label}
        </button>
    }
}

#[derive(Properties, PartialEq)]
struct SearchTabProps {
    search_query: String,
    search_results: Vec<KnowledgeSearchResult>,
    search_total: usize,
    is_searching: bool,
    suggestions: Vec<String>,
    show_suggestions: bool,
    on_input: Callback<InputEvent>,
    on_keydown: Callback<KeyboardEvent>,
    on_search: Callback<()>,
    on_clear: Callback<MouseEvent>,
    on_suggestion_click: Callback<String>,
    on_select_result: Callback<KnowledgeSearchResult>,
}

#[function_component(SearchTab)]
fn search_tab(props: &SearchTabProps) -> Html {
    html! {
        <div class="knowledge-search">
            <div class="knowledge-search-bar">
                <div class="search-input-wrapper">
                    <svg class="search-icon" viewBox="0 0 24 24" fill="currentColor" width="18" height="18">
                        <path d="M15.5 14h-.79l-.28-.27C15.41 12.59 16 11.11 16 9.5 16 5.91 13.09 3 9.5 3S3 5.91 3 9.5 5.91 16 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"/>
                    </svg>
                    <input
                        type="text"
                        class="search-input"
                        placeholder="搜索知识库..."
                        value={props.search_query.clone()}
                        oninput={props.on_input.clone()}
                        onkeydown={props.on_keydown.clone()}
                    />
                    {
                        if !props.search_query.is_empty() {
                            html! {
                                <button class="btn btn-icon search-clear" onclick={props.on_clear.clone()} title="清除">
                                    <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
                                        <path d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/>
                                    </svg>
                                </button>
                            }
                        } else {
                            html! {}
                        }
                    }
                </div>
                <button
                    class="btn btn-primary btn-search"
                    onclick={props.on_search.reform(move |_| ())}
                    disabled={props.is_searching || props.search_query.trim().is_empty()}
                >
                    { if props.is_searching { "搜索中..." } else { "搜索" } }
                </button>
            </div>

            {
                if props.show_suggestions && !props.suggestions.is_empty() {
                    html! {
                        <div class="search-suggestions">
                            {
                                for props.suggestions.iter().map(|s| {
                                    let suggestion_value = s.clone();
                                    let on_click = props.on_suggestion_click.reform(move |_| suggestion_value.clone());
                                    html! {
                                        <div class="suggestion-item" key={s.clone()} onclick={on_click}>
                                            {s.clone()}
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

            <div class="knowledge-results">
                {
                    if props.is_searching {
                        html! {
                            <div class="loading-state">
                                <div class="loading-spinner" />
                                <p>{"正在搜索..."}</p>
                            </div>
                        }
                    } else if !props.search_results.is_empty() {
                        html! {
                            <>
                                <div class="results-header">
                                    <span class="results-count">
                                        {format!("找到 {} 条结果", props.search_total)}
                                    </span>
                                </div>
                                <div class="results-list">
                                    {
                                        for props.search_results.iter().map(|result| {
                                            let card_result = result.clone();
                                            let on_click = props.on_select_result.reform(move |_| card_result.clone());
                                            html! {
                                                <SearchResultCard
                                                    key={result.id.clone()}
                                                    result={result.clone()}
                                                    onclick={on_click}
                                                />
                                            }
                                        })
                                    }
                                </div>
                            </>
                        }
                    } else if !props.search_query.is_empty() {
                        html! {
                            <div class="empty-state">
                                <div class="empty-icon">{"📭"}</div>
                                <h3>{"未找到结果"}</h3>
                                <p>{"尝试使用不同的关键词搜索"}</p>
                            </div>
                        }
                    } else {
                        html! {
                            <div class="empty-state">
                                <div class="empty-icon">{"📚"}</div>
                                <h3>{"知识库检索"}</h3>
                                <p>{"输入关键词搜索已摄入的知识文档"}</p>
                            </div>
                        }
                    }
                }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct SearchResultCardProps {
    result: KnowledgeSearchResult,
    onclick: Callback<MouseEvent>,
}

#[function_component(SearchResultCard)]
fn search_result_card(props: &SearchResultCardProps) -> Html {
    let source_icon = match props.result.source.as_str() {
        "file" | "document" => "📄",
        "url" | "web" => "🌐",
        "note" => "📝",
        _ => "📋",
    };

    html! {
        <div class="result-card" onclick={props.onclick.clone()}>
            <div class="result-header">
                <div class="result-title">
                    {
                        if let Some(meta) = &props.result.metadata {
                            if let Some(title) = &meta.title {
                                html! { <span class="result-name">{title}</span> }
                            } else {
                                html! { <span class="result-name">{&props.result.id}</span> }
                            }
                        } else {
                            html! { <span class="result-name">{&props.result.id}</span> }
                        }
                    }
                </div>
                <div class="result-meta">
                    <span class="result-score">
                        {format!("{:.0}%", props.result.score * 100.0)}
                    </span>
                    <span class="result-source">
                        {source_icon} {" "} {&props.result.source}
                    </span>
                </div>
            </div>
            <div class="result-preview">
                {truncate(&props.result.content, 200)}
            </div>
            {
                if let Some(meta) = &props.result.metadata {
                    if let Some(tags) = &meta.tags {
                        html! {
                            <div class="result-tags">
                                {
                                    for tags.iter().map(|tag| {
                                        html! { <span class="tag" key={tag.clone()}>{tag}</span> }
                                    })
                                }
                            </div>
                        }
                    } else {
                        html! {}
                    }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct SearchResultDetailProps {
    result: KnowledgeSearchResult,
    on_back: Callback<MouseEvent>,
}

#[function_component(SearchResultDetail)]
fn search_result_detail(props: &SearchResultDetailProps) -> Html {
    html! {
        <div class="result-detail">
            <div class="result-detail-header">
                <button
                    class="btn btn-icon"
                    onclick={props.on_back.clone()}
                    title="返回结果列表"
                >
                    <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
                        <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z"/>
                    </svg>
                </button>
                <div class="result-detail-title">
                    {
                        if let Some(meta) = &props.result.metadata {
                            if let Some(title) = &meta.title {
                                html! { <h3>{title}</h3> }
                            } else {
                                html! { <h3>{"文档详情"}</h3> }
                            }
                        } else {
                            html! { <h3>{"文档详情"}</h3> }
                        }
                    }
                </div>
            </div>

            <div class="result-detail-meta">
                <div class="meta-item">
                    <span class="meta-label">{"ID: "}</span>
                    <span class="meta-value">{&props.result.id}</span>
                </div>
                <div class="meta-item">
                    <span class="meta-label">{"来源: "}</span>
                    <span class="meta-value">{&props.result.source}</span>
                </div>
                <div class="meta-item">
                    <span class="meta-label">{"相关度: "}</span>
                    <span class="meta-value">
                        {format!("{:.0}%", props.result.score * 100.0)}
                    </span>
                </div>
                {
                    if let Some(meta) = &props.result.metadata {
                        if let Some(url) = &meta.url {
                            html! {
                                <div class="meta-item">
                                    <span class="meta-label">{"URL: "}</span>
                                    <span class="meta-value meta-url">{url}</span>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    } else {
                        html! {}
                    }
                }
                {
                    if let Some(meta) = &props.result.metadata {
                        if let Some(timestamp) = &meta.timestamp {
                            html! {
                                <div class="meta-item">
                                    <span class="meta-label">{"时间: "}</span>
                                    <span class="meta-value">{timestamp}</span>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    } else {
                        html! {}
                    }
                }
            </div>

            <div class="result-detail-content">
                <h4>{"内容"}</h4>
                <div class="content-body">
                    {&props.result.content}
                </div>
            </div>

            {
                if let Some(meta) = &props.result.metadata {
                    if let Some(tags) = &meta.tags {
                        html! {
                            <div class="result-detail-tags">
                                <h4>{"标签"}</h4>
                                <div class="tags-list">
                                    {
                                        for tags.iter().map(|tag| {
                                            html! { <span class="tag" key={tag.clone()}>{tag}</span> }
                                        })
                                    }
                                </div>
                            </div>
                        }
                    } else {
                        html! {}
                    }
                } else {
                    html! {}
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct IngestTabProps {
    files: Vec<File>,
    tags: String,
    statuses: Vec<IngestJobState>,
    is_ingesting: bool,
    on_file_select: Callback<Event>,
    on_drop_files: Callback<Vec<File>>,
    on_remove_file: Callback<usize>,
    on_tags_change: Callback<InputEvent>,
    on_ingest: Callback<MouseEvent>,
}

#[derive(Clone, PartialEq)]
struct IngestJobState {
    filename: String,
    status: String,
    progress: f32,
    error: Option<String>,
    job_id: Option<String>,
}

#[function_component(IngestTab)]
fn ingest_tab(props: &IngestTabProps) -> Html {
    let drop_active = use_state(|| false);

    let on_drag_over = {
        let drop_active = drop_active.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            drop_active.set(true);
        })
    };

    let on_drag_leave = {
        let drop_active = drop_active.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            drop_active.set(false);
        })
    };

    let on_drop = {
        let drop_active = drop_active.clone();
        let on_drop_files = props.on_drop_files.clone();
        Callback::from(move |e: DragEvent| {
            e.prevent_default();
            drop_active.set(false);
            if let Some(data_transfer) = e.data_transfer() {
                if let Some(file_list) = data_transfer.files() {
                    let mut files: Vec<File> = Vec::new();
                    for i in 0..file_list.length() {
                        if let Some(file) = file_list.item(i) {
                            files.push(file);
                        }
                    }
                    if !files.is_empty() {
                        on_drop_files.emit(files);
                    }
                }
            }
        })
    };

    html! {
        <div class="knowledge-ingest">
            <div class="ingest-upload-area">
                <div
                    class={classes!("upload-dropzone", (*drop_active).then_some("active"))}
                    ondragover={on_drag_over}
                    ondragleave={on_drag_leave}
                    ondrop={on_drop}
                >
                    <div class="dropzone-icon">{"📤"}</div>
                    <p class="dropzone-text">{"拖拽文件到此处上传"}</p>
                    <p class="dropzone-hint">{"或点击下方按钮选择文件"}</p>
                    <input
                        type="file"
                        id="knowledge-file-input"
                        class="file-input-hidden"
                        multiple={true}
                        onchange={props.on_file_select.clone()}
                    />
                    <label for="knowledge-file-input" class="btn btn-secondary">
                        {"选择文件"}
                    </label>
                </div>

                {
                    if !props.files.is_empty() {
                        html! {
                            <div class="upload-file-list">
                                <h4>{format!("已选择 {} 个文件", props.files.len())}</h4>
                                {
                                    for props.files.iter().enumerate().map(|(i, file)| {
                                        let index = i;
                                        let on_remove = props.on_remove_file.reform(move |_| index);
                                        html! {
                                            <div class="upload-file-item" key={format!("{}-{}", index, file.name())}>
                                                <span class="file-icon">{"📄"}</span>
                                                <span class="file-name">{file.name()}</span>
                                                <span class="file-size">{format_size(file.size() as u64)}</span>
                                                <button
                                                    class="btn btn-icon btn-sm"
                                                    onclick={on_remove}
                                                    title="移除"
                                                >
                                                    <svg viewBox="0 0 24 24" fill="currentColor" width="14" height="14">
                                                        <path d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/>
                                                    </svg>
                                                </button>
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

                <div class="ingest-options">
                    <div class="option-field">
                        <label for="ingest-tags">{"标签 (逗号分隔)"}</label>
                        <input
                            id="ingest-tags"
                            type="text"
                            placeholder="例如: rust, document, guide"
                            value={props.tags.clone()}
                            oninput={props.on_tags_change.clone()}
                        />
                    </div>
                </div>

                <button
                    class="btn btn-primary btn-ingest"
                    onclick={props.on_ingest.clone()}
                    disabled={props.is_ingesting || props.files.is_empty()}
                >
                    {
                        if props.is_ingesting {
                            html! { <><span class="loading-spinner-sm" />{" 摄入中..."}</> }
                        } else {
                            html! { {format!("摄入 {} 个文件", props.files.len())} }
                        }
                    }
                </button>
            </div>

            {
                if !props.statuses.is_empty() {
                    html! {
                        <div class="ingest-status-list">
                            <h4>{"摄入任务"}</h4>
                            {
                                for props.statuses.iter().map(|s| {
                                    let status_class = match s.status.as_str() {
                                        "completed" => "success",
                                        "failed" | "error" => "error",
                                        "processing" => "processing",
                                        _ => "pending",
                                    };
                                    html! {
                                        <div class={classes!("ingest-status-item", status_class)} key={s.filename.clone()}>
                                            <div class="status-info">
                                                <span class="status-filename">{&s.filename}</span>
                                                <span class="status-text">{&s.status}</span>
                                            </div>
                                            {
                                                if s.progress > 0.0 && s.progress < 1.0 {
                                                    html! {
                                                        <div class="status-progress">
                                                            <div class="progress-bar" style={format!("width: {}%", s.progress * 100.0)} />
                                                        </div>
                                                    }
                                                } else {
                                                    html! {}
                                                }
                                            }
                                            {
                                                if let Some(err) = &s.error {
                                                    html! {
                                                        <div class="status-error">{err}</div>
                                                    }
                                                } else {
                                                    html! {}
                                                }
                                            }
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
        </div>
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = chars.into_iter().take(max_len).collect();
        format!("{}...", truncated)
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", size, UNITS[unit])
}

async fn poll_ingest_status(
    job_id: &str,
    statuses: UseStateHandle<Vec<IngestJobState>>,
    attempt: u32,
) {
    if attempt > 30 {
        return;
    }
    gloo_timers::future::TimeoutFuture::new(1000).await;
    match knowledge::get_ingest_status(job_id).await {
        Ok(status) => {
            let mut current = (*statuses).to_vec();
            for item in &mut current {
                if item.job_id.as_deref() == Some(job_id) {
                    item.status = status.status.clone();
                    item.progress = status.progress;
                    item.error = status.error.clone();
                }
            }
            let all_done = current
                .iter()
                .all(|s| s.status == "completed" || s.status == "failed" || s.status == "error");
            statuses.set(current);
            if !all_done {
                let statuses = statuses;
                let job_id = job_id.to_string();
                spawn_local(async move {
                    poll_ingest_status(&job_id, statuses, attempt + 1).await;
                });
            }
        }
        Err(err) => {
            gloo_console::error!(format!("获取摄入状态失败: {}", err));
        }
    }
}
