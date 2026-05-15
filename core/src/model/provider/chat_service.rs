use async_trait::async_trait;
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, CreateChatCompletionRequestArgs,
    Role as OaRole,
};
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Message, MessageRole, TokenUsage};
use crate::model::traits::ModelService;
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatChoice, ChunkChoice,
    DeltaContent,
};

use super::client::AsyncOpenAIClient;

#[async_trait]
impl ModelService for AsyncOpenAIClient {
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
            builder.n(n as u8);
        }
        Self::apply_shared_params(&mut builder, &request);

        let oa_request = builder
            .build()
            .map_err(|e| TianyanError::ModelService(format!("构建请求失败：{}", e)))?;

        let response = if request.enable_thinking == Some(true) {
            let mut body = serde_json::to_value(&oa_request)
                .map_err(|e| TianyanError::ModelService(format!("序列化请求失败: {}", e)))?;
            body["enable_thinking"] = Value::Bool(true);
            self.client.chat().create_byot(body).await
        } else {
            self.client.chat().create(oa_request).await
        }
        .map_err(|e| TianyanError::ModelService(format!("聊天补全失败：{}", e)))?;

        let choices = response
            .choices
            .into_iter()
            .map(|c| {
                let msg = c.message;
                ChatChoice {
                    index: c.index as usize,
                    message: Message {
                        role: convert_role(&msg.role),
                        content: msg.content.unwrap_or_default(),
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason: c.finish_reason.as_ref().map(|r| super::finish_reason_str(r).to_string()),
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
            .map_err(|e| TianyanError::ModelService(format!("构建流式请求失败：{}", e)))?;

        let mut stream = if request.enable_thinking == Some(true) {
            let mut body = serde_json::to_value(&oa_request)
                .map_err(|e| TianyanError::ModelService(format!("序列化请求失败: {}", e)))?;
            body["enable_thinking"] = Value::Bool(true);
            self.client.chat().create_stream_byot(body).await
        } else {
            self.client.chat().create_stream(oa_request).await
        }
        .map_err(|e| TianyanError::ModelService(format!("创建流失败：{}", e)))?;

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
                                        role: c.delta.role.as_ref().map(|r| convert_role(r)),
                                        content: c.delta.content,
                                    },
                                    finish_reason: c
                                        .finish_reason
                                        .as_ref()
                                        .map(|r| super::finish_reason_str(r).to_string()),
                                })
                                .collect(),
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(TianyanError::ModelService(format!("流错误：{}", e))))
                            .await;
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
            builder.max_tokens(max_tokens as u32);
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
                ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                    content: Some(ChatCompletionRequestAssistantMessageContent::Text(
                        m.content.clone(),
                    )),
                    refusal: None,
                    name: None,
                    audio: None,
                    tool_calls: None,
                    function_call: None,
                })
            }
            MessageRole::Tool => {
                ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                    content: ChatCompletionRequestToolMessageContent::Text(m.content.clone()),
                    tool_call_id: String::new(),
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