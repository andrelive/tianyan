use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};
use crate::common::types::TokenUsage;
use crate::config::EmbeddingUsageShape;
use crate::model::traits::EmbeddingService;
use crate::model::types::{EmbeddingData, EmbeddingInput, EmbeddingRequest, EmbeddingResponse};

use super::chat::{classify_http_status, RetryableFailure};
use super::client::AsyncOpenAIClient;

/// 上游嵌入响应的原始形状（**宽容**反序列化）。
///
/// 存在理由：async-openai 0.34 的 `CreateEmbeddingResponse.usage.prompt_tokens`
/// 是**必填**字段（无 `#[serde(default)]`），而 DashScope `text-embedding-v4` 的
/// 兼容端点只返回 `usage.total_tokens`（实测）——类型化反序列化会在**库内部**
/// 失败（`missing field 'prompt_tokens'`），而向量本身完全正常，整次检索却
/// 因此失败。故此处绕开库类型自行解析，并对每个字段宽容（与 chat 因
/// `reasoning_content` 弃用库流式解析同一成因、同一手法）。
#[derive(Debug, Deserialize)]
struct RawEmbeddingResponse {
    #[serde(default)]
    object: String,
    #[serde(default)]
    model: String,
    data: Vec<RawEmbedding>,
    #[serde(default)]
    usage: RawEmbeddingUsage,
}

/// 单个嵌入向量（字段宽容：部分实现不返回 `object`）。
#[derive(Debug, Deserialize)]
struct RawEmbedding {
    #[serde(default)]
    index: usize,
    embedding: Vec<f32>,
    #[serde(default)]
    object: String,
}

/// 嵌入用量的原始形状（字段存在性各实现不一，全部宽容缺省）。
#[derive(Debug, Deserialize, Default)]
struct RawEmbeddingUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

/// 把原始 usage 归一为 [`TokenUsage`]（**声明优先 + 宽容兜底**）。
///
/// - [`EmbeddingUsageShape::PromptAndTotal`]：以 `prompt_tokens` 为准，缺失时以
///   `total_tokens` 兜底（部分网关只回后者）；
/// - [`EmbeddingUsageShape::TotalOnly`]：以 `total_tokens` 为唯一真值
///   （DashScope 语义：嵌入请求的输入即全部 token），缺失时反向兜底。
///
/// 嵌入没有 completion 侧，`completion_tokens` 恒为 0（与既有行为一致）。
fn embedding_usage(raw: &RawEmbeddingUsage, shape: EmbeddingUsageShape) -> TokenUsage {
    let prompt_tokens = match shape {
        EmbeddingUsageShape::PromptAndTotal => {
            if raw.prompt_tokens > 0 {
                raw.prompt_tokens
            } else {
                raw.total_tokens
            }
        }
        EmbeddingUsageShape::TotalOnly => {
            if raw.total_tokens > 0 {
                raw.total_tokens
            } else {
                raw.prompt_tokens
            }
        }
    };
    let total_tokens = if raw.total_tokens > 0 {
        raw.total_tokens
    } else {
        prompt_tokens
    };

    let mut usage = TokenUsage::new(prompt_tokens as usize, 0);
    usage.total_tokens = total_tokens as usize;
    usage
}

impl AsyncOpenAIClient {
    /// 发送一次嵌入请求（供重试循环调用）。
    ///
    /// 与 chat 共用同一套请求层单点：同一个 reqwest 客户端、同一组额外请求头、
    /// 同一 `RetryPolicy`、同一 HTTP 状态码语义分类（[`classify_http_status`]）
    /// ——**不新增第二套请求封装层**。
    async fn send_embedding_request(
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
            let raw = format!("嵌入服务错误：嵌入请求失败：{e}");
            let message = if e.is_timeout() {
                TianyanError::timeout(raw).to_string()
            } else {
                raw
            };
            RetryableFailure { retryable, message }
        })?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let detail: String = detail.chars().take(300).collect();
            let retryable = crate::model::retry::should_retry_http(status);
            let raw = format!("嵌入服务错误：嵌入请求失败（HTTP {status}）：{detail}");
            return Err(RetryableFailure {
                retryable,
                message: classify_http_status(status, &raw),
            });
        }
        Ok(response)
    }
}

