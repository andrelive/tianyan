use gloo_net::http::{Request, Response};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

pub mod chat;
pub mod config;
pub mod knowledge;
pub mod sessions;
pub mod skills;

const DEFAULT_API_BASE: &str = "http://localhost:3000/api/v1";
/// 默认请求超时时间（秒）。
const DEFAULT_TIMEOUT_SECS: u32 = 60;

pub(super) fn get_api_base() -> String {
    thread_local! {
        static CACHED_API_BASE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    }
    CACHED_API_BASE.with(|cell| {
        if let Some(ref base) = *cell.borrow() {
            return base.clone();
        }
        let base = web_sys::window()
            .and_then(|w| w.location().href().ok())
            .map(|href| {
                if href.contains("localhost") || href.contains("127.0.0.1") {
                    DEFAULT_API_BASE.to_string()
                } else {
                    "/api/v1".to_string()
                }
            })
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        *cell.borrow_mut() = Some(base.clone());
        base
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
    pub code: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "API Error: {}", self.message)
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
}

pub(super) fn parse_error_body(status: u16, text: &str) -> ApiError {
    if let Ok(err_body) = serde_json::from_str::<ErrorResponse>(text) {
        ApiError {
            message: err_body.error,
            code: Some(status.to_string()),
        }
    } else {
        ApiError {
            message: format!("HTTP {}: {}", status, text.trim()),
            code: Some(status.to_string()),
        }
    }
}

/// 处理 HTTP 响应，解析 JSON 或提取错误信息
async fn handle_response<T>(response: Response) -> ApiResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if response.ok() {
        response.json().await.map_err(|e| ApiError {
            message: format!("解析响应失败: {}", e),
            code: None,
        })
    } else {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Err(parse_error_body(status, &text))
    }
}

/// 根据 HTTP 状态码和文本解析响应（用于原始 web_sys fetch 路径）
pub(super) fn parse_response_text<T>(status: u16, text: &str) -> ApiResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if (200..300).contains(&status) {
        serde_json::from_str(text).map_err(|e| ApiError {
            message: format!("解析响应失败: {}", e),
            code: None,
        })
    } else {
        Err(parse_error_body(status, text))
    }
}

pub async fn get<T>(path: &str) -> ApiResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let url = format!("{}{}", get_api_base(), path);

    let response = with_timeout(
        Request::get(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|e| ApiError {
        message: format!("请求失败: {}", e),
        code: None,
    })?;

    handle_response(response).await
}

pub async fn post<T, B>(path: &str, body: &B) -> ApiResult<T>
where
    T: for<'de> Deserialize<'de>,
    B: Serialize,
{
    let url = format!("{}{}", get_api_base(), path);

    let response = with_timeout(
        Request::post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(body)
            .map_err(|e| ApiError {
                message: format!("序列化请求失败: {}", e),
                code: None,
            })?
            .send(),
    )
    .await
    .map_err(|e| ApiError {
        message: format!("请求失败: {}", e),
        code: None,
    })?;

    handle_response(response).await
}

pub async fn delete<T>(path: &str) -> ApiResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let url = format!("{}{}", get_api_base(), path);

    let response = with_timeout(
        Request::delete(&url)
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|e| ApiError {
        message: format!("请求失败: {}", e),
        code: None,
    })?;

    handle_response(response).await
}

/// 为 WASM HTTP 请求添加超时控制。
/// 使用 gloo_timers 的 TimeoutFuture + futures::future::select 实现。
async fn with_timeout<F, T>(fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, gloo_net::Error>> + 'static,
    T: 'static,
{
    let result = futures::future::select(
        Box::pin(fut),
        Box::pin(async {
            gloo_timers::future::TimeoutFuture::new(DEFAULT_TIMEOUT_SECS * 1000).await;
            Err::<T, String>("请求超时".to_string())
        }),
    )
    .await;
    match result {
        futures::future::Either::Left((resp, _)) => resp.map_err(|e| e.to_string()),
        futures::future::Either::Right((err, _)) => err,
    }
}

/// 为 SSE 流请求添加超时控制。
///
/// 在 WASM 环境中使用 `AbortController` + `setTimeout` 实现请求超时。
pub(super) fn create_abort_controller_with_timeout(
    timeout_secs: u32,
) -> (web_sys::AbortController, web_sys::AbortSignal) {
    let controller = web_sys::AbortController::new().expect("AbortController::new 不应失败");
    let signal = controller.signal();

    let controller_clone = controller.clone();
    let timeout_ms = (timeout_secs as f64) * 1000.0;
    let closure = Closure::once_into_js(move || {
        controller_clone.abort();
    });

    web_sys::window().and_then(|w| {
        w.set_timeout_with_callback_and_timeout_and_arguments_0(
            closure
                .as_ref()
                .dyn_ref::<js_sys::Function>()
                .expect("closure should be castable to js_sys::Function"),
            timeout_ms as i32,
        )
        .ok()
    });

    (controller, signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_display() {
        let err = ApiError {
            message: "测试错误".to_string(),
            status: 400,
        };
        assert_eq!(err.status, 400);
        assert!(err.message.contains("测试错误"));
    }

    #[test]
    fn test_get_api_base_default() {
        // 在 WASM 外，thread_local 应能工作
        let base = get_api_base();
        assert!(!base.is_empty());
    }

    #[test]
    fn test_default_api_base() {
        assert_eq!(DEFAULT_API_BASE, "http://localhost:3000/api/v1");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 60);
    }
}
