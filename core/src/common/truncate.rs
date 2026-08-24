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
}
