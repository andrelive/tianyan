//! 多模态内容片段类型（传输格式）。
//!
//! 作为基础 DTO 位于 common 层，供对话消息（`Message::content_parts`）
//! 与视觉模型请求（`model::types::vision` re-export）共用。

use serde::{Deserialize, Serialize};

/// 内容片段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    /// 内容类型（如 `text`、`image_url`）。
    #[serde(rename = "type")]
    pub content_type: String,
    /// 文本内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 图片 URL。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageUrl>,
}

impl ContentPart {
    /// 创建文本片段。
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content_type: "text".to_string(),
            text: Some(text.into()),
            image_url: None,
        }
    }

    /// 创建图片片段（`url` 为 URL 或 `data:image/png;base64,...` data URL）。
    pub fn image(url: impl Into<String>) -> Self {
        Self {
            content_type: "image_url".to_string(),
            text: None,
            image_url: Some(ImageUrl {
                url: url.into(),
                detail: None,
            }),
        }
    }
}

/// 图片 URL。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    /// URL 地址（或 base64 data URL）。
    pub url: String,
    /// 细节级别。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
