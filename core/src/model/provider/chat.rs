use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, ChatCompletionNamedToolChoice,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ChatCompletionTool, ChatCompletionToolChoiceOption, ChatCompletionTools,
    CreateChatCompletionRequestArgs, FunctionCall as OaFunctionCall, FunctionName, FunctionObject,
    ImageDetail as OaImageDetail, ImageUrl as OaImageUrl, Role as OaRole, ToolChoiceOptions,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Message, MessageRole, TokenUsage};
use crate::model::traits::ChatService;
use crate::model::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ToolCall,
    ToolCallType, ToolChoice,
};

use super::client::AsyncOpenAIClient;

#[async_trait]
impl ChatService for AsyncOpenAIClient {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        // 请求构建与调用在下方重试闭包内完成（每次重试重建请求）。

        // 非流式调用内置指数退避重试（全局默认策略，对齐 opencode/dsh-llm-retry）：
        // 网络/传输/超时类错误自动退避重试；async-openai API 错误不暴露 HTTP 状态码、
        // 无法区分 429/5xx 与 4xx，归类不可重试以避免重复计费与误伤。
        let response = crate::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || async {
                let mut builder = CreateChatCompletionRequestArgs::default();
                builder.model(&request.model);
                builder.messages(convert_messages(&request.messages));
                builder.stream(false);
                if let Some(n) = request.n {
                    builder.n(n.min(u8::MAX as usize) as u8);
                }
                Self::apply_shared_params(&mut builder, &request);
                let oa_request = builder.build().map_err(|e| RetryableFailure {
                    retryable: false,
                    message: format!("构建请求失败：{}", e),
                })?;
                let attempted = if let Some(effort) = request.thinking_effort.as_deref() {
                    if effort == "off" {
                        self.client.chat().create(oa_request).await
                    } else {
                        let mut body =
                            serde_json::to_value(&oa_request).map_err(|e| RetryableFailure {
                                retryable: false,
                                message: format!("序列化请求失败：{}", e),
                            })?;
                        apply_thinking_params(&mut body, effort, &request.model);
                        self.client.chat().create_byot(body).await
                    }
                } else {
                    self.client.chat().create(oa_request).await
                };
                attempted.map_err(|e| RetryableFailure {
                    retryable: match &e {
                        async_openai::error::OpenAIError::Reqwest(r) => {
                            crate::model::retry::should_retry_transport(r)
                        }
                        _ => false,
                    },
                    message: format!("聊天补全失败：{}", e),
                })
            },
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：{}", e.message)))?;

        let choices = response
            .choices
            .into_iter()
            .map(|c| {
                let msg = c.message;
                let tool_calls = msg.tool_calls.map(|calls| {
                    calls
                        .into_iter()
                        .filter_map(|tc| match tc {
                            ChatCompletionMessageToolCalls::Function(f) => Some(ToolCall {
                                id: f.id,
                                call_type: ToolCallType::Function,
                                function: crate::model::types::FunctionCall {
                                    name: f.function.name,
                                    arguments: f.function.arguments,
                                },
                            }),
                            _ => None,
                        })
                        .collect()
                });
                ChatChoice {
                    index: c.index as usize,
                    message: Message {
                        role: convert_role(&msg.role),
                        content: msg.content.unwrap_or_default(),
                        content_parts: None,
                        tool_calls,
                        tool_call_id: None,
                        tool_duration_ms: None,
                        tool_error: None,
                        reasoning_content: None,
                    },
                    finish_reason: c
                        .finish_reason
                        .as_ref()
                        .map(|r| AsyncOpenAIClient::finish_reason_str(r).to_string()),
                }
            })
            .collect();

        let usage = response.usage.map_or_else(TokenUsage::default, |u| {
            let mut tu = TokenUsage::new(u.prompt_tokens as usize, u.completion_tokens as usize);
            // 非流式路径走 async-openai 类型化反序列化：缓存命中只有 OpenAI 标准
            // prompt_tokens_details.cached_tokens 可用（DeepSeek 顶层 prompt_cache_hit_tokens
            // 会被丢弃；流式路径在 drain_sse_lines 中从原始 JSON 提取，见 extract_cache_tokens）。
            if let Some(ref details) = u.prompt_tokens_details {
                tu.cache_read = details.cached_tokens.unwrap_or(0) as usize;
            }
            tu
        });

        Ok(ChatCompletionResponse {
            id: response.id,
            object: "chat.completion".to_string(),
            created: response.created as i64,
            model: response.model,
            choices,
            usage,
        })
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        let messages = convert_messages(&request.messages);

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(&request.model);
        builder.messages(messages);
        builder.stream(true);
        Self::apply_shared_params(&mut builder, &request);

