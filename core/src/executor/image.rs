//! 工具结果图片的冻结副本（ADR-050）。
//!
//! `view_image` 读本地图片 → 严格识别 → 必要的**规范化/降采样** → sha256 命名 → 写入
//! 受管目录 `{data_dir}/session_images/{sha256}.{ext}`；会话里只留**引用**
//! （`session_images/<name>`），组装期按引用回读为 data URL
//! （[`crate::context::SessionImages`]）。
//!
//! 冻结副本的意义（与 ADR-047 spill 同构：大对象出库、会话留引用）：
//! 原图被移动/删除/覆盖后历史回放照常，同一内容只存一份（幂等），字节不进 SQLite。
//!
//! # 为什么落盘前要规范化/降采样（2026-10-24 与 DSH `read_image` 对账后补齐）
//!
//! 只做 magic bytes 校验是不够的——**能识别不等于上游能接收**：
//!
//! | 风险 | 只校验 magic bytes | 本模块 |
//! |---|---|---|
//! | 6000×6000 的 PNG（压完 < 5MB） | 落盘成功，但上游按**尺寸**拒整轮请求 | 等比缩到 2048 内 |
//! | 16-bit PNG | 少数 provider 直接 400 | 解码后转 8-bit 再编码 |
//! | 100 MP 巨图 | 解码要数百 MB 内存（OOM 风险） | 解码前按像素硬拒，提示先缩图 |
//!
//! 缩放后的副本是模型**实际看到**的图，因此结果里回带 `original_dimensions` 与
//! 坐标乘数提示——模型要在原图坐标上定位特征（点按钮、裁区域）时按乘数换算
//! （DSH 同款语义："multiply coordinates by … to locate features in the original file"）。

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::ImageFormat;

use crate::common::binary::sniff_image_mime;
use crate::common::error::{Result, TianyanError};
use crate::common::hash::hex_sha256;

/// 受管图片目录名（相对数据目录）。
pub const SESSION_IMAGES_DIR: &str = "session_images";
/// 单张图片字节上限（**落盘后的最终字节**；读取侧 [`crate::context::SessionImages`] 用同一值）。
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// 单边像素上限：超出即等比降采样。
///
/// 取值依据：主流视觉模型的输入有效分辨率集中在 1568–2048 长边，再大只是
/// 白付 token；2048 兼顾截图细节（1920 宽的 UI 截图不缩）与预算。
pub const MAX_IMAGE_DIMENSION: u32 = 2048;
/// 总像素上限（与单边上限等价，分开命名以便将来单独调整）。
pub const MAX_IMAGE_PIXELS: u64 = (MAX_IMAGE_DIMENSION as u64) * (MAX_IMAGE_DIMENSION as u64);
/// **解码前**的像素硬上限：超过直接拒绝（解码一张 100 MP 图要数百 MB 内存）。
pub const MAX_DECODE_PIXELS: u64 = 64_000_000;

/// 冻结副本的元信息。
pub struct PreparedImage {
    /// 相对数据目录的引用（形如 `session_images/<sha256>.png`）。
    pub reference: String,
    /// 最终字节的 SHA-256 十六进制摘要（副本文件名主体）。
    pub sha256: String,
    /// 最终字节的 MIME（规范化后可能与原文件不同：bmp/16-bit PNG → PNG）。
    pub mime: String,
    /// 最终字节数。
    pub bytes: usize,
    /// 副本的实际像素宽度（模型看到的图）。
    pub width: u32,
    /// 副本的实际像素高度。
    pub height: u32,
    /// 原文件的像素尺寸；**仅当发生降采样/重编码时**给出（否则为 None）。
    pub original_dimensions: Option<(u32, u32)>,
}

impl PreparedImage {
    /// 坐标乘数提示：把模型在副本上量到的坐标换算回原图（`original / 副本`）。
    ///
    /// 两轴比值可能略有差异（整数取整），差异 < 1% 时只给一个数。
    pub fn scale_hint(&self) -> Option<String> {
        let (ow, oh) = self.original_dimensions?;
        let x = ow as f64 / self.width as f64;
        let y = oh as f64 / self.height as f64;
        if (x - y).abs() <= 0.01 {
            Some(format!("{x:.2}"))
        } else {
            Some(format!("x={x:.2}, y={y:.2}"))
        }
    }
}

/// MIME → 文件扩展名（仅覆盖 [`sniff_image_mime`] 的返回值）。
fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        _ => "png",
    }
}

