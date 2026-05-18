//! 知识导入的图像处理模块。
//!
//! 本模块提供图像处理能力，包括格式转换、压缩、EXIF 提取和基于 VLM 的理解。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use image::ImageFormat;
use serde::{Deserialize, Serialize};

use crate::common::error::{Result, TianyanError};
use crate::model::types::{ContentPart, ImageUrl, VisionContent, VisionMessage, VisionRequest};
use crate::model::{VisionEncoder, VlmService};

/// 支持的图像格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormatType {
    /// JPEG 格式。
    Jpeg,
    /// PNG 格式。
    Png,
    /// GIF 格式。
    Gif,
    /// WebP 格式。
    WebP,
    /// BMP 格式。
    Bmp,
    /// TIFF 格式。
    Tiff,
}

impl ImageFormatType {
    /// 获取此格式的文件扩展名。
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }

    /// 从文件扩展名解析。
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::WebP),
            "bmp" => Some(Self::Bmp),
            "tiff" | "tif" => Some(Self::Tiff),
            _ => None,
        }
    }

    /// 获取 image crate 的格式。
    pub fn to_image_format(&self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Gif => ImageFormat::Gif,
            Self::WebP => ImageFormat::WebP,
            Self::Bmp => ImageFormat::Bmp,
            Self::Tiff => ImageFormat::Tiff,
        }
    }
}

/// 从图像中提取 EXIF 元数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExifMetadata {
    /// 相机制造商。
    pub make: Option<String>,
    /// 相机型号。
    pub model: Option<String>,
    /// 拍摄日期和时间。
    pub datetime: Option<String>,
    /// 曝光时间。
    pub exposure_time: Option<String>,
    /// 光圈值（F 数）。
    pub f_number: Option<String>,
    /// ISO 感光度。
    pub iso: Option<u32>,
    /// 焦距。
    pub focal_length: Option<String>,
    /// GPS 纬度。
    pub gps_latitude: Option<f64>,
    /// GPS 经度。
    pub gps_longitude: Option<f64>,
    /// 图像宽度。
    pub width: Option<u32>,
    /// 图像高度。
    pub height: Option<u32>,
    /// 方向。
    pub orientation: Option<u16>,
    /// 使用的软件。
    pub software: Option<String>,
    /// 自定义 EXIF 字段。
    pub custom: HashMap<String, String>,
}

/// 图像处理结果。
#[derive(Debug, Clone)]
pub struct ProcessedImage {
    /// 原始图像数据。
    pub original_data: Vec<u8>,
    /// 处理后的图像数据（压缩/转换）。
    pub processed_data: Vec<u8>,
    /// 缩略图数据。
    pub thumbnail: Option<Vec<u8>>,
    /// 图像格式。
    pub format: ImageFormatType,
    /// 图像宽度（像素）。
    pub width: u32,
    /// 图像高度（像素）。
    pub height: u32,
    /// EXIF 元数据。
    pub exif: ExifMetadata,
    /// 原始文件大小。
    pub original_size: u64,
    /// 处理后文件大小。
    pub processed_size: u64,
}

/// VLM 分析结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysis {
    /// 图像的整体描述。
    pub description: String,
    /// 图像中识别的关键元素。
    pub key_elements: Vec<String>,
    /// 从图像中提取的文本（OCR）。
    pub extracted_text: Option<String>,
    /// 图像的标签/分类。
    pub tags: Vec<String>,
    /// 图像类型分类。
    pub image_type: ImageType,
    /// 分析的置信度分数。
    pub confidence: f32,
}

/// 图像内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ImageType {
    /// 应用程序或界面截图。
    Screenshot,
    /// 照片。
    Photo,
    /// 图表或流程图。
    Diagram,
    /// 图表或图形。
    Chart,
    /// 代码截图。
    CodeScreenshot,
    /// 文档扫描。
    DocumentScan,
    /// 图标或标志。
    Icon,
    /// 插图或绘图。
    Illustration,
    /// 其他/未知。
    #[default]
    Other,
}

/// 图像的统一文本表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedTextRepresentation {
    /// VLM 生成的描述。
    pub description: String,
    /// 提取的文本（OCR）。
    pub extracted_text: Option<String>,
    /// 关键元素。
    pub key_elements: Vec<String>,
    /// 标签。
    pub tags: Vec<String>,
    /// 图像类型。
    pub image_type: ImageType,
    /// 用于索引的组合文本。
    pub combined_text: String,
}

impl UnifiedTextRepresentation {
    /// 创建新的统一文本表示。
    pub fn new(analysis: &ImageAnalysis) -> Self {
        let mut combined_parts = Vec::new();

        // 添加描述
        combined_parts.push(analysis.description.clone());

        // 添加关键元素
        if !analysis.key_elements.is_empty() {
            combined_parts.push(format!("关键元素: {}", analysis.key_elements.join(", ")));
        }

        // 添加提取的文本
        if let Some(ref text) = analysis.extracted_text {
            combined_parts.push(format!("文本内容: {}", text));
        }

        // 添加标签
        if !analysis.tags.is_empty() {
            combined_parts.push(format!("标签: {}", analysis.tags.join(", ")));
        }

        let combined_text = combined_parts.join("\n");

        Self {
            description: analysis.description.clone(),
            extracted_text: analysis.extracted_text.clone(),
            key_elements: analysis.key_elements.clone(),
            tags: analysis.tags.clone(),
            image_type: analysis.image_type,
            combined_text,
        }
    }
}

