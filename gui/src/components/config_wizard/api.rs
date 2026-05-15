//! 配置向导 API 调用

#![allow(dead_code)]

use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;

use super::types::{
    ConfigStatusResponse, SaveConfigRequest, SaveConfigResponse, TestConnectionRequest,
    TestConnectionResponse,
};

const API_BASE: &str = "http://localhost:3000/api";

/// 获取配置状态
pub async fn fetch_config_status() -> Result<ConfigStatusResponse, String> {
    let url = format!("{}/config/status", API_BASE);

    let response = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }

    response
        .json::<ConfigStatusResponse>()
        .await
        .map_err(|e| format!("解析响应失败: {}", e))
}

/// 保存配置
pub async fn save_config(request: &SaveConfigRequest) -> Result<SaveConfigResponse, String> {
    let url = format!("{}/config/wizard", API_BASE);

    let body = serde_json::to_string(request).map_err(|e| format!("序列化失败: {}", e))?;

    let request = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("构建请求失败: {}", e))?;

    let response = request
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }

    response
        .json::<SaveConfigResponse>()
        .await
        .map_err(|e| format!("解析响应失败: {}", e))
}

/// 测试模型连接
pub async fn test_connection(
    request: &TestConnectionRequest,
) -> Result<TestConnectionResponse, String> {
    let url = format!("{}/config/test-connection", API_BASE);

    let body = serde_json::to_string(request).map_err(|e| format!("序列化失败: {}", e))?;

    let request = Request::post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| format!("构建请求失败: {}", e))?;

    let response = request
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if !response.ok() {
        return Err(format!("HTTP 错误: {}", response.status()));
    }

    response
        .json::<TestConnectionResponse>()
        .await
        .map_err(|e| format!("解析响应失败: {}", e))
}

/// 异步获取配置状态（带回调）
pub fn fetch_config_status_async<F>(callback: F)
where
    F: FnOnce(Result<ConfigStatusResponse, String>) + 'static,
{
    spawn_local(async move {
        let result = fetch_config_status().await;
        callback(result);
    });
}

/// 异步保存配置（带回调）
pub fn save_config_async<F>(request: SaveConfigRequest, callback: F)
where
    F: FnOnce(Result<SaveConfigResponse, String>) + 'static,
{
    spawn_local(async move {
        let result = save_config(&request).await;
        callback(result);
    });
}

/// 异步测试连接（带回调）
pub fn test_connection_async<F>(request: TestConnectionRequest, callback: F)
where
    F: FnOnce(Result<TestConnectionResponse, String>) + 'static,
{
    spawn_local(async move {
        let result = test_connection(&request).await;
        callback(result);
    });
}
