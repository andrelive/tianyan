// 本模块不依赖 async-openai：请求体与响应都是 JSON（响应侧 `RawChatResponse`
// 是自研宽容形状，理由见其文档）。
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::types::TokenUsage;
use tianyan::model::types::{
    ContentPart, VisionChoice, VisionContent, VisionMessage, VisionRequest, VisionResponse,
};
use tianyan::model::VlmService;

use crate::client::ChatClient;
use tianyan_provider_client::RetryableFailure;

#[async_trait]
impl VlmService for ChatClient {
    async fn analyze_image(&self, request: VisionRequest) -> Result<VisionResponse> {
        let model = request.model;
        let messages = convert_messages(request.messages)?;

        let mut body = json!({
            "model": model,
            "messages": messages,
        });
        if let Some(max_tokens) = request.max_tokens {
            body["max_completion_tokens"] = json!(max_tokens);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        // 请求体与响应**都绕开库类型化**：请求侧因为方言差异（内容块形状、字段名）
        // 不可穷举；响应侧因为 async-openai 的 `Usage` 三字段为必填，非标准网关
        // （只回 total_tokens 等）会让整次视觉分析失败（与 embedding 事故同因、
        // 同手法）。发送复用 chat 的请求层单点（同一 http client / headers /
        // RetryPolicy / 状态码语义分类）。
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let response = tianyan::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || self.send_chat_request(&url, &body, "VLM 服务错误", None),
        )
        .await
        .map_err(|e| TianyanError::Custom(e.message))?;

        let raw: RawChatResponse = response
            .json()
            .await
            .map_err(|e| TianyanError::Custom(format!("VLM 服务错误：解析响应失败：{e}")))?;

        let choices = raw
            .choices
            .into_iter()
            .map(|choice| VisionChoice {
                index: choice.index,
                message: VisionMessage {
                    role: "assistant".to_string(),
                    content: VisionContent::Text(choice.message.content.unwrap_or_default()),
                },
                finish_reason: choice.finish_reason,
            })
            .collect();

        Ok(VisionResponse {
            id: raw.id,
            object: raw.object,
            created: raw.created,
            model: raw.model,
            choices,
            usage: vision_usage(raw.usage),
        })
    }
}

