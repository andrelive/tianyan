//! 统一 HTTP 客户端工厂。
//!
//! 全库 reqwest 客户端构造的单点：超时 / 连接超时 / 连接池 / UA 策略一处定义。
//! 禁止在模块内复制 `reqwest::Client::builder()` 样板；构建失败上抛（不静默降级）。

use std::time::Duration;

use crate::common::error::{Result, TianyanError};

/// HTTP 客户端规格（构造点的公共参数）。
pub struct HttpClientSpec {
    /// 请求超时。
    pub timeout: Duration,
    /// 连接超时。
    pub connect_timeout: Duration,
    /// User-Agent（None 用 reqwest 默认）。
    pub user_agent: Option<String>,
}

/// 构建 HTTP 客户端（统一连接池策略：每主机空闲连接 5、空闲超时 90s）。
pub fn build_http_client(spec: &HttpClientSpec) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(spec.timeout)
        .connect_timeout(spec.connect_timeout)
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(90));
    if let Some(ua) = &spec.user_agent {
        builder = builder.user_agent(ua);
    }
    builder
        .build()
        .map_err(|e| TianyanError::config(format!("构建 HTTP 客户端失败: {e}")))
}
