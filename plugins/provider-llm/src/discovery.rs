use async_trait::async_trait;
use serde_json::Value;

use tianyan::common::error::{Result, TianyanError};
use tianyan::model::types::{ModelCapability, ModelInfo, ModelType};
use tianyan::model::ServiceDiscovery;

use crate::client::ChatClient;

const MODEL_PREFIXES: &[&str] = &["gpt", "text-embedding", "claude", "deepseek", "qwen"];
const DEFAULT_CHAT_CONTEXT_LENGTH: usize = 8192;

/// `GET /models`，返回原始 JSON。
///
/// 不复用 `send_chat_request`：那是 **POST 专用单点**（`http.post(url).json(body)`），
/// 为一次模型目录查询给它加一条方法分支不划算。模型目录本身是**可选能力**——
/// 部分 OpenAI 兼容网关不实现该端点，失败只影响「发现」，不影响对话。
async fn fetch_models_json(client: &ChatClient) -> Result<Value> {
    let url = format!("{}/models", client.base_url.trim_end_matches('/'));
    let mut req = client.http.get(&url);
    if !client.api_key.is_empty() {
        req = req.bearer_auth(&client.api_key);
    }
    for (name, value) in &client.headers {
        if let (Ok(header), Ok(header_value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            req = req.header(header, header_value);
        }
    }
    let response = req
        .send()
        .await
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：列出模型失败：{e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(TianyanError::Custom(format!(
            "模型服务错误：列出模型失败：HTTP {status}"
        )));
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| TianyanError::Custom(format!("模型服务错误：解析模型目录失败：{e}")))
}

/// 从模型目录 JSON 提取模型 id（`data[].id` 缺失或非字符串即跳过）。
fn model_ids(value: &Value) -> Vec<String> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait]
impl ServiceDiscovery for ChatClient {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let value = fetch_models_json(self).await?;

        let models: Vec<ModelInfo> = model_ids(&value)
            .into_iter()
            .filter(|id| MODEL_PREFIXES.iter().any(|prefix| id.starts_with(prefix)))
            .map(|id| {
                let model_type = if id.contains("embedding") {
                    ModelType::Embedding
                } else if id.contains("vision") || id.contains("gpt-4") {
                    ModelType::Vision
                } else {
                    ModelType::Chat
                };

                let capabilities = match model_type {
                    ModelType::Chat => {
                        vec![ModelCapability::Chat, ModelCapability::Streaming]
                    }
                    ModelType::Embedding => vec![],
                    ModelType::Vision => vec![
                        ModelCapability::Chat,
                        ModelCapability::Vision,
                        ModelCapability::Streaming,
                    ],
                };

                ModelInfo {
                    name: id.clone(),
                    id,
                    provider: self.service_name.clone(),
                    model_type,
                    max_context_length: DEFAULT_CHAT_CONTEXT_LENGTH,
                    capabilities,
                }
            })
            .collect();

        Ok(models)
    }

    async fn is_available(&self) -> bool {
        fetch_models_json(self).await.is_ok()
    }

    fn service_name(&self) -> &str {
        &self.service_name
    }
}
