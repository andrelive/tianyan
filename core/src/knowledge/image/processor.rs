//! 图像处理器。
//!
//! 格式转换、压缩、缩略图与 EXIF 元数据提取。

use std::io::Cursor;

use image::ImageFormat;

use crate::common::error::{Result, TianyanError};

use super::types::{ExifMetadata, ImageFormatType, ImageProcessorConfig, ProcessedImage};

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
            .map_err(|e| TianyanError::Custom(format!("图片处理错误：加载图像失败: {}", e)))?;

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
                processed_img.write_with_encoder(encoder).map_err(|e| {
                    TianyanError::Custom(format!("图片处理错误：JPEG 编码失败: {}", e))
                })?;
            }
            _ => {
                processed_img
                    .write_to(&mut cursor, self.config.target_format.to_image_format())
                    .map_err(|e| {
                        TianyanError::Custom(format!("图片处理错误：图像编码失败: {}", e))
                    })?;
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
                .map_err(|e| {
                    TianyanError::Custom(format!("图片处理错误：创建缩略图失败：{}", e))
                })?;
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
                _ => {
                    tracing::debug!(?field.tag, "EXIF parser: unhandled tag (skipping)");
                }
            }
        }

        Ok(exif)
    }
}
