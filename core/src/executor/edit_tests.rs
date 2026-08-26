//! apply_edits_to_content 纯函数测试（内容匹配 old_string/new_string）。
//!
//! 覆盖：唯一匹配替换、多行块替换、未找到、多处不唯一、replace_all、
//! 自底向上批量、重叠检测、空编辑、数量上限、空 old_string、空白敏感匹配。

use super::*;

fn edit(old_string: &str, new_string: &str, replace_all: bool) -> ContentEdit {
    ContentEdit {
        old_string: old_string.to_string(),
        new_string: new_string.to_string(),
        replace_all: replace_all.then_some(true),
    }
}

#[test]
fn unique_replacement() {
    let content = "let x = 1;\n";
    let result =
        apply_edits_to_content(content, &[edit("let x = 1;", "let x = 2;", false)]).unwrap();
    assert_eq!(result, "let x = 2;\n");
}

#[test]
fn multi_line_block_replacement() {
    let content = "a\nold1\nold2\nc\n";
    let result = apply_edits_to_content(content, &[edit("old1\nold2", "NEW", false)]).unwrap();
    assert_eq!(result, "a\nNEW\nc\n");
}

#[test]
fn not_found_errors() {
    let content = "a\nb\n";
    let err = apply_edits_to_content(content, &[edit("zzz", "X", false)]).unwrap_err();
    assert!(err.to_string().contains("未找到"), "应报未找到: {err}");
}

#[test]
fn multiple_matches_without_replace_all_errors() {
    let content = "x\nx\n";
    let err = apply_edits_to_content(content, &[edit("x", "X", false)]).unwrap_err();
    assert!(err.to_string().contains("不唯一"), "应报不唯一: {err}");
}

#[test]
fn replace_all_replaces_all() {
    let content = "x\nx\nx\n";
    let result = apply_edits_to_content(content, &[edit("x", "X", true)]).unwrap();
    assert_eq!(result, "X\nX\nX\n");
}

#[test]
fn two_non_overlapping_edits_applied() {
    let content = "a\nb\nc\n";
    let edits = vec![edit("a", "A", false), edit("c", "C", false)];
    let result = apply_edits_to_content(content, &edits).unwrap();
    assert_eq!(result, "A\nb\nC\n");
}

#[test]
fn overlapping_edits_error() {
    let content = "abc\n";
    let edits = vec![edit("ab", "X", false), edit("bc", "Y", false)];
    let err = apply_edits_to_content(content, &edits).unwrap_err();
    assert!(err.to_string().contains("编辑重叠"), "应报重叠: {err}");
}

#[test]
fn empty_edits_list_errors() {
    let err = apply_edits_to_content("a\n", &[]).unwrap_err();
    assert!(
        err.to_string().contains("至少需要 1 处编辑"),
        "应报编辑为空: {err}"
    );
}

#[test]
fn too_many_edits_errors() {
    let content = (1..=25)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let edits: Vec<ContentEdit> = (1..=25)
        .map(|i| edit(&format!("line{i}"), "x", false))
        .collect();
    let err = apply_edits_to_content(&content, &edits).unwrap_err();
    assert!(err.to_string().contains("超过上限"), "应报数量上限: {err}");
}

#[test]
fn empty_old_string_errors() {
    let edits = vec![ContentEdit {
        old_string: "".to_string(),
        new_string: "b".to_string(),
        replace_all: None,
    }];
    let err = apply_edits_to_content("a", &edits).unwrap_err();
    assert!(
        err.to_string().contains("不能为空"),
        "应报 old_string 为空: {err}"
    );
}

#[test]
fn content_match_is_substring() {
    // 内容匹配是子串匹配：old_string 只需是文件的一部分（行首缩进被保留）
    let content = "  let x = 1;\n";
    let result =
        apply_edits_to_content(content, &[edit("let x = 1;", "let x = 2;", false)]).unwrap();
    assert_eq!(result, "  let x = 2;\n");
}

#[test]
fn replace_all_old_contained_in_new_applies_once() {
    let content = "a\n";
    let result = apply_edits_to_content(content, &[edit("a", "aa", true)]).unwrap();
    assert_eq!(result, "aa\n", "不循环应用");
}

#[test]
fn crlf_content_matches_lf_old_string() {
    // CRLF 文件 + 模型用 \n 写的 old_string：行尾归一化后应能匹配
    let content = "alpha\r\nbeta\r\n";
    let result = apply_edits_to_content(content, &[edit("beta", "BETA", false)]).unwrap();
    assert_eq!(result, "alpha\r\nBETA\r\n", "写回应恢复 CRLF");
}

#[test]
fn crlf_multiline_old_string_matches() {
    let content = "a\r\nold1\r\nold2\r\nc\r\n";
    let result = apply_edits_to_content(content, &[edit("old1\nold2", "NEW", false)]).unwrap();
    assert_eq!(
        result, "a\r\nNEW\r\nc\r\n",
        "多行 old_string 用 \n 也能匹配 CRLF 文件"
    );
}
