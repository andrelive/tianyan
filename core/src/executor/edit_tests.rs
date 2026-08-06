//! `apply_edits_to_content` 纯函数测试（无文件系统依赖）。
//!
//! 覆盖：中间行编辑、自底向上批量应用、重叠检测、过期锚点、
//! 旧内容校验、越界起始行、删除、CRLF 行尾保持、空白不敏感锚点。

use super::*;
use crate::executor::hashline;

/// 便捷构造编辑规格。
fn spec(start_line: usize, old_lines: Option<Vec<&str>>, new_lines: Vec<&str>) -> EditSpec {
    EditSpec {
        start_line,
        anchor: None,
        old_lines: old_lines.map(|v| v.into_iter().map(str::to_string).collect()),
        new_lines: new_lines.into_iter().map(str::to_string).collect(),
    }
}

#[test]
fn middle_of_file_edit_produces_new_content() {
    let content = "line1\nline2\nline3\n";
    let edits = vec![EditSpec {
        start_line: 2,
        anchor: Some(hashline::line_hash("line2")),
        old_lines: None,
        new_lines: vec!["LINE2".to_string()],
    }];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "line1\nLINE2\nline3\n");
}

#[test]
fn two_non_overlapping_edits_applied_bottom_up() {
    let content = "a\nb\nc\nd\n";
    let edits = vec![spec(1, None, vec!["A"]), spec(3, None, vec!["C"])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    // 自底向上：先改第 3 行、再改第 1 行，行号互不影响
    assert_eq!(result, "A\nb\nC\nd\n");
}

#[test]
fn overlapping_edits_on_same_line_error() {
    let content = "a\nb\nc\n";
    let edits = vec![spec(2, None, vec!["X"]), spec(2, None, vec!["Y"])];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    assert!(
        err.to_string().contains("编辑重叠"),
        "错误应指明重叠: {err}"
    );
}

#[test]
fn overlapping_range_edits_error() {
    // 第 1 处编辑覆盖 1-2 行，第 2 处编辑起始于第 2 行 → 范围相交
    let content = "a\nb\nc\n";
    let edits = vec![
        spec(1, Some(vec!["a", "b"]), vec!["AB"]),
        spec(2, None, vec!["X"]),
    ];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    assert!(
        err.to_string().contains("编辑重叠"),
        "错误应指明重叠: {err}"
    );
}

#[test]
fn stale_anchor_errors_with_latest_anchor() {
    let content = "a\nb\nc\n";
    let edits = vec![EditSpec {
        start_line: 2,
        anchor: Some("00".to_string()),
        old_lines: None,
        new_lines: vec!["X".to_string()],
    }];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("锚点不匹配"), "应报锚点不匹配: {msg}");
    assert!(msg.contains("最新锚点为"), "应回传最新锚点: {msg}");
    let expected_anchor = hashline::format_line(2, &hashline::line_hash("b"), "b");
    assert!(
        msg.contains(&expected_anchor),
        "最新锚点应为 {expected_anchor}: {msg}"
    );
}

#[test]
fn old_lines_mismatch_errors() {
    let content = "a\nb\nc\n";
    let edits = vec![spec(2, Some(vec!["zzz"]), vec!["X"])];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    assert!(
        err.to_string().contains("旧内容不匹配"),
        "错误应指明旧内容不符: {err}"
    );
}

#[test]
fn start_line_beyond_total_errors() {
    let content = "a\nb\n";
    let edits = vec![spec(5, None, vec!["X"])];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("起始行"), "错误应点名起始行: {msg}");
    assert!(msg.contains("超出文件总行数"), "错误应说明越界: {msg}");
}

#[test]
fn start_line_zero_errors() {
    let content = "a\nb\n";
    let edits = vec![spec(0, None, vec!["X"])];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    assert!(
        err.to_string().contains("起始行"),
        "错误应点名起始行: {err}"
    );
}

#[test]
fn deletion_with_empty_new_lines_removes_lines() {
    let content = "a\nb\nc\n";
    let edits = vec![spec(2, None, vec![])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "a\nc\n");
}

#[test]
fn old_lines_range_replaced_by_fewer_lines() {
    let content = "a\nb\nc\nd\n";
    let edits = vec![spec(2, Some(vec!["b", "c"]), vec!["BC"])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "a\nBC\nd\n");
}

#[test]
fn crlf_content_preserves_eol() {
    let content = "a\r\nb\r\n";
    let edits = vec![spec(1, None, vec!["new"])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "new\r\nb\r\n");
}

#[test]
fn crlf_mixed_line_edit_keeps_other_lines() {
    let content = "a\r\nb\r\nc";
    let edits = vec![spec(2, None, vec!["B"])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "a\r\nB\r\nc");
}

#[test]
fn whitespace_insensitive_anchor_matches() {
    // 文件中行首有缩进，但锚点基于无缩进内容计算 → 哈希一致
    let content = "  let x = 1;\n";
    let edits = vec![EditSpec {
        start_line: 1,
        anchor: Some(hashline::line_hash("let x = 1;")),
        old_lines: None,
        new_lines: vec!["let x = 2;".to_string()],
    }];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "let x = 2;\n");
}

#[test]
fn empty_edits_list_errors() {
    let content = "a\n";
    let err = apply_edits_to_content(content, &[]).unwrap_err();
    assert!(
        err.to_string().contains("至少需要 1 处编辑"),
        "错误应说明编辑为空: {err}"
    );
}

#[test]
fn too_many_edits_errors() {
    let content = (1..=25)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let edits: Vec<EditSpec> = (1..=25).map(|i| spec(i, None, vec!["x"])).collect();
    let err = apply_edits_to_content(&content, &edits).unwrap_err();
    assert!(
        err.to_string().contains("超过上限"),
        "错误应说明数量上限: {err}"
    );
}

#[test]
fn no_trailing_newline_input_keeps_no_trailing_newline() {
    let content = "a\nb\nc";
    let edits = vec![spec(2, None, vec!["B"])];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "a\nB\nc");
}
