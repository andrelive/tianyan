//! truncate 单元测试：统一截断层（head / tail / spill）。

use super::*;

fn n_lines(n: usize) -> String {
    (1..=n)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 短文本（10 行）：原样返回，truncated=false。
#[test]
fn short_text_head_is_unchanged() {
    let text = n_lines(10);
    let r = truncate_head(&text);
    assert!(!r.truncated);
    assert_eq!(r.text, text);
    assert_eq!(r.total_lines, 10);
    assert_eq!(r.total_bytes, text.len());
    assert!(r.spill_path.is_none());
}

/// 长文本（3000 行）：head 保留 2000 行，truncated=true，末尾带标记。
#[test]
fn long_text_head_keeps_2000_lines_with_marker() {
    let text = n_lines(3000);
    let r = truncate_head(&text);
    assert!(r.truncated);
    assert_eq!(r.total_lines, 3000);
    assert_eq!(r.total_bytes, text.len());
    let marker = format!(
        "... (输出已截断，共 3000 行 {} 字节，使用 offset 继续)",
        text.len()
    );
    assert!(
        r.text.ends_with(&marker),
        "marker missing or wrong, got tail: {:?}",
        &r.text[r.text.len().saturating_sub(80)..]
    );
    assert!(r.text.lines().any(|l| l == "line 1"));
    assert!(r.text.lines().any(|l| l == "line 2000"));
    assert!(!r.text.lines().any(|l| l == "line 2001"));
    assert_eq!(r.text.lines().count(), 2001); // 2000 行 + 1 标记行
}

/// 恰好等于上限的行数（2000 行）：不应被截断。
#[test]
fn exactly_max_lines_is_not_truncated() {
    let text = n_lines(MAX_LINES);
    let r = truncate_head(&text);
    assert!(!r.truncated);
    assert_eq!(r.text, text);
}

/// 超长单行（70KB）：字节上限生效，不 panic。
#[test]
fn long_single_line_byte_cap_no_panic() {
    let text = "x".repeat(70 * 1024);
    let r = truncate_head(&text);
    assert!(r.truncated);
    assert_eq!(r.total_lines, 1);
    assert_eq!(r.total_bytes, text.len());
    assert!(r.text.contains("输出已截断"));
    assert!(r.text.len() <= MAX_BYTES + 128);
}

/// T2 回归：单行超长时头部/尾部截断必须保留**片段内容**——
/// 此前按行累加的首行即超限 → kept 为空 → 输出只剩截断标记（零内容）。
#[test]
fn single_huge_line_keeps_fragment_head_and_tail() {
    let text = format!("HEAD{}TAIL", "x".repeat(70 * 1024));

    let h = truncate_head(&text);
    assert!(h.truncated);
    assert!(
        h.text.starts_with("HEAD"),
        "头部截断应保留片段开头（而非零内容）"
    );

    let t = truncate_tail(&text);
    assert!(t.truncated);
    assert!(
        t.text.contains("TAIL"),
        "尾部截断应保留片段结尾（而非零内容）"
    );
    assert!(t.text.len() > 1000, "应保留可观内容：{} 字节", t.text.len());
}

/// CJK 多字节边界：30k 个"天"单行不 panic、不切分字符。
#[test]
fn cjk_single_line_never_panics_or_splits() {
    let text = "天".repeat(30_000); // 90 KB
    let r = truncate_head(&text);
    assert!(r.truncated);
    assert!(r.text.contains("输出已截断"));
    for line in r.text.lines().filter(|l| !l.starts_with("...")) {
        assert!(
            line.chars().all(|c| c == '天'),
            "char split or garbage line: {:?}",
            line
        );
    }
}

/// CJK 多行文本：截断后的保留行都是完整字符且字节不超限。
#[test]
fn cjk_multi_line_kept_lines_are_intact() {
    let line = "天".repeat(200); // 600 字节 / 行
    let text = (0..1000)
        .map(|_| line.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let r = truncate_head(&text);
    assert!(r.truncated);
    assert!(r.text.contains("输出已截断"));
    let mut kept_bytes = 0usize;
    for l in r.text.lines().filter(|l| !l.starts_with("...")) {
        assert_eq!(l, line, "kept line must be a full 200-char 天 line");
        kept_bytes += l.len() + 1;
    }
    assert!(kept_bytes <= MAX_BYTES);
}

/// truncate_tail 保留末尾行：包含 line 3000，不含 line 1。
#[test]
fn tail_keeps_last_lines() {
    let text = n_lines(3000);
    let r = truncate_tail(&text);
    assert!(r.truncated);
    assert_eq!(r.total_lines, 3000);
    let marker = format!("... (输出已截断，共 3000 行 {} 字节)", text.len());
    assert!(
        r.text.starts_with(&marker),
        "marker missing or wrong, got head: {:?}",
        &r.text[..r.text.len().min(80)]
    );
    assert!(r.text.lines().any(|l| l == "line 3000"));
    assert!(!r.text.lines().any(|l| l == "line 1"));
    assert_eq!(r.text.lines().count(), 2001); // 标记行 + 2000 行
}

/// truncate_tail 对短文本原样返回。
#[test]
fn tail_short_text_is_unchanged() {
    let text = n_lines(10);
    let r = truncate_tail(&text);
    assert!(!r.truncated);
    assert_eq!(r.text, text);
}

/// truncate_spill 写入完整原文到溢出文件，并设置 spill_path。
#[tokio::test]
async fn spill_writes_full_text_and_sets_path() {
    let text = n_lines(3000);
    let dir = std::env::temp_dir().join(format!("tianyan-truncate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let r = truncate_spill(&text, &dir, "spilltest").await.unwrap();
    assert!(r.truncated);
    assert!(r.text.contains("输出已截断"));
    assert!(r.text.contains("全文已保存至"));
    let path = r.spill_path.clone().expect("spill_path must be Some");
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        written, text,
        "spill file must contain the full original text"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}
