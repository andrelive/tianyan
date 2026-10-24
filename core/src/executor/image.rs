//! 工具结果图片的冻结副本（ADR-050）。
//!
//! `view_image` 读本地图片 → magic bytes 校验 → sha256 命名 → 写入受管目录
//! `{data_dir}/session_images/{sha256}.{ext}`；会话里只留**引用**（`session_images/<name>`），
//! 组装期按引用回读为 data URL（[`crate::context::SessionImages`]）。
//!
//! 冻结副本的意义（与 ADR-047 spill 同构：大对象出库、会话留引用）：
//! 原图被移动/删除/覆盖后历史回放照常，同一内容只存一份（幂等），字节不进 SQLite。

use std::path::{Path, PathBuf};

use crate::common::binary::sniff_image_mime;
use crate::common::error::{Result, TianyanError};
use crate::common::hash::hex_sha256;

/// 受管图片目录名（相对数据目录）。
pub const SESSION_IMAGES_DIR: &str = "session_images";
/// 单张图片上限（字节）。
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// 冻结副本的元信息。
pub struct PreparedImage {
    /// 相对数据目录的引用（形如 `session_images/<sha256>.png`）。
    pub reference: String,
    /// 图片字节的 SHA-256 十六进制摘要（副本文件名主体）。
    pub sha256: String,
    /// 按 magic bytes 识别出的 MIME（png/jpeg/gif/bmp/webp）。
    pub mime: String,
    /// 图片字节数。
    pub bytes: usize,
}

/// MIME → 文件扩展名（仅覆盖 [`sniff_image_mime`] 的返回值）。
fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        // sniff_image_mime 只会返回上面四种或 png
        _ => "png",
    }
}

/// 读取图片文件 → magic bytes 校验 → sha256 → 写入 `{data_dir}/session_images/`。
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

    let bytes = tokio::fs::read(path).await.map_err(|e| {
        TianyanError::Custom(format!(
            "view_image: 读取失败：无法读取图片文件 {path}: {e}"
        ))
    })?;

    let mime = sniff_image_mime(&bytes).ok_or_else(|| {
        TianyanError::Custom(format!(
            "view_image: 不是支持的图片格式（按内容识别）：{path}；\
             支持 png / jpeg / gif / bmp / webp（仅认 magic bytes，不看扩展名）"
        ))
    })?;

    let sha256 = hex_sha256(&bytes);
    let file_name = format!("{sha256}.{}", extension_for(mime));
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
        mime: mime.to_string(),
        bytes: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 字节 PNG 魔数 + 尾巴（sniff_image_mime 只认头部）。
    const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1A\nrest-of-bytes";

    #[tokio::test]
    async fn test_prepare_session_image_writes_frozen_copy() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("shot.png");
        std::fs::write(&src, PNG_BYTES).unwrap();

        let prepared = prepare_session_image(src.to_string_lossy().as_ref(), data_dir.path())
            .await
            .unwrap();
        assert_eq!(prepared.mime, "image/png");
        assert_eq!(prepared.bytes, PNG_BYTES.len());
        assert_eq!(prepared.sha256, hex_sha256(PNG_BYTES));
        assert!(
            prepared.reference.starts_with("session_images/"),
            "reference 必须是受管目录相对引用：{}",
            prepared.reference
        );
        assert!(prepared.reference.ends_with(".png"));
        assert!(
            data_dir
                .path()
                .join(SESSION_IMAGES_DIR)
                .join(format!("{}.png", prepared.sha256))
                .is_file(),
            "冻结副本应落在 {{data_dir}}/session_images/"
        );
    }

    #[tokio::test]
    async fn test_prepare_session_image_is_idempotent() {
        let data_dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("shot.png");
        std::fs::write(&src, PNG_BYTES).unwrap();

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
}
