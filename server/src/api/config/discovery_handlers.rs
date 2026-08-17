//! Provider 发现与引导 HTTP 处理函数。
//!
//! 将原 Ollama 专属模型发现 API 泛化为通用 Provider 发现/引导：
//! 任意支持 OpenAI 兼容协议（`/models`）或 Ollama 原生协议（`/api/*`）
//! 的 provider 均可完成连接测试、模型扫描与配置注册。Ollama 仅为
//! protocol 取值之一，不再是独立端点组。
//! 纯代理逻辑：test/scan 直接调用 provider HTTP API，add-model 持久化到配置。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::api::config::services::ConfigService;
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// Ollama 服务默认端点（仅 ollama 协议使用）。
const DEFAULT_OLLAMA_ENDPOINT: &str = "http://localhost:11434";

/// 构造 HTTP 客户端创建错误（统一错误前缀）。
fn http_client_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("创建 HTTP 客户端失败: {e}"))
}

/// 构造 Provider 响应解析错误（统一错误前缀）。
fn provider_parse_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("解析 Provider 响应失败: {e}"))
}

/// 携带端点与协议的请求体（test/scan 共用）。
#[derive(Debug, Deserialize)]
pub struct ProviderEndpointRequest {
    /// Provider 服务地址，缺省按协议回落（ollama → localhost:11434）。
    #[serde(default)]
    pub endpoint: Option<String>,
    /// 协议："openai"（缺省）或 "ollama"。
    #[serde(default)]
    pub protocol: Option<String>,
    /// API 密钥（openai 协议连接测试时可选携带）。
    #[serde(default)]
    pub api_key: Option<String>,
    /// Provider 名称（scan 可选携带：命中内置目录时零网络直接返回目录模型）。
    #[serde(default)]
    pub provider: Option<String>,
}

/// 单个 Provider 模型信息。
#[derive(Debug, Serialize)]
pub struct ProviderModelInfo {
    /// 模型名称。
    pub name: String,
    /// 模型大小描述（openai 协议无此字段，序列化时省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// 能力标签。
    pub capabilities: Vec<String>,
    /// 显示名（端点/目录提供时下发；未提供时缺省，前端以 name 兜底显示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 上下文窗口长度（token；端点/内置目录提供时下发，供 adopt 预填模型配置）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    /// 单次最大输出 token 数（端点/内置目录提供时下发，供 adopt 预填模型配置）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// 内置目录默认思考档位（仅目录扫描返回；advisory，UI 自动附加 "off"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
}

/// 扫描结果响应。
#[derive(Debug, Serialize)]
pub struct ProviderScanResponse {
    /// 是否成功。
    pub success: bool,
    /// 发现的模型列表。
    pub models: Vec<ProviderModelInfo>,
    /// 失败原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 连接测试响应。
#[derive(Debug, Serialize)]
pub struct ProviderTestResponse {
    /// 是否成功。
    pub success: bool,
    /// Provider 版本号（openai 协议无版本概念，恒为 null）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 失败原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 将模型注册进配置的请求体。
#[derive(Debug, Deserialize)]
pub struct AddProviderModelRequest {
    /// Provider 名称。
    pub provider_name: String,
    /// Provider 服务地址。
    pub endpoint: String,
    /// 模型名称。
    pub model_name: String,
    /// 能力标签（前端回传的扫描结果）。
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// OpenAI 兼容 `/models` 的响应结构。
#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    /// 显示名（OpenRouter 等网关的扩展字段）。
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    /// 上下文窗口（OpenRouter/Together 等网关扩展字段）。
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    context_length: Option<u64>,
    /// 单次最大输出 token 数（扩展字段）。
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
}

/// Ollama `/api/tags` 的响应结构。
#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaTagModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagModel {
    name: String,
    #[serde(default)]
    size: u64,
}

/// 解析请求中的协议字段：缺省/空为 "openai"，非法值返回 400。
fn resolve_protocol(protocol: Option<&str>) -> Result<&'static str, ApiError> {
    match protocol.map(str::trim).unwrap_or("openai") {
        "" | "openai" => Ok("openai"),
        "ollama" => Ok("ollama"),
        other => Err(ApiError::BadRequest(format!(
            "不支持的协议 '{other}'，合法值为 openai / ollama"
        ))),
    }
}

