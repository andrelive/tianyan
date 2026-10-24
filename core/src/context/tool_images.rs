//! 组装期把 tool 结果里的图片引用加载为 data URL（ADR-050）。
//!
//! tool 结果只存**引用**（`session_images/<sha256>.png`，字节在受管目录里），
//! 组装时按引用回读并编码成 `data:{mime};base64,...`，与 tool 原文一起构成
//! 一条携带图片的工具结果消息（[`crate::common::types::Message::tool_with_images`]）。
//!
//! **best effort，绝不报错**（同 ADR-047 纪律）：无引用 / 副本缺失 / 超限 /
//! magic 不符 → 该引用被跳过，降级为纯文本 tool 结果，会话照常可用。
//!
//! 路径逃逸是**结构上**不可能，而不是靠事后校验：引用必须形如
//! `session_images/<name>`（只认这一种前缀），`<name>` 里禁止 `/`、`\`、`..`。

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde_json::Value;

use crate::common::binary::sniff_image_mime;
use crate::common::types::Message;
use crate::executor::image::{MAX_IMAGE_BYTES, SESSION_IMAGES_DIR};

/// 会话图片加载器：把 tool 结果里的引用解析为 data URL 列表。
pub struct SessionImages {
    dir: PathBuf,
    max_bytes: u64,
}

