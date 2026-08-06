//! VLM 图像分析器。
//!
//! 通过视觉语言模型生成图像描述与结构化分析，
//! 并支持统一文本表示与视觉嵌入。

use crate::common::error::Result;
use crate::common::types::Embedding;
use crate::model::types::{ContentPart, ImageUrl, VisionContent, VisionMessage, VisionRequest};
use crate::model::{EmbeddingService, VlmService};

use super::types::{ImageAnalysis, ImageType, UnifiedTextRepresentation};

/// 使用 VLM 的图像分析器。
pub struct ImageAnalyzer<'a> {
    vlm_service: &'a dyn VlmService,
    embedding_service: &'a dyn EmbeddingService,
    model: String,
}

impl<'a> ImageAnalyzer<'a> {
    /// 创建新的图像分析器。
    pub fn new(
        vlm_service: &'a dyn VlmService,
        embedding_service: &'a dyn EmbeddingService,
        model: impl Into<String>,
    ) -> Self {
        Self {
            vlm_service,
            embedding_service,
            model: model.into(),
        }
    }

    /// 分析图像并生成描述。
    pub async fn analyze(&self, image_data: &[u8]) -> Result<ImageAnalysis> {
        // 使用 VLM 生成描述
        let prompt = "分析这张图片并提供：\n\
                      1. 你所看到内容的详细描述\n\
                      2. 图像中的关键元素或对象（每行列出一个）\n\
                      3. 任何可见的文本（精确转录）\n\
                      4. 建议的分类标签\n\
                      5. 图像类型（screenshot、photo、diagram、chart、code、document、icon、illustration 或 other）\n\
                      请以 JSON 格式返回，包含以下键：description、key_elements（数组）、extracted_text、tags（数组）、image_type、confidence（0-1）";

        let description =
            analyze_image_base64(self.vlm_service, &self.model, image_data, prompt).await?;

        // 解析响应
        self.parse_analysis_response(&description)
    }

    /// 将 VLM 响应解析为结构化分析。
    fn parse_analysis_response(&self, response: &str) -> Result<ImageAnalysis> {
        // 首先尝试解析为 JSON
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(response) {
            return Ok(ImageAnalysis {
                description: json
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or(response)
                    .to_string(),
                key_elements: json
                    .get("key_elements")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                extracted_text: json
                    .get("extracted_text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                tags: json
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                image_type: json
                    .get("image_type")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(&format!("\"{}\"", s)).ok())
                    .unwrap_or(ImageType::Other),
                confidence: json
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(0.8),
            });
        }

        // 回退：使用整个响应作为描述
        Ok(ImageAnalysis {
            description: response.to_string(),
            key_elements: Vec::new(),
            extracted_text: None,
            tags: Vec::new(),
            image_type: ImageType::Other,
            confidence: 0.5,
        })
    }

    /// 创建统一文本表示。
    pub fn create_unified_text(&self, analysis: &ImageAnalysis) -> UnifiedTextRepresentation {
        UnifiedTextRepresentation::new(analysis)
    }

    /// 为图像生成视觉嵌入向量（通过多模态嵌入模型）。
    pub async fn generate_visual_embedding(&self, image_data: &[u8]) -> Result<Embedding> {
        self.embedding_service
            .embed_image(&self.model, image_data)
            .await
    }
}

async fn analyze_image_base64(
    vlm: &dyn VlmService,
    model: &str,
    image_data: &[u8],
    prompt: &str,
) -> Result<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let base64_data = STANDARD.encode(image_data);
    let mime_type = crate::model::provider::vision::infer_mime_type(image_data);
    let data_url = format!("data:{};base64,{}", mime_type, base64_data);

    let message = VisionMessage {
        role: "user".to_string(),
        content: VisionContent::MultiPart(vec![
            ContentPart {
                content_type: "text".to_string(),
                text: Some(prompt.to_string()),
                image_url: None,
            },
            ContentPart {
                content_type: "image_url".to_string(),
                text: None,
                image_url: Some(ImageUrl {
                    url: data_url,
                    detail: None,
                }),
            },
        ]),
    };

    let request = VisionRequest::new(model, vec![message]);
    let response = vlm.analyze_image(request).await?;
    Ok(response
        .choices
        .first()
        .map(|c| match &c.message.content {
            VisionContent::Text(text) => text.clone(),
            VisionContent::MultiPart(parts) => parts
                .iter()
                .filter_map(|p| p.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .unwrap_or_default())
}