#[async_trait]
impl EmbeddingService for AsyncOpenAIClient {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));

        // 请求体手工构造（绕开 async-openai 类型，理由见 RawEmbeddingResponse 文档）；
        // wire 字段与既有类型化请求保持一致：model / input / dimensions（可选）。
        let mut body = serde_json::json!({
            "model": request.model,
            "input": match &request.input {
                EmbeddingInput::Single(text) => Value::String(text.clone()),
                EmbeddingInput::Multiple(texts) => Value::Array(
                    texts.iter().map(|t| Value::String(t.clone())).collect()
                ),
            },
        });
        if let Some(dimensions) = request.dimensions {
            body["dimensions"] = Value::Number(dimensions.into());
        }

        // 重试复用请求层单点（全局 RetryPolicy，与 chat 同款）。
        let response = crate::model::retry::with_retry(
            &self.retry_policy,
            |e: &RetryableFailure| e.retryable,
            || self.send_embedding_request(&url, &body),
        )
        .await
        .map_err(|e| TianyanError::Custom(e.message))?;

        let raw: RawEmbeddingResponse = response
            .json()
            .await
            .map_err(|e| TianyanError::Custom(format!("嵌入服务错误：解析响应失败：{e}")))?;

        let data = raw
            .data
            .into_iter()
            .map(|d| EmbeddingData {
                index: d.index,
                embedding: d.embedding,
                object: d.object,
            })
            .collect();

        Ok(EmbeddingResponse {
            object: raw.object,
            data,
            model: raw.model,
            usage: embedding_usage(&raw.usage, self.dialect.embedding_usage),
        })
    }

    fn embedding_dimension(&self, model: &str) -> usize {
        crate::model::types::embedding_dimension(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    /// 本地 mock HTTP 服务器：把收到的请求体累积到 `captured`，随后返回给定 JSON。
    fn spawn_mock(
        listener: std::net::TcpListener,
        body: String,
        captured: Arc<Mutex<String>>,
    ) -> std::net::SocketAddr {
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let mut total = 0usize;
            // 循环读，直到 header 之后的 body 非空（TCP 可能分段投递）
            for _ in 0..4 {
                let n = stream.read(&mut buf[total..]).unwrap_or(0);
                total += n;
                let seen = String::from_utf8_lossy(&buf[..total]).to_string();
                if seen
                    .split_once("\r\n\r\n")
                    .is_some_and(|(_, body_part)| !body_part.is_empty())
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            captured
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..total]));

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

    /// 构造指向 mock 的客户端（provider 名决定方言：dashscope → DashScope 预设）。
    fn client(name: &str, addr: std::net::SocketAddr) -> AsyncOpenAIClient {
        let provider = crate::config::ProviderConfig {
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
        };
        AsyncOpenAIClient::from_provider(&provider).unwrap()
    }

    /// 事故回归：DashScope 只回 `usage.total_tokens`（无 `prompt_tokens`）。
    ///
    /// 判别力：旧路径（async-openai 类型化反序列化）在此必红——
    /// `missing field 'prompt_tokens'` 使整次嵌入失败、向量被丢弃。
    #[tokio::test]
    async fn test_embed_dashscope_total_only_usage() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2,0.3]}],"model":"text-embedding-v4","usage":{"total_tokens":17},"id":"req-1"}"#.to_string();
        let addr = spawn_mock(listener, body, Arc::clone(&captured));
        let client = client("dashscope", addr);

        let resp = client
            .embed(EmbeddingRequest::new("text-embedding-v4", "你好"))
            .await
            .expect("DashScope 形状必须可解析（缺 prompt_tokens 不得失败）");

        assert_eq!(resp.model, "text-embedding-v4");
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.usage.total_tokens, 17);
        assert_eq!(
            resp.usage.prompt_tokens, 17,
            "缺 prompt_tokens 时应以 total_tokens 兜底"
        );
        assert_eq!(resp.usage.completion_tokens, 0, "嵌入无 completion 侧");

        let sent = captured.lock().unwrap().clone();
        assert!(sent.contains("\"model\":\"text-embedding-v4\""), "{sent}");
        assert!(sent.contains("\"input\":\"你好\""), "{sent}");
    }

    /// 标准形状（prompt + total）不回归。
    #[tokio::test]
    async fn test_embed_openai_standard_usage() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[1.0]}],"model":"text-embedding-3-small","usage":{"prompt_tokens":8,"total_tokens":8}}"#.to_string();
        let addr = spawn_mock(listener, body, captured);
        let client = client("openai", addr);

        let resp = client
            .embed(EmbeddingRequest::new("text-embedding-3-small", "hi"))
            .await
            .expect("标准形状必须可解析");
        assert_eq!(resp.usage.prompt_tokens, 8);
        assert_eq!(resp.usage.total_tokens, 8);
    }

    /// usage 对象整体缺失：宽容为 0，不因非关键字段缺失而丢向量。
    #[tokio::test]
    async fn test_embed_missing_usage_is_tolerated() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let body =
            r#"{"object":"list","data":[{"index":0,"embedding":[0.5]}],"model":"m"}"#.to_string();
        let addr = spawn_mock(listener, body, captured);
        let client = client("mock", addr);

        let resp = client
            .embed(EmbeddingRequest::new("m", "x"))
            .await
            .expect("缺 usage 不得导致整次嵌入失败");
        assert_eq!(resp.usage.total_tokens, 0);
        assert_eq!(resp.data[0].embedding, vec![0.5]);
    }

    /// 批量输入与 dimensions 的 wire 形状（手工构造请求体不得偏离既有类型化请求）。
    #[tokio::test]
    async fn test_embed_batch_input_and_dimensions_wire_shape() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let body =
            r#"{"object":"list","data":[],"model":"m","usage":{"total_tokens":3}}"#.to_string();
        let addr = spawn_mock(listener, body, Arc::clone(&captured));
        let client = client("mock", addr);

        let request = EmbeddingRequest::new_batch("m", vec!["a".to_string(), "b".to_string()])
            .with_dimensions(1024);
        client.embed(request).await.expect("批量请求应成功");

        let sent = captured.lock().unwrap().clone();
        assert!(sent.contains("\"input\":[\"a\",\"b\"]"), "{sent}");
        assert!(sent.contains("\"dimensions\":1024"), "{sent}");
    }

    /// 成因锁定：async-openai 0.34 的类型化路径在 DashScope 形状上**必然失败**
    /// （`usage.prompt_tokens` 为必填字段）。
    ///
    /// 该测试把「为何必须绕开库类型」固化为可验证事实。若将来上游把该字段改为
    /// 可选（本断言转红），即可据此评估移除本模块的手写解析。
    #[tokio::test]
    async fn test_upstream_typed_path_fails_on_dashscope_usage() {
        use async_openai::types::embeddings::{
            CreateEmbeddingRequestArgs, EmbeddingInput as OaInput,
        };

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1]}],"model":"text-embedding-v4","usage":{"total_tokens":17}}"#.to_string();
        let addr = spawn_mock(listener, body, captured);
        let client = client("dashscope", addr);

        let mut builder = CreateEmbeddingRequestArgs::default();
        builder.model("text-embedding-v4");
        builder.input(OaInput::String("你好".to_string()));
        let request = builder.build().unwrap();

        let err = client
            .client
            .embeddings()
            .create(request)
            .await
            .expect_err("库类型化路径必须在 DashScope 形状上失败（手写解析的存在理由）");
        let message = format!("{err}");
        assert!(
            message.contains("prompt_tokens"),
            "失败原因应为缺 prompt_tokens 字段，实际：{message}"
        );
    }

    /// **OCP 判别测试**：假想 provider「零 Rust 代码改动」接入。
    ///
    /// 场景：自建网关（端点与名称均嗅探不到任何预设）+ 仅靠配置声明方言与
    /// 模型级参数形态。断言请求侧（思考参数形态）与响应侧（非标准 usage 解析）
    /// 都按配置生效 —— 即「新增服务商 = 加配置，不改代码」这一目标成立。
    ///
    /// 判别力：若方言没有真正驱动请求 / 响应路径（例如仍有散落的硬编码嗅探），
    /// 本测试必红。
    #[tokio::test]
    async fn test_ocp_hypothetical_provider_zero_code_adoption() {
        use crate::config::{DialectPreset, ModelEntry, ProviderConfig, ThinkingParam};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        // 假想厂商：嵌入响应只回 total_tokens（非标准 usage 形状）
        let body = r#"{"object":"list","data":[{"index":0,"embedding":[0.5]}],"model":"qwen3-max","usage":{"total_tokens":9}}"#.to_string();
        let addr = spawn_mock(listener, body, captured);

        // —— 以下全部是配置，无任何 Rust 代码改动 ——
        let provider = ProviderConfig {
            name: "my-vendor-gw".to_string(),
            endpoint: format!("http://{addr}/v1"),
            api_key: Some("k".to_string()),
            models: vec![ModelEntry {
                name: "qwen3-max".to_string(),
                thinking_param: Some(ThinkingParam::ReasoningEffort),
                ..Default::default()
            }],
            timeout: 5,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: Some(DialectPreset::DashScope),
        };
        let client = AsyncOpenAIClient::from_provider(&provider).unwrap();

        // 响应侧：非标准 usage 形状可解析（方言声明 + 宽容解析）
        let resp = client
            .embed(EmbeddingRequest::new("qwen3-max", "x"))
            .await
            .expect("假想 provider 的非标准 usage 必须可解析");
        assert_eq!(resp.usage.total_tokens, 9);
        assert_eq!(
            resp.usage.prompt_tokens, 9,
            "缺 prompt_tokens 时以 total 兜底"
        );

        // 请求侧：模型级参数形态覆盖生效（模型名含 qwen 却走 reasoning_effort）
        assert_eq!(
            client.thinking_param_for("qwen3-max"),
            ThinkingParam::ReasoningEffort
        );
        // provider 级方言声明生效（缓存字段来自 DashScope 预设）
        assert_eq!(
            client.dialect.cache_field,
            crate::config::CacheField::DashScopeTop
        );
    }
}