impl SessionImages {
    /// `dir` 为 `{data_dir}/session_images`。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: MAX_IMAGE_BYTES,
        }
    }

    /// 覆盖单张图片上限（**测试用**；生产恒为 [`MAX_IMAGE_BYTES`]）。
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// 解析 tool 结果原文里的图片引用并加载为 `data:{mime};base64,...` 列表。
    /// 无引用 / 副本缺失 / 超限 / 非图片 → 空（best effort，绝不报错）。
    pub fn resolve(&self, tool_result: &str) -> Vec<String> {
        let Ok(value) = serde_json::from_str::<Value>(tool_result) else {
            return Vec::new();
        };

        let mut references: Vec<&str> = Vec::new();
        if let Some(single) = value.get("image").and_then(Value::as_str) {
            references.push(single);
        }
        if let Some(list) = value.get("images").and_then(Value::as_array) {
            references.extend(list.iter().filter_map(Value::as_str));
        }

        references
            .into_iter()
            .filter_map(|reference| self.load(reference))
            .collect()
    }

    /// 单条引用 → data URL（任何不合规/读不到的情况返回 `None`）。
    fn load(&self, reference: &str) -> Option<String> {
        // 只认 `session_images/<name>`：前缀与分隔符分别剥离（`session_imagesX/…` 也不认）
        let name = reference
            .strip_prefix(SESSION_IMAGES_DIR)?
            .strip_prefix('/')?;
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(name)).ok()?;
        if bytes.len() as u64 > self.max_bytes {
            return None;
        }
        let mime = sniff_image_mime(&bytes)?;
        Some(format!(
            "data:{mime};base64,{}",
            BASE64_STANDARD.encode(&bytes)
        ))
    }

    /// 给一条 tool 消息**就地**附加图片片段（ADR-050；无可注入图片时不动它）。
    ///
    /// 为什么需要它：agent loop 在一轮内直接复用消息 Vec（工具结果 `push` 后
    /// 立刻作为下一次请求），因此**不经** `assemble` 的组装期加载——不在这里
    /// 补一次，模型在"看到工具结果的那一轮"就看不到图（只能靠再调一次工具或
    /// 绕路命令去猜内容）。形状由 [`Message::attach_images`] 单点保证，与下一轮
    /// 重新组装的结果逐字节一致。
    pub fn hydrate(&self, msg: &mut Message) {
        let urls = self.resolve(&msg.content);
        msg.attach_images(urls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1A\nrest-of-bytes";

    /// 造 `{tmp}/session_images/x.png` 并返回加载器。
    fn fixture(dir: &std::path::Path) -> SessionImages {
        let images_dir = dir.join(SESSION_IMAGES_DIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        std::fs::write(images_dir.join("x.png"), PNG_BYTES).unwrap();
        SessionImages::new(images_dir)
    }

    #[test]
    fn test_resolve_single_image_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        let result = r#"{"image":"session_images/x.png","sha256":"abc","mime":"image/png"}"#;
        let urls = images.resolve(result);
        assert_eq!(urls.len(), 1);
        assert!(
            urls[0].starts_with("data:image/png;base64,"),
            "应为 data URL：{}",
            urls[0]
        );
        assert_eq!(
            urls[0],
            format!(
                "data:image/png;base64,{}",
                BASE64_STANDARD.encode(PNG_BYTES)
            )
        );
    }

    #[test]
    fn test_resolve_images_array() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        let result = r#"{"images":["session_images/x.png","session_images/x.png"]}"#;
        assert_eq!(images.resolve(result).len(), 2);
    }

    #[test]
    fn test_resolve_rejects_path_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        assert!(
            images
                .resolve(r#"{"image":"session_images/../x.png"}"#)
                .is_empty(),
            "禁止 .. 逃逸"
        );
        assert!(
            images
                .resolve(r#"{"image":"session_images/sub/x.png"}"#)
                .is_empty(),
            "禁止子目录"
        );
        assert!(
            images.resolve(r#"{"image":"session_images/"}"#).is_empty(),
            "空文件名不合法"
        );
        assert!(
            images
                .resolve(r#"{"image":"session_images/a\\b.png"}"#)
                .is_empty(),
            "禁止反斜杠"
        );
    }

    #[test]
    fn test_resolve_rejects_foreign_prefix_and_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        assert!(
            images.resolve(r#"{"image":"other/x.png"}"#).is_empty(),
            "只认 session_images/ 前缀"
        );
        assert!(
            images
                .resolve(r#"{"image":"session_images/missing.png"}"#)
                .is_empty(),
            "副本缺失 → 降级为纯文本（空列表）"
        );
    }

    #[test]
    fn test_resolve_rejects_non_json_and_no_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        assert!(images.resolve("plain text result").is_empty());
        assert!(images.resolve(r#"{"result":"ok"}"#).is_empty());
        assert!(images.resolve("").is_empty());
    }

    #[test]
    fn test_resolve_rejects_oversize_and_non_image() {
        let tmp = tempfile::tempdir().unwrap();
        let images_dir = tmp.path().join(SESSION_IMAGES_DIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        // 非图片内容（同名的合法引用但 magic 不符）
        std::fs::write(images_dir.join("text.png"), b"not an image").unwrap();
        let images = SessionImages::new(&images_dir);
        assert!(
            images
                .resolve(r#"{"image":"session_images/text.png"}"#)
                .is_empty(),
            "magic 不符 → 跳过"
        );

        // 超限：把上限压到 1 字节，PNG 头就超了
        std::fs::write(images_dir.join("x.png"), PNG_BYTES).unwrap();
        let strict = SessionImages::new(&images_dir).with_max_bytes(1);
        assert!(
            strict
                .resolve(r#"{"image":"session_images/x.png"}"#)
                .is_empty(),
            "超 max_bytes → 跳过"
        );
    }

    // ── 本轮内 hydrate（工具循环直接复用消息 Vec，不经 assemble）────────────

    #[test]
    fn test_hydrate_attaches_image_to_tool_message() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        let mut msg = Message::tool("call_1", r#"{"image":"session_images/x.png"}"#);
        images.hydrate(&mut msg);

        let parts = msg.content_parts.as_ref().expect("应附加图片片段");
        assert_eq!(parts.len(), 2, "文本片段 + 图片片段");
        assert_eq!(
            parts[0].text.as_deref(),
            Some(r#"{"image":"session_images/x.png"}"#)
        );
        assert_eq!(
            msg.image_urls().len(),
            1,
            "形状与组装层 tool_with_images 一致"
        );
        // 与组装层产物逐字节一致（前缀稳定性：本轮 vs 下一轮重新组装）
        let assembled = Message::tool_with_images(
            "call_1",
            r#"{"image":"session_images/x.png"}"#,
            msg.image_urls(),
        );
        assert_eq!(
            serde_json::to_string(&msg.content_parts).unwrap(),
            serde_json::to_string(&assembled.content_parts).unwrap(),
            "本轮 hydrate 与组装层必须逐字节一致"
        );
    }

    #[test]
    fn test_hydrate_keeps_plain_tool_message_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let images = fixture(tmp.path());
        let mut msg = Message::tool("call_1", r#"{"result":"ok"}"#);
        images.hydrate(&mut msg);
        assert!(
            msg.content_parts.is_none(),
            "无引用时保持纯文本（content_parts 不设）"
        );

        let mut missing = Message::tool("call_2", r#"{"image":"session_images/missing.png"}"#);
        images.hydrate(&mut missing);
        assert!(
            missing.content_parts.is_none(),
            "副本缺失 → 降级纯文本（best effort）"
        );
    }
}
