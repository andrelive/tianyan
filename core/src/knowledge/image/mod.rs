//! 知识导入的图像处理模块。
//!
//! 本模块提供图像处理能力，包括格式转换、压缩、EXIF 提取和基于 VLM 的理解。
//!
//! 按职责拆分：
//! - [`types`]：图像领域数据类型（格式 / EXIF / 处理结果 / 分析结果 / 配置）
//! - [`processor`]：图像处理器（格式转换、压缩、EXIF 提取）
//! - [`analyzer`]：VLM 图像分析器（描述生成、统一文本表示、视觉嵌入）

mod analyzer;
mod processor;
mod types;

pub use analyzer::ImageAnalyzer;
pub use processor::ImageProcessor;
pub use types::{
    ExifMetadata, ImageAnalysis, ImageFormatType, ImageProcessorConfig, ImageType, ProcessedImage,
    UnifiedTextRepresentation,
};

/// 测试模块（拆分至独立文件，保持主模块聚焦生产逻辑）。
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
