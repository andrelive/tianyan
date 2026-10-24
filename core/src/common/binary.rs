//! 二进制内容嗅探原语。
//!
//! 供 executor（文件读取预览）与 snapshot（diff 二进制判定）复用的
//! 纯函数——历史上存在两份逐字近似的实现（`executor::actions::sniff_binary`、
//! `snapshot::looks_binary`），统一收敛于此（ADR-007 共享基础设施归属被依赖方先例）。

/// 启发式判断内容是否为二进制：含 NUL 字节，或不可打印控制字符
/// （除 `\n` `\r` `\t` 外 < 0x20 及 0x7f）占比严格超过 30%。
///
/// UTF-8 多字节字符（≥ 0x80）不计为控制字符，中文文本不会被误判。
/// 空内容返回 `false`。
pub fn sniff_binary(bytes: &[u8]) -> bool {
    let mut non_printable = 0usize;
    for &b in bytes {
        if b == 0 {
            return true;
        }
        if (b < 0x20 || b == 0x7f) && b != b'\n' && b != b'\r' && b != b'\t' {
            non_printable += 1;
        }
    }
    !bytes.is_empty() && non_printable * 100 > bytes.len() * 30
}

/// 按 magic bytes 识别图片 MIME（严格：未知格式返回 `None`）。
///
/// 支持 png / jpeg / gif / bmp / webp。**刻意不复用** `knowledge::image::analyzer`
/// 的 `infer_mime_type`：那里的语义是"宽容推断 + 未知回退 image/png"（知识导入
/// 不能因为认不出就丢图），这里必须严格——`view_image` 认不出就该报错。
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    // jpeg：3 字节 SOI + 标记前缀
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    // webp：RIFF 容器，偏移 8 起为 "WEBP"
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nul_byte_is_binary() {
        assert!(sniff_binary(b"ab\x00cd"));
    }

    #[test]
    fn test_empty_is_not_binary() {
        assert!(!sniff_binary(b""));
    }

    #[test]
    fn test_utf8_text_is_not_binary() {
        assert!(!sniff_binary("hello world\n第二行\t缩进\r\n".as_bytes()));
    }

    #[test]
    fn test_cjk_text_is_not_binary() {
        assert!(!sniff_binary("中文文本不会误判为二进制".as_bytes()));
    }

    #[test]
    fn test_control_chars_above_threshold() {
        // 31/100 控制字符 > 30% → 二进制
        let mut bytes = vec![b' '; 100];
        for b in bytes.iter_mut().take(31) {
            *b = 0x01;
        }
        assert!(sniff_binary(&bytes));
    }

    #[test]
    fn test_control_chars_at_threshold_not_binary() {
        // 恰好 30/100 = 30% 非严格超过 → 非二进制（严格大于语义）
        let mut bytes = vec![b' '; 100];
        for b in bytes.iter_mut().take(30) {
            *b = 0x01;
        }
        assert!(!sniff_binary(&bytes));
    }

    #[test]
    fn test_whitespace_controls_excluded() {
        // \n \r \t 不计为控制字符
        let bytes = b"\n\r\t\n\r\t\n\r\t";
        assert!(!sniff_binary(bytes));
    }

    #[test]
    fn test_del_is_control() {
        // 0x7f (DEL) 计入控制字符
        let mut bytes = vec![b' '; 10];
        for b in bytes.iter_mut().take(4) {
            *b = 0x7f;
        }
        assert!(sniff_binary(&bytes));
    }

    // ── 图片 magic bytes 识别（ADR-050）────────────────────────────

    #[test]
    fn test_sniff_image_mime_known_formats() {
        assert_eq!(
            sniff_image_mime(b"\x89PNG\r\n\x1A\n\x00\x00"),
            Some("image/png")
        );
        assert_eq!(
            sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image_mime(b"GIF87a...."), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"BM\x00\x00\x00\x00"), Some("image/bmp"));
        assert_eq!(
            sniff_image_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
    }

    #[test]
    fn test_sniff_image_mime_rejects_non_image() {
        assert_eq!(sniff_image_mime(b"hello world"), None);
        assert_eq!(sniff_image_mime(b""), None, "空切片不是图片");
        // webp 需要长度 ≥ 12：截断的 RIFF 头不算
        assert_eq!(sniff_image_mime(b"RIFF\x00\x00WEB"), None);
        assert_eq!(sniff_image_mime(b"RIFF\x00\x00\x00\x00PNG "), None);
    }
}