        let oa_request = builder
            .build()
            .map_err(|e| TianyanError::Custom(format!("模型服务错误：构建流式请求失败：{}", e)))?;

        // 一律走手写 SSE 解析（create_raw_stream）：async-openai 0.34 不解析
        // DeepSeek 思考模型的 reasoning_content 增量字段——推理内容在反序列化时
        // 被静默丢弃，模型"想完没说话"（content 空 + 仅 reasoning）会被误判为
        // 空响应。思考模型天然携带该字段（与 enable_thinking 开关无关），
        // 非思考模型不携带，行为与原有 async-openai 路径一致。
        let mut body = serde_json::to_value(&oa_request)
            .map_err(|e| TianyanError::Custom(format!("模型服务错误：序列化请求失败: {}", e)))?;
        if let Some(effort) = request.thinking_effort.as_deref() {
            if effort != "off" {
                apply_thinking_params(&mut body, effort, &request.model);
            }
        }
        self.create_raw_stream(body).await
    }
}

/// 把思考强度映射为请求体参数（按模型族区分，档位为模型自己声明的集合）：
/// - Qwen 思考族（模型名含 qwen）：`enable_thinking: true` + `thinking_budget`（DashScope 原生强度参数）
/// - 其余 OpenAI 兼容族：`enable_thinking: true` + `reasoning_effort`（OpenAI 标准强度参数，
///   DeepSeek 等已实测容忍附加参数）
///
/// 关闭（Off）时调用方不进入本函数，不附加任何参数（模型默认行为）。
/// 把思考强度档位原样映射为请求体参数（档位值为模型自己声明的，不做本地翻译）：
/// - Qwen 思考族（模型名含 qwen）：`enable_thinking: true` + `thinking_budget`（DashScope 原生强度参数；
///   已知档位映射预算，未知档位用默认预算 4096）
/// - 其余 OpenAI 兼容族：`enable_thinking: true` + `reasoning_effort: <原档位值>`（原样透传，
///   DeepSeek 等已实测容忍附加参数）
///
/// 档位为 "off" 时调用方不进入本函数，不附加任何参数（模型默认行为）。
fn apply_thinking_params(body: &mut Value, effort: &str, model: &str) {
    body["enable_thinking"] = Value::Bool(true);
    let lower = model.to_lowercase();
    if lower.contains("qwen") {
        let budget = match effort {
            "low" => 1024,
            "medium" => 4096,
            "high" => 16384,
            _ => 4096,
        };
        body["thinking_budget"] = Value::Number(budget.into());
    } else {
        body["reasoning_effort"] = Value::String(effort.to_string());
    }
}

/// 请求发送阶段的失败（携带是否可重试的类别标记）；重试是请求层自我保护。
#[derive(Debug)]
struct RetryableFailure {
    /// 是否属于可重试的临时失败（HTTP 429 / 5xx / 网络 / 超时）。
    retryable: bool,
    message: String,
}