/// 归一化 Provider 端点 URL；ollama 协议空值回落本地默认端点。
fn normalize_endpoint(raw: Option<&str>, protocol: &str) -> String {
    let endpoint = raw.unwrap_or("").trim();
    if endpoint.is_empty() {
        if protocol == "ollama" {
            DEFAULT_OLLAMA_ENDPOINT.to_string()
        } else {
            String::new()
        }
    } else {
        endpoint.trim_end_matches('/').to_string()
    }
}

/// 将模型大小字节数格式化为可读字符串（>=1GB 显示 GB，>=1MB 显示 MB，否则原样）。
fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        bytes.to_string()
    }
}

/// 从模型名推断能力标签（vision/llava/vl/vlm/qwen2.5-vl 命名约定，
/// 两种协议通用——openai 兼容的本地模型同样适用）；默认输出 ["chat"]。
fn infer_capabilities(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut caps = vec!["chat".to_string()];
    if ["vision", "llava", "vl", "vlm", "qwen2.5-vl"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        caps.push("vision".to_string());
    }
    caps
}

/// POST /api/v1/config/providers/test — 测试任意 Provider 连接。
pub async fn test_provider_connection(
    Json(request): Json<ProviderEndpointRequest>,
) -> Result<Json<ProviderTestResponse>, ApiError> {
    let protocol = resolve_protocol(request.protocol.as_deref())?;
    let endpoint = normalize_endpoint(request.endpoint.as_deref(), protocol);
    let url = if protocol == "ollama" {
        format!("{endpoint}/api/version")
    } else {
        format!("{endpoint}/models")
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(http_client_error)?;

    let mut req = client.get(&url);
    if protocol == "openai" {
        if let Some(key) = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let version = if protocol == "ollama" {
                let body: serde_json::Value = resp.json().await.map_err(provider_parse_error)?;
                body.get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            };
            info!(endpoint = %endpoint, protocol, version = ?version, "Provider 连接成功");
            Ok(Json(ProviderTestResponse {
                success: true,
                version,
                error: None,
            }))
        }
        Ok(resp) => {
            warn!(
                endpoint = %endpoint,
                protocol,
                status = resp.status().as_u16(),
                "Provider 连接失败"
            );
            Ok(Json(ProviderTestResponse {
                success: false,
                version: None,
                error: Some(format!(
                    "Provider 服务返回状态码 {}",
                    resp.status().as_u16()
                )),
            }))
        }
        Err(e) => {
            warn!(endpoint = %endpoint, protocol, error = %e, "Provider 连接失败");
            Ok(Json(ProviderTestResponse {
                success: false,
                version: None,
                error: Some(format!("无法连接 Provider 服务: {e}")),
            }))
        }
    }
}

