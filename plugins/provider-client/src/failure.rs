//! 请求阶段的失败形状与 HTTP 状态码的**语义分类单点**。
//!
//! 这两个符号原在 `core/src/model/provider/chat.rs`（与聊天实现同文件），ADR-049 期 2
//! 序 6 随 provider 域搬出 core；它们不放在 `provider-llm` 而放在本基底 crate 的理由是
//! **两个能力插件都要用**：
//!
//! - `tianyan_provider_llm`：聊天（非流式 / 流式 / VLM）重试闭包里的 `retryable` 判定；
//! - `tianyan_provider_embed`：嵌入请求发送（`send_embedding_request`）复用同一分类。
//!
//! 若放在 `provider-llm`，`provider-embed` 就得反向依赖它——那是插件间依赖（D2-2）。
//! 逻辑本身逐字未改（ADR-014：分类契约单点，消费端只按语义谓词判定，不做文本匹配）。

use tianyan::common::error::TianyanError;

/// 请求发送阶段的失败（携带是否可重试的类别标记）；重试是请求层自我保护。
#[derive(Debug)]
pub struct RetryableFailure {
    /// 是否属于可重试的临时失败（HTTP 429 / 5xx / 网络 / 超时）。
    pub retryable: bool,
    /// 面向用户的错误文案（已按语义分类，见 [`classify_http_status`]）。
    pub message: String,
}

/// HTTP 状态码 → 语义分类（ADR-014：分类契约单点；流式请求的显式状态码）。
///
/// 401/403 → permission、404 → not_found、408/504 → timeout；其余返回原文。
pub fn classify_http_status(status: reqwest::StatusCode, raw: &str) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return TianyanError::permission(raw).to_string();
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return TianyanError::not_found(raw).to_string();
    }
    if status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::GATEWAY_TIMEOUT
    {
        return TianyanError::timeout(raw).to_string();
    }
    raw.to_string()
}
