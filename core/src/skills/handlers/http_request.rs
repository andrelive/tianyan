use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

use crate::common::error::{Result, TianyanError};

use crate::skills::definition::SkillHandler;
use crate::skills::types::{ExecutionContext, SkillExecutionResult};

/// HTTP 请求处理器。
pub struct HttpRequestHandler {
    client: reqwest::Client,
}

impl HttpRequestHandler {
    /// 创建新的 HTTP 请求处理器。
    pub fn new() -> Self {
        Self::with_timeout(60)
    }

    /// 使用自定义超时创建。
    pub fn with_timeout(secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(secs))
            .connect_timeout(Duration::from_secs(secs.min(15)))
            .pool_max_idle_per_host(5)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
        }
    }
}

impl Default for HttpRequestHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillHandler for HttpRequestHandler {
    async fn execute(
        &self,
        params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> Result<SkillExecutionResult> {
        let start = Instant::now();

        let url = params.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            TianyanError::Custom("[http_request] 缺少 'url' 参数".to_string())
        })?;

        if let Ok(parsed) = url.parse::<reqwest::Url>() {
            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(TianyanError::Custom(format!(
                    "操作不被允许：不支持的 URL 协议: {}",
                    scheme
                )));
            }
            if let Some(host) = parsed.host_str() {
                let lower = host.to_lowercase();
                if lower == "localhost"
                    || lower == "127.0.0.1"
                    || lower.starts_with("192.168.")
                    || lower.starts_with("10.")
                    || lower.starts_with("172.")
                    || lower.starts_with("0.")
                {
                    return Err(TianyanError::Custom(
                        "操作不被允许：禁止访问本地或内网地址".to_string(),
                    ));
                }
            }
        } else {
            return Err(TianyanError::Custom("[http_request] 无效的 URL 格式".to_string()));
        }

        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let headers = params.get("headers").and_then(|v| v.as_object()).cloned();

        let body = params.get("body").and_then(|v| v.as_str());

        let mut request = match method.to_uppercase().as_str() {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "DELETE" => self.client.delete(url),
            "PATCH" => self.client.patch(url),
            _ => {
                return Err(TianyanError::Custom(format!(
                    "[http_request] 不支持的 HTTP 方法: {}",
                    method
                )))
            }
        };

        if let Some(ref headers) = headers {
            for (key, value) in headers {
                if let Some(v) = value.as_str() {
                    request = request.header(key, v);
                }
            }
        }

        if let Some(body) = body {
            request = request.body(body.to_string());
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status();
                match response.text().await {
                    Ok(text) => Ok(SkillExecutionResult {
                        success: status.is_success(),
                        output: Some(text),
                        error: if status.is_success() {
                            None
                        } else {
                            Some(format!("HTTP {}", status))
                        },
                        exit_code: Some(status.as_u16() as i32),
                        execution_time_ms: start.elapsed().as_millis() as u64,
                        data: {
                            let mut data = HashMap::new();
                            data.insert(
                                "status_code".to_string(),
                                Value::Number(status.as_u16().into()),
                            );
                            data
                        },
                    }),
                    Err(e) => Ok(SkillExecutionResult {
                        success: false,
                        output: None,
                        error: Some(format!("读取响应失败: {}", e)),
                        exit_code: Some(status.as_u16() as i32),
                        execution_time_ms: start.elapsed().as_millis() as u64,
                        data: HashMap::new(),
                    }),
                }
            }
            Err(e) => Ok(SkillExecutionResult {
                success: false,
                output: None,
                error: Some(format!("HTTP 请求失败: {}", e)),
                exit_code: None,
                execution_time_ms: start.elapsed().as_millis() as u64,
                data: HashMap::new(),
            }),
        }
    }

    fn skill_id(&self) -> &str {
        "http_request"
    }
}
