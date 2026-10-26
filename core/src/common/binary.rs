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

/// 按 magic bytes 识别二进制容器格式（`read_file` 命中二进制时回传给模型，
/// 让它知道该换什么工具 / 该不该尝试解包）。
///
/// 返回**短标识字符串**（`&'static str`，零分配）：`zip` / `pdf` / `ole2` /
/// `image` / `gzip` / `rar` / `7z` / `sqlite` / `pe` / `elf`；认不出来返回
/// `"unknown"`——**刻意不猜**：格式名会被模型拿去选解包命令（`unzip` / `pdftotext`
/// / `sqlite3`），猜错比"不知道"更坏。空切片与不足 magic 长度的前缀都归 `unknown`。
///
/// 注意 `zip` 是**容器**语义：docx/xlsx/pptx/jar 共用 `PK\x03\x04`，此处无法
/// （也不该）按扩展名二次区分——那是调用方（hint 文案）的事。
pub fn sniff_binary_format(bytes: &[u8]) -> &'static str {
    // 图片优先：png/jpeg/gif/bmp/webp 复用严格 MIME 嗅探，不重复实现魔数。
    if sniff_image_mime(bytes).is_some() {
        return "image";
    }
    // ZIP：本地文件头 PK\x03\x04；空归档 PK\x05\x06；分卷/跨界 PK\x07\x08。
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        return "zip";
    }
    if bytes.starts_with(b"%PDF-") {
        return "pdf";
    }
    // OLE2 复合文档（旧版 doc/xls/ppt）：固定 8 字节签名。
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return "ole2";
    }
    if bytes.starts_with(&[0x1F, 0x8B]) {
        return "gzip";
    }
    if bytes.starts_with(b"Rar!") {
        return "rar";
    }
    if bytes.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return "7z";
    }
    // SQLite 数据库文件头（含结尾 NUL）。
    if bytes.starts_with(b"SQLite format 3\0") {
        return "sqlite";
    }
    // PE（exe/dll）：DOS 头 "MZ"。仅前缀，不做 PE 头偏移校验（够用即可）。
    if bytes.starts_with(b"MZ") {
        return "pe";
    }
    if bytes.starts_with(&[0x7F, 0x45, 0x4C, 0x46]) {
        return "elf";
    }
    "unknown"
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

    // ── 二进制容器格式识别（read_file 的 format/hint 回传）──────────────

    #[test]
    fn test_sniff_binary_format_zip_family() {
        // docx/xlsx/pptx/jar 共用本地文件头
        assert_eq!(sniff_binary_format(b"PK\x03\x04\x14\x00\x06\x00"), "zip");
        // 空归档 / 分卷签名也归 zip
        assert_eq!(sniff_binary_format(b"PK\x05\x06\x00\x00"), "zip");
        assert_eq!(sniff_binary_format(b"PK\x07\x08\x01\x02"), "zip");
        // 只有两个字节 "PK" 不足以判定
        assert_eq!(sniff_binary_format(b"PK"), "unknown");
    }

    #[test]
    fn test_sniff_binary_format_pdf() {
        assert_eq!(sniff_binary_format(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3"), "pdf");
        assert_eq!(sniff_binary_format(b"%PDF-"), "pdf");
        // 少了连字符就不认（避免把普通文本前缀误判）
        assert_eq!(sniff_binary_format(b"%PDF"), "unknown");
    }

    #[test]
    fn test_sniff_binary_format_ole2() {
        assert_eq!(
            sniff_binary_format(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00]),
            "ole2"
        );
        // 截断的签名（7 字节）不认
        assert_eq!(
            sniff_binary_format(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A]),
            "unknown"
        );
    }

    #[test]
    fn test_sniff_binary_format_image() {
        // 复用 sniff_image_mime：五种图片统一报 "image"
        assert_eq!(sniff_binary_format(b"\x89PNG\r\n\x1A\n\x00"), "image");
        assert_eq!(sniff_binary_format(&[0xFF, 0xD8, 0xFF, 0xE0]), "image");
        assert_eq!(sniff_binary_format(b"GIF89a...."), "image");
        assert_eq!(sniff_binary_format(b"BM\x00\x00\x00\x00"), "image");
        assert_eq!(
            sniff_binary_format(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            "image"
        );
    }

    #[test]
    fn test_sniff_binary_format_archives() {
        assert_eq!(sniff_binary_format(&[0x1F, 0x8B, 0x08, 0x00]), "gzip");
        assert_eq!(sniff_binary_format(b"Rar!\x1A\x07\x00"), "rar");
        assert_eq!(
            sniff_binary_format(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00]),
            "7z"
        );
    }

    #[test]
    fn test_sniff_binary_format_database_and_executables() {
        assert_eq!(sniff_binary_format(b"SQLite format 3\0\x10\x00"), "sqlite");
        assert_eq!(sniff_binary_format(b"MZ\x90\x00\x03\x00"), "pe");
        assert_eq!(
            sniff_binary_format(&[0x7F, 0x45, 0x4C, 0x46, 0x02, 0x01]),
            "elf"
        );
    }

    #[test]
    fn test_sniff_binary_format_unknown() {
        assert_eq!(sniff_binary_format(b""), "unknown", "空切片");
        assert_eq!(sniff_binary_format(b"hello world\n"), "unknown", "纯文本");
        // GBK/GB18030 中文：判定口径不认为是二进制，格式也认不出
        assert_eq!(sniff_binary_format(&[0xD6, 0xD0, 0xCE, 0xC4]), "unknown");
        // UTF-16 BOM（记事本"Unicode"）同样只是 unknown——说明里如实提示了这一限制
        assert_eq!(sniff_binary_format(&[0xFF, 0xFE, 0x61, 0x00]), "unknown");
        // 随机控制字节
        assert_eq!(sniff_binary_format(&[0x01, 0x02, 0x03, 0x04]), "unknown");
    }
}