/// 图像处理器配置。
#[derive(Debug, Clone)]
pub struct ImageProcessorConfig {
    /// 处理后图像的最大宽度。
    pub max_width: u32,
    /// 处理后图像的最大高度。
    pub max_height: u32,
    /// JPEG 质量（1-100）。
    pub jpeg_quality: u8,
    /// 是否创建缩略图。
    pub create_thumbnails: bool,
    /// 缩略图宽度。
    pub thumbnail_width: u32,
    /// 缩略图高度。
    pub thumbnail_height: u32,
    /// 处理后图像的目标格式。
    pub target_format: ImageFormatType,
}

impl Default for ImageProcessorConfig {
    fn default() -> Self {
        Self {
            max_width: 2048,
            max_height: 2048,
            jpeg_quality: 85,
            create_thumbnails: true,
            thumbnail_width: 256,
            thumbnail_height: 256,
            target_format: ImageFormatType::Jpeg,
        }
    }
}

impl ImageProcessorConfig {
    /// 创建新配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置最大尺寸。
    pub fn with_max_dimensions(mut self, width: u32, height: u32) -> Self {
        self.max_width = width;
        self.max_height = height;
        self
    }

    /// 设置 JPEG 质量。
    pub fn with_jpeg_quality(mut self, quality: u8) -> Self {
        self.jpeg_quality = quality.clamp(1, 100);
        self
    }

    /// 启用或禁用缩略图。
    pub fn with_thumbnails(mut self, enabled: bool) -> Self {
        self.create_thumbnails = enabled;
        self
    }
}

/// 处理图像导入的图像处理器。
pub struct ImageProcessor {
    config: ImageProcessorConfig,
}

impl ImageProcessor {
    /// 创建新的图像处理器。
    pub fn new(config: ImageProcessorConfig) -> Self {
        Self { config }
    }

    /// 使用默认配置创建处理器。
    pub fn with_defaults() -> Self {
        Self::new(ImageProcessorConfig::default())
    }

    /// 从字节处理图像。
    pub fn process(&self, data: &[u8], _filename: &str) -> Result<ProcessedImage> {
        // 加载图像
        let img = image::load_from_memory(data)
            .map_err(|e| TianyanError::ImageProcessing(format!("加载图像失败: {}", e)))?;

        let original_width = img.width();
        let original_height = img.height();

        // 如需要则调整大小
        let processed_img =
            if original_width > self.config.max_width || original_height > self.config.max_height {
                img.resize(
                    self.config.max_width,
                    self.config.max_height,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                img.clone()
            };

        // 转换为目标格式
        let mut processed_buffer = Vec::new();
        let mut cursor = Cursor::new(&mut processed_buffer);

        match self.config.target_format {
            ImageFormatType::Jpeg => {
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut cursor,
                    self.config.jpeg_quality,
                );
                processed_img
                    .write_with_encoder(encoder)
                    .map_err(|e| TianyanError::ImageProcessing(format!("JPEG 编码失败: {}", e)))?;
            }
            _ => {
                processed_img
                    .write_to(&mut cursor, self.config.target_format.to_image_format())
                    .map_err(|e| TianyanError::ImageProcessing(format!("图像编码失败: {}", e)))?;
            }
        }

        let processed_size = processed_buffer.len() as u64;

        // 如果启用则创建缩略图
        let thumbnail = if self.config.create_thumbnails {
            let thumbnail_img =
                processed_img.thumbnail(self.config.thumbnail_width, self.config.thumbnail_height);
            let mut thumb_buffer = Vec::new();
            let mut thumb_cursor = Cursor::new(&mut thumb_buffer);
            thumbnail_img
                .write_to(&mut thumb_cursor, ImageFormat::Jpeg)
                .map_err(|e| TianyanError::ImageProcessing(format!("创建缩略图失败：{}", e)))?;
            Some(thumb_buffer)
        } else {
            None
        };

        // 提取 EXIF 元数据
        let exif = self.extract_exif(data)?;

        Ok(ProcessedImage {
            original_data: data.to_vec(),
            processed_data: processed_buffer,
            thumbnail,
            format: self.config.target_format,
            width: processed_img.width(),
            height: processed_img.height(),
            exif,
            original_size: data.len() as u64,
            processed_size,
        })
    }