/// POST /api/v1/config/providers/scan — 扫描 Provider 可用模型。
pub async fn scan_provider_models(
    Json(request): Json<ProviderEndpointRequest>,
) -> Result<Json<ProviderScanResponse>, ApiError> {
    let protocol = resolve_protocol(request.protocol.as_deref())?;
    let endpoint = normalize_endpoint(request.endpoint.as_deref(), protocol);
    let url = if protocol == "ollama" {
        format!("{endpoint}/api/tags")
    } else {
        format!("{endpoint}/models")
    };

    // 内置目录零网络路径：协议为 openai 且 provider 名命中内置目录时，
    // 直接返回目录模型（规格 + 默认档位），不发起网络请求。
    if protocol == "openai" {
        let catalog = tianyan::model::spec::builtin_catalog_models(
            request.provider.as_deref().unwrap_or("").trim(),
        );
        if !catalog.is_empty() {
            let models: Vec<ProviderModelInfo> = catalog
                .into_iter()
                .map(|m| ProviderModelInfo {
                    name: m.name.to_string(),
                    size: None,
                    capabilities: infer_capabilities(m.name),
                    display_name: m.display_name.map(str::to_string),
                    context_length: Some(m.spec.context_length as u64),
                    max_output_tokens: Some(m.spec.max_output_tokens as u64),
                    reasoning_efforts: m
                        .reasoning_efforts
                        .map(|efforts| efforts.iter().map(|s| s.to_string()).collect()),
                })
                .collect();
            info!(
                provider = %request.provider.as_deref().unwrap_or(""),
                count = models.len(),
                "内置目录命中，跳过网络扫描"
            );
            return Ok(Json(ProviderScanResponse {
                success: true,
                models,
                error: None,
            }));
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(http_client_error)?;

    let mut req = client.get(&url);
    if protocol == "openai" {
        if let Some(key) = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let models: Vec<ProviderModelInfo> = if protocol == "ollama" {
                let body: OllamaTagsResponse = resp.json().await.map_err(provider_parse_error)?;
                body.models
                    .into_iter()
                    .map(|m| ProviderModelInfo {
                        capabilities: infer_capabilities(&m.name),
                        name: m.name,
                        size: Some(format_size(m.size)),
                        display_name: None,
                        context_length: None,
                        max_output_tokens: None,
                        reasoning_efforts: None,
                    })
                    .collect()
            } else {
                let body: OpenAiModelsResponse = resp.json().await.map_err(provider_parse_error)?;
                body.data
                    .into_iter()
                    .map(|m| ProviderModelInfo {
                        capabilities: infer_capabilities(&m.id),
                        name: m.id.clone(),
                        size: None,
                        display_name: m.display_name.or(m.name).or(Some(m.id)),
                        // 网关扩展字段：context_window/context_length、
                        // max_output_tokens/max_tokens（OpenRouter/Together 等提供）
                        context_length: m.context_window.or(m.context_length),
                        max_output_tokens: m.max_output_tokens.or(m.max_tokens),
                        reasoning_efforts: None,
                    })
                    .collect()
            };
            info!(
                endpoint = %endpoint,
                protocol,
                count = models.len(),
                "Provider 模型扫描完成"
            );
            Ok(Json(ProviderScanResponse {
                success: true,
                models,
                error: None,
            }))
        }
        Ok(resp) => {
            warn!(
                endpoint = %endpoint,
                protocol,
                status = resp.status().as_u16(),
                "Provider 扫描失败"
            );
            Ok(Json(ProviderScanResponse {
                success: false,
                models: vec![],
                error: Some(format!(
                    "Provider 服务返回状态码 {}",
                    resp.status().as_u16()
                )),
            }))
        }
        Err(e) => {
            warn!(endpoint = %endpoint, protocol, error = %e, "Provider 扫描失败");
            Ok(Json(ProviderScanResponse {
                success: false,
                models: vec![],
                error: Some(format!("无法连接 Provider 服务: {e}")),
            }))
        }
    }
}

/// POST /api/v1/config/providers/add-model — 将扫描到的模型注册进 Provider 配置。
pub async fn add_provider_model(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddProviderModelRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.provider_name.trim().is_empty() {
        return Err(ApiError::BadRequest("Provider 名称不能为空".to_string()));
    }
    if request.model_name.trim().is_empty() {
        return Err(ApiError::BadRequest("模型名称不能为空".to_string()));
    }

    let service = ConfigService::new(state);

    service
        .add_provider_model(
            &request.provider_name,
            &request.endpoint,
            &request.model_name,
            &request.capabilities,
        )
        .await
        .map(|_| {
            Json(serde_json::json!({
                "success": true,
                "message": format!("模型 '{}' 已添加到配置", request.model_name)
            }))
        })
        .map_err(|e| {
            error!(
                provider = %request.provider_name,
                model = %request.model_name,
                "添加 Provider 模型 '{}' 失败: {}",
                request.model_name,
                e
            );
            e
        })
}
