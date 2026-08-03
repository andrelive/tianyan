use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, ChatCompletionNamedToolChoice,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionTool, ChatCompletionToolChoiceOption,
    ChatCompletionTools, CreateChatCompletionRequestArgs, FunctionCall as OaFunctionCall,
    FunctionName, FunctionObject, Role as OaRole, ToolChoiceOptions,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Message, MessageRole, TokenUsage};
use crate::model::traits::ChatService;
use crate::model::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChunkChoice,
    DeltaContent, ToolCall, ToolCallDelta, ToolCallFunctionDelta, ToolCallType, ToolChoice,
};

use super::client::AsyncOpenAIClient;

#[async_trait]
impl ChatService for AsyncOpenAIClient {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let messages = convert_messages(&request.messages);

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder.model(&request.model);
        builder.messages(messages);
        builder.stream(false);
        if let Some(n) = request.n {
            builder.n(n.min(u8::MAX as usize) as u8);
        }
        Self::apply_shared_params(&mut builder, &request);

        let oa_request = builder
            .build()
            .map_err(|e| TianyanError::Custom(format!("模型服务错误：构建请求失败：{}", e)))?;

        let response = if request.enable_thinking == Some(true) {
            let mut body = serde_json::to_value(&oa_request).map_err(|e| {
                TianyanError::Custom(format!("模型服务错误：序列化请求失败: {}", e))
            })?;
            body["enable_thinking"] = Value::Bool(true);
            self.client.chat().create_byot(body).await
        } else {
            self.client.chat().create(oa_request).await
        }
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：聊天补全失败：{}", e)))?;

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
                        tool_calls,
                        tool_call_id: None,
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
            TokenUsage::new(u.prompt_tokens as usize, u.completion_tokens as usize)
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

        let mut stream = if request.enable_thinking == Some(true) {
            let mut body = serde_json::to_value(&oa_request).map_err(|e| {
                TianyanError::Custom(format!("模型服务错误：序列化请求失败: {}", e))
            })?;
            body["enable_thinking"] = Value::Bool(true);
            self.client.chat().create_stream_byot(body).await
        } else {
            self.client.chat().create_stream(oa_request).await
        }
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：创建流失败：{}", e)))?;

        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(oa_chunk) => {
                        let chunk = ChatCompletionChunk {
                            id: oa_chunk.id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created: oa_chunk.created as i64,
                            model: oa_chunk.model,
                            choices: oa_chunk
                                .choices
                                .into_iter()
                                .map(|c| ChunkChoice {
                                    index: c.index as usize,
                                    delta: DeltaContent {
                                        role: c.delta.role.as_ref().map(convert_role),
                                        content: c.delta.content,
                                        tool_calls: c.delta.tool_calls.map(|tcs| {
                                            tcs.into_iter()
                                                .map(|tc| ToolCallDelta {
                                                    index: tc.index as usize,
                                                    id: tc.id,
                                                    call_type: tc
                                                        .r#type
                                                        .map(|_| "function".to_string()),
                                                    function: tc.function.map(|f| {
                                                        ToolCallFunctionDelta {
                                                            name: f.name,
                                                            arguments: f.arguments,
                                                        }
                                                    }),
                                                })
                                                .collect()
                                        }),
                                    },
                                    finish_reason: c.finish_reason.as_ref().map(|r| {
                                        AsyncOpenAIClient::finish_reason_str(r).to_string()
                                    }),
                                })
                                .collect(),
                            usage: oa_chunk.usage.map(|u| TokenUsage {
                                prompt_tokens: u.prompt_tokens as usize,
                                completion_tokens: u.completion_tokens as usize,
                                total_tokens: u.total_tokens as usize,
                            }),
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if let Err(send_err) = tx
                            .send(Err(TianyanError::Custom(format!(
                                "模型服务错误：流错误：{}",
                                e
                            ))))
                            .await
                        {
                            tracing::warn!(error = %send_err, "流错误通知发送失败");
                        }
                        return;
                    }
                }
            }
        });

        Ok(rx)
    }
}

impl AsyncOpenAIClient {
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
                    content: ChatCompletionRequestUserMessageContent::Text(m.content.clone()),
                    name: None,
                })
            }
            MessageRole::Assistant => {
                let tool_calls = m.tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| {
                            ChatCompletionMessageToolCalls::Function(
                                ChatCompletionMessageToolCall {
                                    id: c.id.clone(),
                                    function: OaFunctionCall {
                                        name: c.function.name.clone(),
                                        arguments: c.function.arguments.clone(),
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
    fn test_convert_tool_message_with_id() {
        let msg = Message::tool("call_abc", r#"{"result":"ok"}"#);
        let converted = convert_messages(&[msg]);
        assert_eq!(converted.len(), 1);
    }
}