    /// 从文件名和内容检测图像格式。
    #[allow(dead_code)]
    fn detect_format(&self, filename: &str, _data: &[u8]) -> Result<ImageFormatType> {
        // 首先尝试从扩展名检测
        let ext = Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if let Some(format) = ImageFormatType::from_extension(ext) {
            return Ok(format);
        }

        // 如果未知则默认为 JPEG
        Ok(ImageFormatType::Jpeg)
    }

    /// 从图像提取 EXIF 元数据。
    fn extract_exif(&self, data: &[u8]) -> Result<ExifMetadata> {
        let mut exif = ExifMetadata::default();

        // 尝试解析 EXIF 数据
        let reader = exif::Reader::new();
        let exif_data = match reader.read_from_container(&mut Cursor::new(data)) {
            Ok(data) => data,
            Err(_) => return Ok(exif), // 没有 EXIF 数据，返回空
        };

        // 提取常见字段
        for field in exif_data.fields() {
            match field.tag {
                exif::Tag::Make => {
                    exif.make = Some(field.display_value().to_string());
                }
                exif::Tag::Model => {
                    exif.model = Some(field.display_value().to_string());
                }
                exif::Tag::DateTime => {
                    exif.datetime = Some(field.display_value().to_string());
                }
                exif::Tag::ExposureTime => {
                    exif.exposure_time = Some(field.display_value().to_string());
                }
                exif::Tag::FNumber => {
                    exif.f_number = Some(field.display_value().to_string());
                }
                exif::Tag::PhotographicSensitivity => {
                    if let exif::Value::Short(v) = &field.value {
                        exif.iso = v.first().copied().map(|v| v as u32);
                    }
                }
                exif::Tag::FocalLength => {
                    exif.focal_length = Some(field.display_value().to_string());
                }
                exif::Tag::ImageWidth => {
                    if let exif::Value::Long(v) = &field.value {
                        exif.width = v.first().copied();
                    }
                }
                exif::Tag::ImageLength => {
                    if let exif::Value::Long(v) = &field.value {
                        exif.height = v.first().copied();
                    }
                }
                exif::Tag::Orientation => {
                    if let exif::Value::Short(v) = &field.value {
                        exif.orientation = v.first().copied();
                    }
                }
                exif::Tag::Software => {
                    exif.software = Some(field.display_value().to_string());
                }
                _ => {}
            }
        }

        Ok(exif)
    }
}

/// 使用 VLM 的图像分析器。
pub struct ImageAnalyzer<'a, V: VlmService, E: VisionEncoder> {
    vlm_service: &'a V,
    vision_encoder: &'a E,
    model: String,
}

impl<'a, V: VlmService, E: VisionEncoder> ImageAnalyzer<'a, V, E> {
    /// 创建新的图像分析器。
    pub fn new(vlm_service: &'a V, vision_encoder: &'a E, model: impl Into<String>) -> Self {
        Self {
            vlm_service,
            vision_encoder,
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

    /// 为图像生成视觉嵌入向量。
    pub async fn generate_visual_embedding(
        &self,
        image_data: &[u8],
    ) -> Result<crate::common::types::Embedding> {
        self.vision_encoder.encode_image(image_data).await
    }

    /// 创建统一文本表示。
    pub fn create_unified_text(&self, analysis: &ImageAnalysis) -> UnifiedTextRepresentation {
        UnifiedTextRepresentation::new(analysis)
    }
}

async fn analyze_image_base64<V: VlmService>(
    vlm: &V,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_type() {
        assert_eq!(ImageFormatType::Jpeg.extension(), "jpg");
        assert_eq!(
            ImageFormatType::from_extension("png"),
            Some(ImageFormatType::Png)
        );
        assert_eq!(ImageFormatType::from_extension("unknown"), None);
    }

    #[test]
    fn test_image_processor_config() {
        let config = ImageProcessorConfig::new()
            .with_max_dimensions(1024, 1024)
            .with_jpeg_quality(90)
            .with_thumbnails(false);

        assert_eq!(config.max_width, 1024);
        assert_eq!(config.max_height, 1024);
        assert_eq!(config.jpeg_quality, 90);
        assert!(!config.create_thumbnails);
    }

    #[test]
    fn test_unified_text_representation() {
        let analysis = ImageAnalysis {
            description: "测试图像".to_string(),
            key_elements: vec!["元素1".to_string(), "元素2".to_string()],
            extracted_text: Some("Hello World".to_string()),
            tags: vec!["测试".to_string()],
            image_type: ImageType::Photo,
            confidence: 0.9,
        };

        let unified = UnifiedTextRepresentation::new(&analysis);

        assert!(unified.combined_text.contains("测试图像"));
        assert!(unified.combined_text.contains("元素1"));
        assert!(unified.combined_text.contains("Hello World"));
        assert!(unified.combined_text.contains("测试"));
    }

    #[test]
    fn test_image_type_default() {
        let image_type = ImageType::default();
        assert_eq!(image_type, ImageType::Other);
    }

    #[test]
    fn test_exif_metadata_default() {
        let exif = ExifMetadata::default();
        assert!(exif.make.is_none());
        assert!(exif.model.is_none());
    }
}
