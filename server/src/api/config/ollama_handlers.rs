//! Ollama 模型发现 HTTP 处理函数。
//!
//! 提供 Ollama 本地实例的连接测试、模型扫描与将模型注册进配置的能力。
//! 纯代理逻辑：test/scan 直接调用 Ollama HTTP API，add-model 持久化到配置。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::api::config::services::ConfigService;
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// Ollama 服务默认端点。
const DEFAULT_OLLAMA_ENDPOINT: &str = "http://localhost:11434";

/// 构造 Ollama 客户端创建错误（统一错误前缀）。
fn ollama_client_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("创建 HTTP 客户端失败: {e}"))
}

/// 构造 Ollama 响应解析错误（统一错误前缀）。
fn ollama_parse_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("解析 Ollama 响应失败: {e}"))
}

/// 携带可选端点的请求体。
#[derive(Debug, Deserialize)]
pub struct OllamaEndpointRequest {
    /// Ollama 服务地址，缺省使用 localhost:11434。
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// 单个 Ollama 模型信息。
#[derive(Debug, Serialize)]
pub struct OllamaModelInfo {
    /// 模型名称。
    pub name: String,
    /// 模型大小（字节数，由前端格式化展示）。
    pub size: String,
    /// 能力标签。
    pub capabilities: Vec<String>,
}

/// 扫描结果响应。
#[derive(Debug, Serialize)]
pub struct OllamaScanResponse {
    /// 是否成功。
    pub success: bool,
    /// 发现的模型列表。
    pub models: Vec<OllamaModelInfo>,
    /// 失败原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 连接测试响应。
#[derive(Debug, Serialize)]
pub struct OllamaTestResponse {
    /// 是否成功。
    pub success: bool,
    /// Ollama 版本号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 失败原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 将模型注册进配置的请求体。
#[derive(Debug, Deserialize)]
pub struct AddOllamaModelRequest {
    /// Ollama 服务地址。
    #[serde(default)]
    pub endpoint: Option<String>,
    /// 模型名称。
    pub model_name: String,
    /// 能力标签（前端回传的扫描结果）。
    #[serde(default)]
    pub capabilities: Vec<String>,
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

/// 归一化 Ollama 端点 URL。
fn normalize_endpoint(raw: Option<&str>) -> String {
    let endpoint = raw.unwrap_or(DEFAULT_OLLAMA_ENDPOINT).trim();
    if endpoint.is_empty() {
        DEFAULT_OLLAMA_ENDPOINT.to_string()
    } else {
        endpoint.trim_end_matches('/').to_string()
    }
}

/// 从模型名推断能力标签（Ollama 均为对话模型，视觉模型按命名约定识别）。
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

/// POST /api/v1/config/ollama/test — 测试 Ollama 连接。
pub async fn test_ollama_connection(
    Json(request): Json<OllamaEndpointRequest>,
) -> Result<Json<OllamaTestResponse>, ApiError> {
    let endpoint = normalize_endpoint(request.endpoint.as_deref());
    let url = format!("{}/api/version", endpoint);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(ollama_client_error)?;

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.map_err(ollama_parse_error)?;
            let version = body
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            info!(endpoint = %endpoint, version = ?version, "Ollama 连接成功");
            Ok(Json(OllamaTestResponse {
                success: true,
                version,
                error: None,
            }))
        }
        Ok(resp) => {
            warn!(endpoint = %endpoint, status = resp.status().as_u16(), "Ollama 连接失败");
            Ok(Json(OllamaTestResponse {
                success: false,
                version: None,
                error: Some(format!("Ollama 服务返回状态码 {}", resp.status().as_u16())),
            }))
        }
        Err(e) => {
            warn!(endpoint = %endpoint, error = %e, "Ollama 连接失败");
            Ok(Json(OllamaTestResponse {
                success: false,
                version: None,
                error: Some(format!("无法连接 Ollama 服务: {}", e)),
            }))
        }
    }
}

/// POST /api/v1/config/ollama/scan — 扫描 Ollama 已下载的模型。
pub async fn scan_ollama_models(
    Json(request): Json<OllamaEndpointRequest>,
) -> Result<Json<OllamaScanResponse>, ApiError> {
    let endpoint = normalize_endpoint(request.endpoint.as_deref());
    let url = format!("{}/api/tags", endpoint);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(ollama_client_error)?;

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: OllamaTagsResponse = resp.json().await.map_err(ollama_parse_error)?;
            let models: Vec<OllamaModelInfo> = body
                .models
                .into_iter()
                .map(|m| {
                    let mut size_desc = m.size.to_string();
                    // 汇总大小字节数为可读字符串：>=1GB 显示 GB，>=1MB 显示 MB
                    if m.size >= 1_000_000_000 {
                        size_desc = format!("{:.1} GB", m.size as f64 / 1e9);
                    } else if m.size >= 1_000_000 {
                        size_desc = format!("{:.1} MB", m.size as f64 / 1e6);
                    }
                    let capabilities = infer_capabilities(&m.name);
                    OllamaModelInfo {
                        name: m.name,
                        size: size_desc,
                        capabilities,
                    }
                })
                .collect();
            info!(endpoint = %endpoint, count = models.len(), "Ollama 模型扫描完成");
            Ok(Json(OllamaScanResponse {
                success: true,
                models,
                error: None,
            }))
        }
        Ok(resp) => {
            warn!(endpoint = %endpoint, status = resp.status().as_u16(), "Ollama 扫描失败");
            Ok(Json(OllamaScanResponse {
                success: false,
                models: vec![],
                error: Some(format!("Ollama 服务返回状态码 {}", resp.status().as_u16())),
            }))
        }
        Err(e) => {
            warn!(endpoint = %endpoint, error = %e, "Ollama 扫描失败");
            Ok(Json(OllamaScanResponse {
                success: false,
                models: vec![],
                error: Some(format!("无法连接 Ollama 服务: {}", e)),
            }))
        }
    }
}

/// POST /api/v1/config/ollama/add-model — 将扫描到的模型注册进模型配置。
pub async fn add_ollama_model(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddOllamaModelRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.model_name.trim().is_empty() {
        return Err(ApiError::BadRequest("模型名称不能为空".to_string()));
    }

    let endpoint = normalize_endpoint(request.endpoint.as_deref());
    let service = ConfigService::new(state);

    service
        .add_ollama_model(&endpoint, &request.model_name, &request.capabilities)
        .await
        .map(|_| {
            Json(serde_json::json!({
                "success": true,
                "message": format!("模型 '{}' 已添加到配置", request.model_name)
            }))
        })
        .map_err(|e| {
            error!("添加 Ollama 模型 '{}' 失败: {}", request.model_name, e);
            e
        })
}