fn convert_messages(vision_messages: Vec<VisionMessage>) -> Result<Vec<Value>> {
    let mut messages = Vec::with_capacity(vision_messages.len());
    for vision_msg in vision_messages {
        let msg = match vision_msg.role.as_str() {
            "system" => json!({
                "role": "system",
                "content": extract_text_content(vision_msg.content),
            }),
            _ => {
                let content = match vision_msg.content {
                    VisionContent::Text(text) => Value::String(text),
                    VisionContent::MultiPart(parts) => Value::Array(convert_content_parts(parts)?),
                };
                json!({ "role": "user", "content": content })
            }
        };
        messages.push(msg);
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

fn convert_content_parts(parts: Vec<ContentPart>) -> Result<Vec<Value>> {
    let mut content_parts = Vec::with_capacity(parts.len());
    for part in parts {
        match part.content_type.as_str() {
            "text" => {
                content_parts.push(json!({
                    "type": "text",
                    "text": part.text.unwrap_or_default(),
                }));
            }
            "image_url" => {
                let image_url = part.image_url.ok_or_else(|| {
                    TianyanError::Custom(
                        "VLM 服务错误：image_url 部分缺少 image_url 字段".to_string(),
                    )
                })?;
                // detail 始终外发（未知取值按 auto）——与改造前 `map_image_detail`
                // 的 `_ => ImageDetail::Auto` 映射字节一致。
                let detail = match image_url.detail.as_deref() {
                    Some("low") => "low",
                    Some("high") => "high",
                    _ => "auto",
                };
                content_parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": image_url.url, "detail": detail },
                }));
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

/// 上游 chat completions 响应的**宽容**形状（视觉分析所需子集）。
///
/// 存在理由与 embedding 的 `RawEmbeddingResponse` 同源：async-openai 的
/// `Usage` 把 `prompt_tokens` / `completion_tokens` / `total_tokens` 定义为
/// **必填**，非标准网关（如只回 `total_tokens`）会让类型化反序列化在**库内部**
/// 失败——图片已成功分析与计费，结果却整次丢弃。
#[derive(Debug, Deserialize)]
struct RawChatResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    object: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<RawChoice>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

/// 单个选择项（字段宽容）。
#[derive(Debug, Deserialize)]
struct RawChoice {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    message: RawChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// 选择项消息（视觉场景只取文本内容）。
#[derive(Debug, Deserialize, Default)]
struct RawChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

/// 用量原始形状（字段存在性各实现不一，全部宽容缺省）。
#[derive(Debug, Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

/// 归一用量：usage 整体缺失 → 全 0（与既有语义一致）；缺 `total_tokens` 时以
/// `prompt + completion` 兜底（部分网关省略 total）。
fn vision_usage(raw: Option<RawUsage>) -> TokenUsage {
    let Some(u) = raw else {
        return TokenUsage::default();
    };
    let total = if u.total_tokens > 0 {
        u.total_tokens
    } else {
        u.prompt_tokens + u.completion_tokens
    };
    TokenUsage {
        prompt_tokens: u.prompt_tokens as usize,
        completion_tokens: u.completion_tokens as usize,
        total_tokens: total as usize,
        cache_read: 0,
        cache_write: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// 本地 mock HTTP 服务器：一次性返回给定 JSON（不校验请求体）。
    fn spawn_mock(listener: std::net::TcpListener, body: String) -> std::net::SocketAddr {
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 32768];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        });
        addr
    }

    fn client(name: &str, addr: std::net::SocketAddr) -> ChatClient {
        let provider = tianyan::config::ProviderConfig {
            name: name.to_string(),
            endpoint: format!("http://{addr}/v1"),
            api_key: Some("k".to_string()),
            models: vec![],
            timeout: 5,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
            ..Default::default()
        };
        ChatClient::from_provider(&provider).unwrap()
    }

    fn text_request() -> VisionRequest {
        VisionRequest::new(
            "qwen-vl-max",
            vec![VisionMessage {
                role: "user".to_string(),
                content: VisionContent::Text("描述这张图".to_string()),
            }],
        )
    }

    /// 事故同源回归：非标准 usage 形状（仅 `total_tokens`）必须可解析。
    #[tokio::test]
    async fn test_vision_accepts_nonstandard_usage_shape() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = r#"{"id":"c1","object":"chat.completion","created":1700000000,"model":"qwen-vl-max","choices":[{"index":0,"message":{"role":"assistant","content":"一只猫"},"finish_reason":"stop"}],"usage":{"total_tokens":42}}"#.to_string();
        let addr = spawn_mock(listener, body);
        let client = client("dashscope", addr);

        let resp = client
            .analyze_image(text_request())
            .await
            .expect("非标准 usage 形状不得导致整次视觉分析失败");
        assert_eq!(resp.model, "qwen-vl-max");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        match &resp.choices[0].message.content {
            VisionContent::Text(t) => assert_eq!(t, "一只猫"),
            other => panic!("期望文本内容: {other:?}"),
        }
        assert_eq!(resp.usage.total_tokens, 42);
    }

    /// 标准形状；`total_tokens` 缺失时以 prompt + completion 兜底。
    #[tokio::test]
    async fn test_vision_standard_usage_and_total_fallback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = r#"{"id":"c2","object":"chat.completion","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":30,"completion_tokens":12}}"#.to_string();
        let addr = spawn_mock(listener, body);
        let client = client("openai", addr);

        let resp = client.analyze_image(text_request()).await.expect("应成功");
        assert_eq!(resp.usage.prompt_tokens, 30);
        assert_eq!(resp.usage.completion_tokens, 12);
        assert_eq!(
            resp.usage.total_tokens, 42,
            "缺 total 时以 prompt+completion 兜底"
        );
    }

    /// usage 整体缺失 → 全 0（与既有语义一致），不失败。
    #[tokio::test]
    async fn test_vision_missing_usage_is_tolerated() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let body = r#"{"id":"c3","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"content":"ok"}}]}"#.to_string();
        let addr = spawn_mock(listener, body);
        let client = client("mock", addr);

        let resp = client
            .analyze_image(text_request())
            .await
            .expect("缺 usage 不得失败");
        assert_eq!(resp.usage.total_tokens, 0);
        assert_eq!(resp.choices[0].finish_reason, None);
    }
}