impl AsyncOpenAIClient {
    /// 手写 SSE 流式补全：POST {base_url}/chat/completions（body 已含 stream: true），
    /// 逐行解析 data: 事件为内部 ChatCompletionChunk 并经 channel 返回。
    ///
    /// async-openai 0.34 的流式反序列化丢弃 DeepSeek 思考模型的
    /// reasoning_content 增量字段（其 chunk 结构体无此字段），本方法绕开
    /// async-openai 直接解析原始 SSE，保留推理过程增量；非思考模型不携带
    /// 该字段，解析结果与原有 async-openai 路径一致。
    async fn create_raw_stream(
        &self,
        body: Value,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // 请求发送阶段内置指数退避重试（全局默认策略，对齐 opencode/dsh-llm-retry）：
        // HTTP 429 / 5xx / 网络 / 超时 自动退避重试；一旦进入 2xx，流读取失败不再整段重发。
        let response = crate::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || self.send_chat_request(&url, &body),
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：{}", e.message)))?;

        let mut byte_stream = response.bytes_stream();
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::new();
            loop {
                match byte_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend_from_slice(&bytes);
                        drain_sse_lines(&mut buf, &tx).await;
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(TianyanError::Custom(format!(
                                "模型服务错误：流读取失败：{}",
                                e
                            ))))
                            .await;
                        return;
                    }
                    None => {
                        // 流结束：处理残留缓冲（无换行的最后一个 data 行）
                        drain_sse_lines(&mut buf, &tx).await;
                        return;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 发送一次流式补全请求并检查状态（供重试循环调用；每次调用重建请求）。
    async fn send_chat_request(
        &self,
        url: &str,
        body: &Value,
    ) -> std::result::Result<reqwest::Response, RetryableFailure> {
        let mut req = self.http.post(url).json(body);
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if !self.api_key.is_empty() && !self.headers.contains_key("Authorization") {
            req = req.bearer_auth(&self.api_key);
        }
        let response = req.send().await.map_err(|e| {
            let retryable = crate::model::retry::should_retry_transport(&e);
            RetryableFailure {
                retryable,
                message: format!("流式请求失败：{}", e),
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let detail: String = detail.chars().take(300).collect();
            let retryable = crate::model::retry::should_retry_http(status);
            return Err(RetryableFailure {
                retryable,
                message: format!("流式请求被拒绝（HTTP {}）：{}", status, detail),
            });
        }
        Ok(response)
    }

    fn apply_shared_params(
        builder: &mut CreateChatCompletionRequestArgs,
        request: &ChatCompletionRequest,
    ) {
        if let Some(max_tokens) = request.max_tokens {
            builder.max_completion_tokens(max_tokens as u32);
        }
        if let Some(temperature) = request.temperature {
            builder.temperature(temperature);
        }
        if let Some(top_p) = request.top_p {
            builder.top_p(top_p);
        }
        if let Some(ref stop) = request.stop {
            builder.stop(stop.clone());
        }
        if let Some(presence_penalty) = request.presence_penalty {
            builder.presence_penalty(presence_penalty);
        }
        if let Some(frequency_penalty) = request.frequency_penalty {
            builder.frequency_penalty(frequency_penalty);
        }
        if let Some(ref tools) = request.tools {
            let oa_tools: Vec<ChatCompletionTools> = tools
                .iter()
                .map(|t| {
                    ChatCompletionTools::Function(ChatCompletionTool {
                        function: FunctionObject {
                            name: t.function.name.clone(),
                            description: Some(t.function.description.clone()),
                            parameters: Some(t.function.parameters.clone()),
                            strict: None,
                        },
                    })
                })
                .collect();
            builder.tools(oa_tools);
        }
        if let Some(ref tool_choice) = request.tool_choice {
            let oa_choice = match tool_choice {
                ToolChoice::Auto => ChatCompletionToolChoiceOption::Mode(ToolChoiceOptions::Auto),
                ToolChoice::None => ChatCompletionToolChoiceOption::Mode(ToolChoiceOptions::None),
                ToolChoice::Function { function } => {
                    ChatCompletionToolChoiceOption::Function(ChatCompletionNamedToolChoice {
                        function: FunctionName {
                            name: function.name.clone(),
                        },
                    })
                }
            };
            builder.tool_choice(oa_choice);
        }
    }
}

/// 从 SSE 缓冲中逐行提取完整的 data: 事件并送入 channel；
/// 缓冲中未以换行结尾的残留数据留待后续字节到达或流结束时再处理。
///
/// 容错契约（对齐 DSH 流式协议：usage 可附着 finish chunk 或作为尾随的
/// usage-only chunk 到达，两者都不应中断或刷屏）：
/// - `[DONE]`：正常流结束标记；
/// - usage-only 块（`{"choices":[],"usage":...}` / 兼容层 `{"choices":[],
///   "cost":"..."}`）：结构化识别后静默跳过（非错误）；
/// - 真正的错误对象（带 `error` 字段）：照常上报并终止；
/// - 其它无法解析的行：debug 级记录后跳过（兼容未知扩展字段）。
async fn drain_sse_lines(buf: &mut Vec<u8>, tx: &mpsc::Sender<Result<ChatCompletionChunk>>) {
    loop {
        let Some(nl) = buf.iter().position(|&b| b == b'\n') else {
            return;
        };
        let line: Vec<u8> = buf.drain(..=nl).collect();
        // 去掉行尾换行符；空行与注释行（: keep-alive）直接跳过
        let line = String::from_utf8_lossy(&line[..line.len().saturating_sub(1)]);
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            continue;
        }
        // 先解析原始 JSON：既能反序列化 chunk，又能从 usage 原始字段提取缓存命中
        // （DeepSeek prompt_cache_hit_tokens 等不进 async-openai 类型，须在此补充）。
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            tracing::debug!(line = %data, "跳过非 JSON 流式数据行");
            continue;
        };
        match serde_json::from_value::<ChatCompletionChunk>(value.clone()) {
            Ok(mut chunk) => {
                if let (Some(usage_value), Some(usage)) = (value.get("usage"), chunk.usage.as_mut())
                {
                    extract_cache_tokens(usage_value, usage);
                }
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
            Err(_) => {
                // 结构化分流：先识别 usage-only / cost 块（正常流尾，静默跳过），
                // 再识别错误对象（上报），其余按 debug 记录（不刷屏）。
                if value.get("error").is_some() {
                    let _ = tx
                        .send(Err(TianyanError::Custom(format!(
                            "模型服务错误：提供商返回错误：{}",
                            value
                        ))))
                        .await;
                    return;
                }
                // usage-only / cost 块：choices 为空（或缺失）且无 delta 内容
                let choices_empty = value
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .map(|c| c.is_empty())
                    .unwrap_or(true);
                let has_usage = value.get("usage").is_some() || value.get("cost").is_some();
                if choices_empty && has_usage {
                    tracing::debug!(line = %data, "流式 usage-only 块（正常流尾）");
                    continue;
                }
                tracing::debug!(line = %data, "跳过无法解析的流式数据行");
                continue;
            }
        }
    }
}

/// 从提供商原始 usage JSON 提取缓存命中 token 数（各提供商标识不同）：
/// - DeepSeek：顶层 `prompt_cache_hit_tokens`
/// - DashScope（阿里云百炼 OpenAI 兼容）：顶层 `input_cache_hit_tokens`
/// - OpenAI 标准：`prompt_tokens_details.cached_tokens`
///
/// 命中任一即填充 `cache_read`；`cache_write` 提供商一般不下发，保持 0。
fn extract_cache_tokens(usage_value: &Value, usage: &mut TokenUsage) {
    let hit = usage_value
        .get("prompt_cache_hit_tokens")
        .or_else(|| usage_value.get("input_cache_hit_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage_value
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_u64)
        });
    if let Some(hit) = hit {
        usage.cache_read = hit as usize;
    }
}

/// 构造用户消息内容：携带多模态片段时输出数组格式（文本 + 图片），
/// 否则保持纯文本（与旧行为一致）。
fn user_content(m: &Message) -> ChatCompletionRequestUserMessageContent {
    let Some(parts) = &m.content_parts else {
        return ChatCompletionRequestUserMessageContent::Text(m.content.clone());
    };
    if parts.is_empty() {
        return ChatCompletionRequestUserMessageContent::Text(m.content.clone());
    }

    let oa_parts: Vec<ChatCompletionRequestUserMessageContentPart> = parts
        .iter()
        .map(|p| {
            if p.content_type == "image_url" {
                match &p.image_url {
                    Some(url) => ChatCompletionRequestUserMessageContentPart::ImageUrl(
                        ChatCompletionRequestMessageContentPartImage {
                            image_url: OaImageUrl {
                                url: url.url.clone(),
                                detail: url.detail.clone().and_then(|d| match d.as_str() {
                                    "low" => Some(OaImageDetail::Low),
                                    "high" => Some(OaImageDetail::High),
                                    "auto" => Some(OaImageDetail::Auto),
                                    _ => None,
                                }),
                            },
                        },
                    ),
                    None => ChatCompletionRequestUserMessageContentPart::Text(
                        ChatCompletionRequestMessageContentPartText {
                            text: "[图片（内容缺失）]".to_string(),
                        },
                    ),
                }
            } else {
                ChatCompletionRequestUserMessageContentPart::Text(
                    ChatCompletionRequestMessageContentPartText {
                        text: p.text.clone().unwrap_or_default(),
                    },
                )
            }
        })
        .collect();

    ChatCompletionRequestUserMessageContent::Array(oa_parts)
}

fn convert_messages(messages: &[Message]) -> Vec<ChatCompletionRequestMessage> {
    messages
        .iter()
        .map(|m| match m.role {
            MessageRole::System => {
                ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                    content: ChatCompletionRequestSystemMessageContent::Text(m.content.clone()),
                    name: None,
                })
            }
            MessageRole::User => {
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: user_content(m),
                    name: None,
                })
            }
            MessageRole::Assistant => {
                let tool_calls = m.tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| {
                            // 健壮性：历史中的工具调用参数若为非法 JSON（如流式截断/
                            // 模型中途停止导致的截断参数），降级为 {} 再发送——否则上游
                            // 因单个坏工具调用 400 拒绝整轮请求，污染后续所有对话。
                            let arguments =
                                if serde_json::from_str::<Value>(&c.function.arguments).is_ok() {
                                    c.function.arguments.clone()
                                } else {
                                    tracing::warn!(
                                        tool = %c.function.name,
                                        id = %c.id,
                                        "工具调用参数非合法 JSON，降级为 {{}} 发送"
                                    );
                                    "{}".to_string()
                                };
                            ChatCompletionMessageToolCalls::Function(
                                ChatCompletionMessageToolCall {
                                    id: c.id.clone(),
                                    function: OaFunctionCall {
                                        name: c.function.name.clone(),
                                        arguments,
                                    },
                                },
                            )
                        })
                        .collect()
                });
                ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                    content: if m.content.is_empty() {
                        None
                    } else {
                        Some(ChatCompletionRequestAssistantMessageContent::Text(
                            m.content.clone(),
                        ))
                    },
                    tool_calls,
                    ..Default::default()
                })
            }
            MessageRole::Tool => {
                ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                    content: ChatCompletionRequestToolMessageContent::Text(m.content.clone()),
                    tool_call_id: m.tool_call_id.clone().unwrap_or_default(),
                })
            }
        })
        .collect()
}

