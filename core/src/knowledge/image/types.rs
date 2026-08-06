//! 图像领域数据类型。
//!
//! 格式、EXIF 元数据、处理结果、分析结果与处理器配置。
//! 处理逻辑见 [`super::processor`]，VLM 分析见 [`super::analyzer`]。

use std::collections::HashMap;

use image::ImageFormat;
use serde::{Deserialize, Serialize};

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
