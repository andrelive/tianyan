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
use crate::common::types::{Message, MessageRole};
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
        // **廉价门控**：绝大多数 tool 结果（命令输出 / 文件内容 / grep 命中）里
        // 根本没有图片引用，而 `serde_json::from_str::<Value>` 会把整段内容解析成
        // 语法树（几十 KB ~ 几 MB 的分配 + 解析）——**每条** tool 结果都要付一次。
        // 子串扫描不分配、不解析，先挡掉这些调用；含引用的结果照旧走完整解析。
        if !tool_result.contains(SESSION_IMAGES_DIR) {
            return Vec::new();
        }
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

    /// 轮内图片窗口（ADR-050 修订二）：只保留 `batch_start` 之后新增的 tool 图，
    /// 把更早批次的 tool 图片片段剥回纯文本（`content_parts = None`——与跨轮组装
    /// 产出的形态逐字节一致，所以下一轮重新组装时视图不变）。
    ///
    /// 按**批**保留而不是按张：`batch_start` 是一次 LLM 请求的全部工具结果起点，
    /// 同批并行 `view_image` 的多张图因此全部可见（"改一版、看一眼、再改一版"）。
    ///
    /// 两条纪律：
    /// - **本批没有图时不动更早的图**——普通工具调用不该清掉上一批图，模型可能
    ///   仍在用它们推理；
    /// - 只动 `role: tool` 的消息：用户消息里的图（`Part::Image`）不受影响。
    pub fn retain_only_last_batch(&self, messages: &mut [Message], batch_start: usize) {
        let start = batch_start.min(messages.len());
        let batch_has_images = messages[start..]
            .iter()
            .any(|m| m.role == MessageRole::Tool && m.content_parts.is_some());
        if !batch_has_images {
            return;
        }
        for m in &mut messages[..start] {
            if m.role == MessageRole::Tool && m.content_parts.is_some() {
                m.content_parts = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1A\nrest-of-bytes";

    /// 轮内只保留最近一批图：更早批次的 tool 图被剥成纯文本，本批保留。
    #[test]
    fn test_retain_only_last_batch_strips_earlier_batches() {
        let images = SessionImages::new(std::env::temp_dir());
        let url = || vec!["data:image/png;base64,AA".to_string()];
        let mut messages = vec![
            Message::tool_with_images("c1", r#"{"image":"session_images/a.png"}"#, url()),
            Message::tool_with_images("c2", r#"{"image":"session_images/b.png"}"#, url()),
            Message::tool_with_images("c3", r#"{"image":"session_images/c.png"}"#, url()),
        ];

        images.retain_only_last_batch(&mut messages, 2);

        assert!(
            messages[0].content_parts.is_none(),
            "更早批次的图必须剥掉（否则断点前移，整批之后全部重算）"
        );
        assert!(messages[1].content_parts.is_none());
        assert!(
            messages[2].content_parts.is_some(),
            "最近一批的图必须保留——模型正靠它推理"
        );
        assert_eq!(messages[2].image_urls().len(), 1);
        assert_eq!(
            messages[0].content, r#"{"image":"session_images/a.png"}"#,
            "剥图只动 content_parts，引用原文原样保留"
        );
    }

    /// 本批没有图时**不动**更早的图：普通工具调用不该清掉上一批图。
    #[test]
    fn test_retain_only_last_batch_keeps_earlier_when_batch_has_no_images() {
        let images = SessionImages::new(std::env::temp_dir());
        let mut messages = vec![
            Message::tool_with_images(
                "c1",
                r#"{"image":"session_images/a.png"}"#,
                vec!["data:image/png;base64,AA".to_string()],
            ),
            Message::tool("c2", "plain command output"),
        ];

        images.retain_only_last_batch(&mut messages, 1);

        assert!(
            messages[0].content_parts.is_some(),
            "本批无图 → 上一批图必须留着（模型可能仍在用）"
        );
        assert!(messages[1].content_parts.is_none());
    }

    /// 只剥 tool 消息：用户自己贴的图（`Part::Image`）不受影响。
    #[test]
    fn test_retain_only_last_batch_never_touches_user_images() {
        let images = SessionImages::new(std::env::temp_dir());
        let mut messages = vec![
            Message::user_with_images("看这张图", vec!["data:image/png;base64,UU".to_string()]),
            Message::tool_with_images(
                "c1",
                r#"{"image":"session_images/a.png"}"#,
                vec!["data:image/png;base64,AA".to_string()],
            ),
            Message::tool_with_images(
                "c2",
                r#"{"image":"session_images/b.png"}"#,
                vec!["data:image/png;base64,BB".to_string()],
            ),
        ];

        images.retain_only_last_batch(&mut messages, 2);

        assert!(
            messages[0].content_parts.is_some(),
            "用户消息的图与工具图窗口无关"
        );
        assert!(messages[1].content_parts.is_none());
        assert!(messages[2].content_parts.is_some());
    }

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
