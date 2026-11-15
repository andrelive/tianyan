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
    // 413 不归入既有语义变体（它不是"输入写错了"或"权限/超时"），但**必须**给出
    // 可执行的原因：裸的 "HTTP 413 Payload Too Large" 会被读成网络抖动，而真实
    // 原因是请求体本身超限。两个账本在这里分家——token 预算按模型窗口算且图片
    // 只折约 1,000 token/张，传输却按 base64 字节算（265 KB 的 PNG ≈ 354 KB）。
    // 实测：134 张截图 = base64 48 MB 撞墙时，`cached_input` 仅占 1M 窗口的 46.7%。
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return format!(
            "{raw}\n\
             提示：这是**请求体过大**，不是网络故障，重试同样会失败。图片按 base64 传输\
             （一张 265 KB 的 PNG ≈ 354 KB），同一轮内连续查看多张大图、或单条工具输出\
             过长都会触发。减少同一轮的图片数量后重试即可——历史图片已不再随每次请求\
             重发（ADR-050 修订），跨轮需要细节时用 `view_image` 重看。"
        );
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 413 必须点明"请求体过大"，否则现场会把体积问题误读成网络抖动。
    #[test]
    fn test_classify_413_explains_payload_not_network() {
        let raw = "模型服务错误：请求被拒绝（HTTP 413 Payload Too Large）：too large";
        let msg = classify_http_status(reqwest::StatusCode::PAYLOAD_TOO_LARGE, raw);
        assert!(msg.contains(raw), "原始错误必须保留（排查依据）：{msg}");
        assert!(
            msg.contains("请求体过大"),
            "必须点明是请求体体积问题：{msg}"
        );
        assert!(
            msg.contains("view_image"),
            "必须给出跨轮重看的具体手段：{msg}"
        );
    }

    /// 其余状态码的直通行为不受 413 分支影响。
    #[test]
    fn test_classify_other_statuses_passthrough() {
        assert_eq!(
            classify_http_status(reqwest::StatusCode::BAD_REQUEST, "boom"),
            "boom"
        );
    }
}
