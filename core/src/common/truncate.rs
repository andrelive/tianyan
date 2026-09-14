//! 统一文本截断原语（UTF-8 边界安全）。
//!
//! 全库「截断」概念的字节级单点：
//! - 前缀截断 + 省略号：[`truncate_utf8_boundary`]（session 存储 / 可观测性共用）；
//! - 头尾保留式截断：`common::llm_judge::truncate_output`（LLM 摘要；语义不同，原地保留）；
//! - 行/字节级输出截断：`executor::truncate`（truncate_head / truncate_tail / truncate_line）。
//!
//! 新增截断需求一律从这里取，禁止在模块内复制实现。

/// UTF-8 边界安全截断：超过 `max` 字节的文本在字符边界截断并追加省略号。
pub fn truncate_utf8_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
/// 就地保留尾部 `max_bytes` 字节（UTF-8 边界安全）。
///
/// 超出部分从头部丢弃，丢弃起点对齐到最近的字符边界（至多少丢 ≤3 字节）；
/// `max_bytes = 0` 时清空。供"内存快照保留尾部"型截断使用——此前命令输出
/// tail 直接 `String::drain(..excess)`：excess 落在多字节字符中间时 panic
/// （当时 release 为 `panic = "abort"` → 整个进程无痕退出；现已改 `unwind`）。
pub fn truncate_keep_tail_bytes(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s.drain(..start);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_utf8_boundary_short_text_unchanged() {
        assert_eq!(truncate_utf8_boundary("short", 10), "short");
    }

    #[test]
    fn test_truncate_utf8_boundary_multibyte_safe() {
        // 5 字节落在多字节字符中间：必须回退到字符边界且不产生非法 UTF-8
        let t = truncate_utf8_boundary("中文中文", 5);
        assert!(t.ends_with('…'));
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_utf8_boundary_appends_ellipsis() {
        let t = truncate_utf8_boundary("hello 世界 world", 10);
        assert!(t.starts_with("hello "));
        assert!(t.ends_with('…'));
        assert!(
            t.len() <= 10 + '…'.len_utf8(),
            "截断后字节应不超过 max+省略号"
        );
    }

    #[test]
    fn test_truncate_keep_tail_bytes_short_unchanged() {
        let mut s = "short".to_string();
        truncate_keep_tail_bytes(&mut s, 10);
        assert_eq!(s, "short");
    }

    #[test]
    fn test_truncate_keep_tail_bytes_aligns_multibyte() {
        // 32 个"中"（96 字节）+ "xx"（共 98 字节）：保留 4 字节 → 丢弃起点
        // 94 落在"中"#31（93..96）的字符内部，必须向后对齐（旧实现
        // `String::drain(..94)` 在此 panic）。
        let mut s = "中".repeat(32) + "xx";
        truncate_keep_tail_bytes(&mut s, 4);
        assert_eq!(s, "xx");
        assert!(std::str::from_utf8(s.as_bytes()).is_ok());

        // 纯多字节内容：对齐后保留完整字符（3 字节 ≤ 4）
        let mut t = "中".repeat(32);
        truncate_keep_tail_bytes(&mut t, 4);
        assert_eq!(t, "中");
    }

    #[test]
    fn test_truncate_keep_tail_bytes_zero_clears() {
        let mut s = "内容".to_string();
        truncate_keep_tail_bytes(&mut s, 0);
        assert!(s.is_empty());
    }
}
