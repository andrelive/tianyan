#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::ReadableStreamDefaultReader;

use super::{post, ApiResult};

macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        web_sys::console::log_1(&format!($($arg)*).into());
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_calls: Option<Vec<SkillCallInfo>>,
    #[serde(default)]
    pub chunk_type: Option<StreamChunkType>,
}

impl ChatMessage {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
            skill_calls: None,
            chunk_type: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn with_skill_calls(mut self, calls: Vec<SkillCallInfo>) -> Self {
        self.skill_calls = Some(calls);
        self
    }

    pub fn with_chunk_type(mut self, chunk_type: StreamChunkType) -> Self {
        self.chunk_type = Some(chunk_type);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_max_tokens() -> u32 {
    2048
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub session_id: String,
    pub message: ChatMessage,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 技能调用信息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillCallInfo {
    pub skill_id: String,
    pub skill_name: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStreamEvent {
    pub id: String,
    pub session_id: String,
    pub delta: String,
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_calls: Option<Vec<SkillCallInfo>>,
    #[serde(default)]
    pub chunk_type: StreamChunkType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum StreamChunkType {
    #[default]
    Answer,
    Thought,
    ToolCall,
    Observation,
    Clarification,
    Error,
}

pub async fn send_message(request: ChatRequest) -> ApiResult<ChatResponse> {
    post("/chat", &request).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegenerateRequest {
    pub session_id: String,
    pub message_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMessageRequest {
    pub session_id: String,
    pub message_index: usize,
    pub new_content: String,
}

pub async fn regenerate_message(request: RegenerateRequest) -> ApiResult<ChatResponse> {
    post("/chat/regenerate", &request).await
}

pub async fn edit_message(request: EditMessageRequest) -> ApiResult<ChatResponse> {
    post("/chat/edit", &request).await
}

pub struct StreamHandler {
    abort_controller: Option<web_sys::AbortController>,
}

impl StreamHandler {
    pub fn new() -> Self {
        Self {
            abort_controller: None,
        }
    }

    /// 构建带通用 headers 的 RequestInit
    fn build_request_init(
        body: &str,
        signal: Option<&web_sys::AbortSignal>,
    ) -> web_sys::RequestInit {
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_body(&JsValue::from_str(body));

        if let Some(sig) = signal {
            init.set_signal(Some(sig));
        }

        let headers = web_sys::Headers::new().expect("Headers::new 不应失败");
        let _ = headers.set("Content-Type", "application/json");
        let _ = headers.set("Accept", "text/event-stream");
        init.set_headers(&headers);

        init
    }

    pub fn start<F>(
        &mut self,
        request: ChatRequest,
        on_message: F,
        on_error: Option<Box<dyn Fn(String)>>,
        on_complete: Option<Box<dyn Fn()>>,
    ) where
        F: Fn(ChatStreamEvent) + 'static,
    {
        let api_base = super::get_api_base();
        let url = format!("{}/chat/stream", api_base);

        // Create abort controller for cancellation with timeout
        let (abort_controller, signal) =
            super::create_abort_controller_with_timeout(super::DEFAULT_TIMEOUT_SECS);
        let abort_controller = Some(abort_controller);

        let body = match serde_json::to_string(&request) {
            Ok(json) => json,
            Err(e) => {
                if let Some(on_error) = on_error {
                    on_error(format!("Failed to serialize request: {}", e));
                }
                return;
            }
        };

        spawn_local(async move {
            let window = match web_sys::window() {
                Some(w) => w,
                None => {
                    if let Some(on_error) = on_error {
                        on_error("No window available".to_string());
                    }
                    return;
                }
            };

            let init = Self::build_request_init(&body, Some(&signal));
            let fetch_promise = window.fetch_with_str_and_init(&url, &init);

            debug_log!("Fetching SSE from: {}", url);

            let response = match wasm_bindgen_futures::JsFuture::from(fetch_promise).await {
                Ok(resp) => {
                    debug_log!("SSE fetch succeeded");
                    web_sys::Response::from(resp)
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("SSE fetch failed: {:?}", e).into());
                    if let Some(on_error) = on_error {
                        on_error(format!("Fetch failed: {:?}", e));
                    }
                    return;
                }
            };

            if !response.ok() {
                if let Some(on_error) = on_error {
                    on_error(format!("HTTP error: {}", response.status()));
                }
                return;
            }

            debug_log!("SSE response status: {}", response.status());

            // Get readable stream body
            let body = match response.body() {
                Some(b) => {
                    debug_log!("Got response body stream");
                    b
                }
                None => {
                    web_sys::console::error_1(&"No response body".into());
                    if let Some(on_error) = on_error {
                        on_error("No response body".to_string());
                    }
                    return;
                }
            };

            let reader: ReadableStreamDefaultReader = match body.get_reader().dyn_into() {
                Ok(r) => r,
                Err(e) => {
                    if let Some(on_error) = on_error {
                        on_error(format!("Failed to get reader: {:?}", e));
                    }
                    return;
                }
            };

            // Read stream chunks
            let mut stream_error: Option<String> = None;
            loop {
                let read_promise = reader.read();
                let result = match wasm_bindgen_futures::JsFuture::from(read_promise).await {
                    Ok(r) => r,
                    Err(e) => {
                        stream_error = Some(format!("Read error: {:?}", e));
                        break;
                    }
                };

                let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
                    .unwrap_or(JsValue::from_bool(false))
                    .as_bool()
                    .unwrap_or(false);

                if done {
                    break;
                }

                let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
                    .unwrap_or(JsValue::NULL);

                if let Ok(chunk) = value.dyn_into::<js_sys::Uint8Array>() {
                    let bytes = chunk.to_vec();
                    debug_log!("SSE raw bytes received: len={}", bytes.len());
                    if let Ok(text) = String::from_utf8(bytes) {
                        debug_log!("SSE raw text: {}", text);
                        // Parse SSE format: data: {...}
                        for line in text.lines() {
                            let line = line.trim();
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    if let Some(ref on_complete) = on_complete {
                                        on_complete();
                                    }
                                    return;
                                }

                                if let Ok(event) = serde_json::from_str::<ChatStreamEvent>(data) {
                                    debug_log!("SSE event received: id={}, delta_len={}, finish_reason={:?}",
                                        event.id, event.delta.len(), event.finish_reason);
                                    on_message(event.clone());

                                    if event.finish_reason.is_some() {
                                        if let Some(ref on_complete) = on_complete {
                                            on_complete();
                                        }
                                    }
                                } else {
                                    web_sys::console::error_1(
                                        &format!("Failed to parse SSE data: {}", data).into(),
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // 确保流结束时调用回调（无论正常结束还是出错）
            if let Some(err) = stream_error {
                if let Some(on_error) = &on_error {
                    on_error(err);
                }
            } else if let Some(on_complete) = &on_complete {
                on_complete();
            }
        });

        self.abort_controller = abort_controller;
    }

    pub fn stop(&mut self) {
        if let Some(controller) = self.abort_controller.take() {
            controller.abort();
        }
    }
}

impl Drop for StreamHandler {
    fn drop(&mut self) {
        self.stop();
    }
}
