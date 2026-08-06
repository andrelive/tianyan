//! hashline 单元测试：行锚点哈希 / 格式化 / 解析。

use super::*;

/// 仅空白不同的行必须得到相同哈希。
#[test]
fn whitespace_variants_hash_same() {
    let a = line_hash("let x = 1;");
    let b = line_hash("  let x = 1;  ");
    let c = line_hash("let\tx = 1;");
    assert_eq!(a, b);
    assert_eq!(a, c);
}

/// 任意输入哈希必须恰好是 2 位小写十六进制字符。
#[test]
fn hash_is_exactly_two_lowercase_hex_chars() {
    for input in [
        "",
        "a",
        "let x = 1;",
        "fn main() {}",
        "汉字内容",
        "  spaced  ",
    ] {
        let h = line_hash(input);
        assert_eq!(
            h.len(),
            2,
            "hash of {:?} must be 2 chars, got {:?}",
            input,
            h
        );
        assert!(
            h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "hash of {:?} must be lowercase hex, got {:?}",
            input,
            h
        );
    }
}

/// format_line 生成 `N#ID|content`，parse_anchor 能往返解析。
#[test]
fn format_line_round_trips_with_parse_anchor() {
    let line = format_line(42, "3f", "let x = 1;");
    assert_eq!(line, "42#3f|let x = 1;");
    assert_eq!(parse_anchor(&line), Some((42, "3f".to_string())));
}

/// 无 `#`..`|` 模式的行返回 None。
#[test]
fn parse_anchor_returns_none_without_pattern() {
    assert!(parse_anchor("plain line without anchor").is_none());
    assert!(parse_anchor("").is_none());
    assert!(parse_anchor("#3f|no number").is_none());
    assert!(parse_anchor("5#id-without-pipe").is_none());
    assert!(parse_anchor("abc#3f|x").is_none());
}

/// 相同输入两次哈希结果一致。
#[test]
fn hash_is_deterministic() {
    let input = "let x = 1;";
    assert_eq!(line_hash(input), line_hash(input));
    let cjk = "fn 测试() -> i32 { 1 }";
    assert_eq!(line_hash(cjk), line_hash(cjk));
}

/// hash_line_pair 组合 format_line 与 line_hash。
#[test]
fn hash_line_pair_formats_with_hash_id() {
    let pair = hash_line_pair(7, "let x = 1;");
    assert_eq!(pair, format!("7#{}|let x = 1;", line_hash("let x = 1;")));
    assert_eq!(parse_anchor(&pair), Some((7, line_hash("let x = 1;"))));
}