/// 重编码格式：无损优先、体积其次。
///
/// JPEG 保持 JPEG（照片类体积优势大）；其余一律 PNG——`image` crate 的 WebP
/// **只读不写**，BMP 直接存会远大于 PNG，GIF 动图缩帧会丢信息（见调用处拒绝规则）。
fn reencode_format(original_mime: &str) -> &'static str {
    if original_mime == "image/jpeg" {
        "image/jpeg"
    } else {
        "image/png"
    }
}

/// PNG 的 IHDR 位深（偏移 24）；非 PNG 或长度不足返回 0（未知）。
///
/// 16-bit PNG 是唯一需要「解码再降位深」的常见情形：JPEG/GIF 恒定 8-bit，
/// WebP 主流 8-bit。
fn png_bit_depth(bytes: &[u8]) -> u8 {
    if bytes.len() > 25 && bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        bytes[24]
    } else {
        0
    }
}

/// 编码为指定 MIME 的字节（JPEG 用 85 质量；其余 PNG 默认压缩级别）。
fn encode(img: &image::DynamicImage, mime: &str) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    if mime == "image/jpeg" {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
        encoder
            .encode_image(img)
            .map_err(|e| TianyanError::Custom(format!("view_image: 重编码失败（jpeg）：{e}")))?;
    } else {
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .map_err(|e| TianyanError::Custom(format!("view_image: 重编码失败（png）：{e}")))?;
    }
    Ok(buf)
}

/// 读取图片文件 → 校验 → 规范化/降采样 → sha256 → 写入 `{data_dir}/session_images/`。
///
/// 同名副本已存在则不重写（幂等）。失败一律 `TianyanError::Custom("view_image: ...")`。
pub async fn prepare_session_image(path: &str, data_dir: &Path) -> Result<PreparedImage> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| {
        TianyanError::Custom(format!(
            "view_image: 读取失败：无法访问图片文件 {path}: {e}"
        ))
    })?;
    if !meta.is_file() {
        return Err(TianyanError::Custom(format!(
            "view_image: 参数无效：不是文件：{path}"
        )));
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(TianyanError::Custom(format!(
            "view_image: 图片过大：{} 字节，超过上限 {} 字节（5MB）",
            meta.len(),
            MAX_IMAGE_BYTES
        )));
    }

    let raw = tokio::fs::read(path).await.map_err(|e| {
        TianyanError::Custom(format!(
            "view_image: 读取失败：无法读取图片文件 {path}: {e}"
        ))
    })?;

    let mime = sniff_image_mime(&raw).ok_or_else(|| {
        TianyanError::Custom(format!(
            "view_image: 不是支持的图片格式（按内容识别）：{path}；\
             支持 png / jpeg / gif / bmp / webp（仅认 magic bytes，不看扩展名）"
        ))
    })?;

    // 先只读文件头拿尺寸（不解全图），据此决定是否需要解码
    let (src_w, src_h) = image::ImageReader::new(Cursor::new(&raw))
        .with_guessed_format()
        .map_err(|e| TianyanError::Custom(format!("view_image: 读取图片头失败：{path}: {e}")))?
        .into_dimensions()
        .map_err(|e| {
            TianyanError::Custom(format!(
                "view_image: 图片头解析失败（文件可能已损坏）：{path}: {e}"
            ))
        })?;
    let src_pixels = (src_w as u64) * (src_h as u64);
    if src_pixels > MAX_DECODE_PIXELS {
        return Err(TianyanError::Custom(format!(
            "view_image: 像素过多：{src_w}x{src_h}（{} 万像素），超过解码上限 {} 万；\
             请先缩小图片再读取",
            src_pixels / 10_000,
            MAX_DECODE_PIXELS / 10_000
        )));
    }

    let needs_scale =
        src_w > MAX_IMAGE_DIMENSION || src_h > MAX_IMAGE_DIMENSION || src_pixels > MAX_IMAGE_PIXELS;
    // 需要规范化（重编码）的两种情形：
    // - 16-bit PNG（少数 provider 直接 400）
    // - BMP：**不在模型接受的格式列表内**（只认 png/jpeg/webp/gif），落盘前转 PNG
    let needs_normalize = png_bit_depth(&raw) == 16 || mime == "image/bmp";

    let (bytes, final_mime, width, height, original_dimensions) = if needs_scale || needs_normalize
    {
        // GIF 可能是动图：缩放会只保留首帧（语义损失），所以超限时明确拒绝而不是偷偷丢帧
        if needs_scale && mime == "image/gif" {
            return Err(TianyanError::Custom(format!(
                "view_image: GIF 动图尺寸过大（{src_w}x{src_h}），无法安全缩放（会丢帧）；\
                 请改用静态图片或先缩小"
            )));
        }
        let decoded = image::load_from_memory_with_format(&raw, format_of(mime)).map_err(|e| {
            TianyanError::Custom(format!(
                "view_image: 解码失败（文件可能已损坏）：{path}: {e}"
            ))
        })?;
        let (ow, oh) = (decoded.width(), decoded.height());
        // `resize` 等比缩进 MAX_IMAGE_DIMENSION 方框内（大图）或原样（仅降位深）
        let scaled = if needs_scale {
            decoded.resize(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            decoded
        };
        // 统一 8-bit（16-bit PNG 的常态是 RGBA16 → RGBA8）
        let eight_bit = scaled.to_rgba8();
        let (w, h) = (eight_bit.width(), eight_bit.height());
        let out_mime = reencode_format(mime);
        let encoded = encode(&image::DynamicImage::ImageRgba8(eight_bit), out_mime)?;
        let orig = if needs_scale || (ow, oh) != (w, h) {
            Some((ow, oh))
        } else {
            None
        };
        (encoded, out_mime, w, h, orig)
    } else {
        (raw, mime, src_w, src_h, None)
    };

    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(TianyanError::Custom(format!(
            "view_image: 规范化后仍超过上限：{} 字节（上限 {} 字节）；请先压缩图片",
            bytes.len(),
            MAX_IMAGE_BYTES
        )));
    }

    let sha256 = hex_sha256(&bytes);
    let file_name = format!("{sha256}.{}", extension_for(final_mime));
    let dir: PathBuf = data_dir.join(SESSION_IMAGES_DIR);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        TianyanError::Custom(format!(
            "view_image: 写入失败：无法创建图片目录 {}: {e}",
            dir.display()
        ))
    })?;

    let target = dir.join(&file_name);
    if !target.exists() {
        tokio::fs::write(&target, &bytes).await.map_err(|e| {
            TianyanError::Custom(format!(
                "view_image: 写入失败：无法写入图片副本 {}: {e}",
                target.display()
            ))
        })?;
    }

    Ok(PreparedImage {
        reference: format!("{SESSION_IMAGES_DIR}/{file_name}"),
        sha256,
        mime: final_mime.to_string(),
        bytes: bytes.len(),
        width,
        height,
        original_dimensions,
    })
}

