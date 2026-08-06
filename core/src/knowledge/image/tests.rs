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
