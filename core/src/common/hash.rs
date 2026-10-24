//! 内容摘要（SHA-256 十六进制）。

use ring::digest::{digest, SHA256};

/// 计算字节的 SHA-256 十六进制摘要（小写，64 字符）。
pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = digest(&SHA256, bytes);
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_sha256_empty_known_value() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_hex_sha256_is_lowercase_64_chars_and_content_sensitive() {
        let a = hex_sha256(b"hello");
        let b = hex_sha256(b"hello ");
        assert_eq!(a.len(), 64, "十六进制摘要恒为 64 字符");
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        assert_ne!(a, b, "内容不同 → 摘要不同");
    }
}