/// MIME → `image` crate 的解码格式（与 [`sniff_image_mime`] 返回值一一对应）。
fn format_of(mime: &str) -> ImageFormat {
    match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/bmp" => ImageFormat::Bmp,
        "image/webp" => ImageFormat::WebP,
        _ => ImageFormat::Png,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 字节 PNG 魔数 + 尾巴（仅供 `sniff_image_mime` 使用）。
    const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1A\nrest-of-bytes";

    /// 生成一张真实可解码的 PNG（`image` crate 编码）。
    fn encode_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .expect("encode png");
        buf
    }

    #[tokio::test]
    async fn test_prepare_session_image_writes_frozen_copy() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("shot.png");
        let png = encode_png(64, 48);
        std::fs::write(&src, &png).unwrap();

        let prepared = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        assert_eq!(prepared.mime, "image/png");
        assert_eq!(prepared.bytes, png.len());
        assert_eq!(prepared.sha256, hex_sha256(&png));
        assert_eq!((prepared.width, prepared.height), (64, 48));
        assert!(
            prepared.original_dimensions.is_none(),
            "未缩放时不该记录原图尺寸"
        );
        assert!(prepared.scale_hint().is_none());
        assert!(
            prepared.reference.starts_with("session_images/"),
            "reference 必须是受管目录相对引用：{}",
            prepared.reference
        );
        assert!(prepared.reference.ends_with(".png"));
        assert!(
            data_dir.path().join(&prepared.reference).is_file(),
            "冻结副本应落在 {{data_dir}}/session_images/"
        );
    }

    #[tokio::test]
    async fn test_prepare_session_image_is_idempotent() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("shot.png");
        std::fs::write(&src, encode_png(32, 32)).unwrap();

        let first = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        let second = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.reference, second.reference);

        let entries: Vec<_> = std::fs::read_dir(data_dir.path().join(SESSION_IMAGES_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "同图重复读取只存一份副本：{entries:?}");
    }

    #[tokio::test]
    async fn test_prepare_session_image_downscales_oversized_dimensions() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("wide.png");
        // 3000x600：单边超 2048 → 等比缩到 2048x410
        std::fs::write(&src, encode_png(3000, 600)).unwrap();

        let prepared = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        assert_eq!(prepared.width, MAX_IMAGE_DIMENSION);
        assert!(prepared.height < 600, "等比缩小：{}", prepared.height);
        assert_eq!(
            prepared.original_dimensions,
            Some((3000, 600)),
            "必须记录原图尺寸（模型要在原图坐标定位特征）"
        );
        let hint = prepared.scale_hint().expect("缩放后必须给坐标乘数");
        assert!(hint.starts_with("1.4"), "3000/2048 ≈ 1.46，实际 {hint}");
    }

    #[tokio::test]
    async fn test_prepare_session_image_normalizes_16bit_png() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("deep.png");
        // 16-bit 灰度 PNG：magic 是 PNG，但位深 16 → 必须降位深重编码
        let img: image::ImageBuffer<image::Luma<u16>, Vec<u16>> =
            image::ImageBuffer::from_pixel(16, 16, image::Luma([1024u16]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageLuma16(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        assert_eq!(png_bit_depth(&buf), 16, "测试前提：源文件确实是 16-bit PNG");
        std::fs::write(&src, &buf).unwrap();

        let prepared = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        let stored = std::fs::read(data_dir.path().join(&prepared.reference)).unwrap();
        assert_eq!(
            png_bit_depth(&stored),
            8,
            "落盘副本必须是 8-bit PNG（16-bit 会被少数 provider 直接拒绝）"
        );
        assert_eq!(
            (prepared.width, prepared.height),
            (16, 16),
            "降位深不应改变尺寸"
        );
    }

    #[tokio::test]
    async fn test_prepare_session_image_converts_bmp_to_png() {
        // BMP 能被 magic bytes 识别、也能被 image crate 解码，但**模型不认这个格式**
        // （provider 只接受 png/jpeg/webp/gif）。落盘前必须转成 PNG，否则上游 400。
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("shot.bmp");
        let img = image::RgbImage::from_pixel(24, 16, image::Rgb([200, 100, 50]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Bmp)
            .unwrap();
        assert_eq!(sniff_image_mime(&buf), Some("image/bmp"), "测试前提");
        std::fs::write(&src, &buf).unwrap();

        let prepared = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        assert_eq!(prepared.mime, "image/png", "BMP 必须转成 PNG");
        assert!(
            prepared.reference.ends_with(".png"),
            "{}",
            prepared.reference
        );
        let stored = std::fs::read(data_dir.path().join(&prepared.reference)).unwrap();
        assert_eq!(sniff_image_mime(&stored), Some("image/png"));
        assert_eq!((prepared.width, prepared.height), (24, 16));
    }

    #[tokio::test]
    async fn test_prepare_session_image_rejects_text_file() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("note.txt");
        std::fs::write(&src, b"hello world, not an image").unwrap();

        let Err(err) = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path()).await
        else {
            panic!("非图片必须报错");
        };
        let msg = err.to_string();
        assert!(msg.contains("view_image"), "错误带模块前缀：{msg}");
        assert!(
            !data_dir.path().join(SESSION_IMAGES_DIR).exists(),
            "校验失败不应创建图片目录"
        );
    }

    #[tokio::test]
    async fn test_prepare_session_image_rejects_missing_file() {
        let data_dir = tempfile::tempdir().unwrap();
        let missing = data_dir.path().join("nope.png");
        let Err(err) =
            prepare_session_image(missing.to_string_lossy().as_ref(), data_dir.path()).await
        else {
            panic!("文件不存在必须报错");
        };
        assert!(err.to_string().contains("view_image"));
    }

    #[tokio::test]
    async fn test_prepare_session_image_rejects_oversized_bytes() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("big.png");
        // 头部合法、体积超限：先被字节上限拒绝（不进入解码）
        let mut bytes = PNG_BYTES.to_vec();
        bytes.resize(MAX_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&src, bytes).unwrap();

        let Err(err) = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path()).await
        else {
            panic!("超 5MB 必须报错");
        };
        let msg = err.to_string();
        assert!(msg.contains("图片过大"), "{msg}");
    }
}