fn convert_role(role: &OaRole) -> MessageRole {
    match role {
        OaRole::System => MessageRole::System,
        OaRole::User => MessageRole::User,
        OaRole::Assistant => MessageRole::Assistant,
        OaRole::Tool => MessageRole::Tool,
        OaRole::Function => MessageRole::Assistant,
    }
}

#[cfg(test)]
mod convert_tests {
    use super::*;
    use crate::common::types::Message;
    use crate::model::types::{FunctionCall, ToolCall, ToolCallType};

    #[test]
    fn test_convert_system_message() {
        let msg = Message::system("You are a helpful assistant");
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_user_message() {
        let msg = Message::user("Hello");
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_user_message_with_images_produces_array() {
        use async_openai::types::chat::{
            ChatCompletionRequestMessage, ChatCompletionRequestUserMessageContent,
            ChatCompletionRequestUserMessageContentPart,
        };

        let msg =
            Message::user_with_images("描述这张图", vec!["data:image/png;base64,AAAA".to_string()]);
        let converted = convert_messages(&[msg]);
        let ChatCompletionRequestMessage::User(user) = &converted[0] else {
            panic!("应生成 User 消息");
        };
        let ChatCompletionRequestUserMessageContent::Array(parts) = &user.content else {
            panic!("带图片的 User 消息应为 Array 格式");
        };
        assert_eq!(parts.len(), 2, "文本 + 图片共 2 个片段");
        assert!(matches!(
            parts[0],
            ChatCompletionRequestUserMessageContentPart::Text(_)
        ));
        assert!(matches!(
            parts[1],
            ChatCompletionRequestUserMessageContentPart::ImageUrl(_)
        ));
    }

    #[test]
    fn test_convert_user_message_plain_text_unchanged() {
        use async_openai::types::chat::{
            ChatCompletionRequestMessage, ChatCompletionRequestUserMessageContent,
        };

        let msg = Message::user("Hello");
        let converted = convert_messages(&[msg]);
        let ChatCompletionRequestMessage::User(user) = &converted[0] else {
            panic!("应生成 User 消息");
        };
        assert!(
            matches!(
                user.content,
                ChatCompletionRequestUserMessageContent::Text(_)
            ),
            "无图片时保持纯文本格式（向后兼容）"
        );
    }

    #[test]
    fn test_convert_assistant_with_tools() {
        let msg = Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_abc".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: r#"{"q":"test"}"#.to_string(),
                },
            }],
        );
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }

    #[test]
    fn test_convert_assistant_sanitizes_invalid_tool_arguments() {
        // 流式截断导致的非法 JSON 参数：发送前降级为 {}，避免上游 400
        let msg = Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_bad".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "apply_patch".to_string(),
                    // 未闭合的 JSON（模拟截断）
                    arguments: r#"{"patch": "*** Begin Patch"#.to_string(),
                },
            }],
        );
        let converted = convert_messages(&[msg]);
        let ChatCompletionRequestMessage::Assistant(assistant) = &converted[0] else {
            panic!("应生成 Assistant 消息");
        };
        let calls = assistant.tool_calls.as_ref().expect("应有 tool_calls");
        let ChatCompletionMessageToolCalls::Function(f) = &calls[0] else {
            panic!("应为 Function 调用");
        };
        assert_eq!(f.function.arguments, "{}");
    }

    #[test]
    fn test_convert_tool_message_with_id() {
        let msg = Message::tool("call_abc", r#"{"result":"ok"}"#);
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }
}
