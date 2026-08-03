use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImageArgs,
    ChatCompletionRequestMessageContentPartTextArgs, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    CreateChatCompletionRequestArgs, ImageDetail, ImageUrlArgs,
};
use async_trait::async_trait;

use crate::common::error::{Result, TianyanError};
use crate::common::types::TokenUsage;
use crate::model::traits::VlmService;
use crate::model::types::{
    ContentPart, VisionChoice, VisionContent, VisionMessage, VisionRequest, VisionResponse,
};

use super::client::AsyncOpenAIClient;

#[async_trait]
impl VlmService for AsyncOpenAIClient {
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse> {
        let model = request.model;
        let messages = convert_messages(request.messages)?;

        let mut request_builder = CreateChatCompletionRequestArgs::default();
        request_builder.model(model).messages(messages);

        if let Some(max_tokens) = request.max_tokens {
            request_builder.max_completion_tokens(max_tokens as u32);
        }
        if let Some(temp) = request.temperature {
            request_builder.temperature(temp);
        }

        let chat_request = request_builder
            .build()
            .map_err(|e| TianyanError::Custom(format!("VLM 服务错误：构建请求失败: {}", e)))?;

        let response =
            self.client.chat().create(chat_request).await.map_err(|e| {
                TianyanError::Custom(format!("VLM 服务错误：视觉分析请求失败: {}", e))
            })?;

        let choices = response
            .choices
            .into_iter()
            .map(|choice| {
                let content = choice.message.content.unwrap_or_default();
                VisionChoice {
                    index: choice.index as usize,
                    message: VisionMessage {
                        role: "assistant".to_string(),
                        content: VisionContent::Text(content),
                    },
                    finish_reason: choice
                        .finish_reason
                        .map(|fr| AsyncOpenAIClient::finish_reason_str(&fr).to_string()),
                }
            })
            .collect();

        let usage = match response.usage {
            Some(u) => TokenUsage {
                prompt_tokens: u.prompt_tokens as usize,
                completion_tokens: u.completion_tokens as usize,
                total_tokens: u.total_tokens as usize,
            },
            None => TokenUsage::default(),
        };

        Ok(VisionResponse {
            id: response.id,
            object: response.object,
            created: response.created as i64,
            model: response.model,
            choices,
            usage,
        })
    }
}

fn convert_messages(
    vision_messages: Vec<VisionMessage>,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    let mut messages = Vec::with_capacity(vision_messages.len());
    for vision_msg in vision_messages {
        let chat_msg = match vision_msg.role.as_str() {
            "system" => {
                let content = extract_text_content(vision_msg.content);
                ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                    content: ChatCompletionRequestSystemMessageContent::Text(content),
                    name: None,
                })
            }
            _ => {
                let content = match vision_msg.content {
                    VisionContent::Text(text) => {
                        ChatCompletionRequestUserMessageContent::Text(text)
                    }
                    VisionContent::MultiPart(parts) => {
                        let content_parts = convert_content_parts(parts)?;
                        ChatCompletionRequestUserMessageContent::Array(content_parts)
                    }
                };
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content,
                    name: None,
                })
            }
        };
        messages.push(chat_msg);
    }
    Ok(messages)
}

fn extract_text_content(content: VisionContent) -> String {
    match content {
        VisionContent::Text(text) => text,
        VisionContent::MultiPart(parts) => parts
            .iter()
            .filter_map(|p| p.text.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn convert_content_parts(
    parts: Vec<ContentPart>,
) -> Result<Vec<ChatCompletionRequestUserMessageContentPart>> {
    let mut content_parts = Vec::with_capacity(parts.len());
    for part in parts {
        match part.content_type.as_str() {
            "text" => {
                let text_content = ChatCompletionRequestMessageContentPartTextArgs::default()
                    .text(part.text.unwrap_or_default())
                    .build()
                    .map_err(|e| {
                        TianyanError::Custom(format!("VLM 服务错误：构建文本内容部分失败: {}", e))
                    })?;
                content_parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                    text_content,
                ));
            }
            "image_url" => {
                let image_url = part.image_url.ok_or_else(|| {
                    TianyanError::Custom(
                        "VLM 服务错误：image_url 部分缺少 image_url 字段".to_string(),
                    )
                })?;
                let detail = map_image_detail(image_url.detail);
                let img_url = ImageUrlArgs::default()
                    .url(image_url.url)
                    .detail(detail)
                    .build()
                    .map_err(|e| {
                        TianyanError::Custom(format!("VLM 服务错误：构建图片 URL 失败: {}", e))
                    })?;
                let image_part = ChatCompletionRequestMessageContentPartImageArgs::default()
                    .image_url(img_url)
                    .build()
                    .map_err(|e| {
                        TianyanError::Custom(format!("VLM 服务错误：构建图片内容部分失败: {}", e))
                    })?;
                content_parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                    image_part,
                ));
            }
            other => {
                return Err(TianyanError::Custom(format!(
                    "VLM 服务错误：不支持的内容类型: {}",
                    other
                )));
            }
        }
    }
    Ok(content_parts)
}

fn map_image_detail(detail: Option<String>) -> ImageDetail {
    match detail.as_deref() {
        Some("low") => ImageDetail::Low,
        Some("high") => ImageDetail::High,
        Some("auto") => ImageDetail::Auto,
        _ => ImageDetail::Auto,
    }
}

/// 根据图片字节数据推断 MIME 类型
pub(crate) fn infer_mime_type(data: &[u8]) -> &str {
    if data.len() >= 3 && &data[0..3] == b"\xFF\xD8\xFF" {
        "image/jpeg"
    } else if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1A\n" {
        "image/png"
    } else if data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        "image/gif"
    } else if data.len() >= 2 && &data[0..2] == b"BM" {
        "image/bmp"
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png" // safe fallback
    }
}
