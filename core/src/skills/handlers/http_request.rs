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
        Self { client }
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

        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TianyanError::Custom("[http_request] 缺少 'url' 参数".to_string()))?;

        if let Ok(parsed) = url.parse::<reqwest::Url>() {
            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(TianyanError::Custom(format!(
                    "操作不被允许：不支持的 URL 协议: {}",
                    scheme
                )));
            }
            if let Some(host) = parsed.host_str() {
                // host_str() 返回原始切片：IPv6 带方括号（[::1]），先去掉再判断
                let lower = host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_lowercase();
                // IPv4/IPv6 回环、未指定地址与私网/链路本地地址一律禁止，
                // 防止 SSRF 访问本地服务（127.0.0.1、::1、10/8、172.16/12、192.168/16、
                // 0.0.0.0、fe80:: 链路本地、fc00::/7 唯一本地地址）。
                let is_ipv6_loopback = lower == "::1";
                let is_ipv6_unspecified = lower == "::";
                let is_ipv6_private =
                    lower.contains(':') && (lower.starts_with("fc") || lower.starts_with("fd"));
                let is_ipv6_link_local = lower.starts_with("fe80:");
                if lower == "localhost"
                    || lower.starts_with("127.")
                    || lower.starts_with("192.168.")
                    || lower.starts_with("10.")
                    || lower.starts_with("172.")
                    || lower.starts_with("0.")
                    || is_ipv6_loopback
                    || is_ipv6_unspecified
                    || is_ipv6_private
                    || is_ipv6_link_local
                {
                    return Err(TianyanError::Custom(
                        "操作不被允许：禁止访问本地或内网地址".to_string(),
                    ));
                }
            }
        } else {
            return Err(TianyanError::Custom(
                "[http_request] 无效的 URL 格式".to_string(),
            ));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params(url: &str) -> HashMap<String, Value> {
        HashMap::from([("url".to_string(), Value::String(url.to_string()))])
    }

    #[tokio::test]
    async fn test_missing_url_param() {
        let handler = HttpRequestHandler::new();
        let err = handler
            .execute(HashMap::new(), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("缺少 'url'"));
    }

    #[tokio::test]
    async fn test_invalid_url_rejected() {
        let handler = HttpRequestHandler::new();
        let err = handler
            .execute(params("not a url"), ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("无效的 URL 格式"),
            "无效 URL 应被拒绝: {err}"
        );
    }

    #[tokio::test]
    async fn test_non_http_scheme_rejected() {
        let handler = HttpRequestHandler::new();
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "gopher://example.com",
        ] {
            let err = handler
                .execute(params(url), ExecutionContext::default())
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("不支持的 URL 协议"),
                "{url} 应因协议被拒绝: {err}"
            );
        }
    }

    #[tokio::test]
    async fn test_local_and_private_hosts_rejected() {
        let handler = HttpRequestHandler::new();
        let blocked = [
            "http://localhost/x",
            "http://127.0.0.1/x",
            "http://127.0.0.2:8080/x",
            "http://192.168.1.1/x",
            "http://10.0.0.1/x",
            "http://172.16.0.1/x",
            "http://0.0.0.0/x",
            "http://[::1]/x",
        ];
        for url in blocked {
            match handler
                .execute(params(url), ExecutionContext::default())
                .await
            {
                Ok(_) => panic!("{url} 应被拒绝但执行成功了"),
                Err(e) => assert!(
                    e.to_string().contains("禁止访问本地或内网地址"),
                    "{url} 应因内网地址被拒绝: {e}"
                ),
            }
        }
    }

    #[tokio::test]
    async fn test_unsupported_method_rejected() {
        let handler = HttpRequestHandler::new();
        let mut p = params("https://example.com/x");
        p.insert("method".to_string(), Value::String("TRACE".to_string()));
        let err = handler
            .execute(p, ExecutionContext::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("不支持的 HTTP 方法"),
            "TRACE 应被拒绝: {err}"
        );
    }

    #[test]
    fn test_skill_id() {
        assert_eq!(HttpRequestHandler::new().skill_id(), "http_request");
    }
}
