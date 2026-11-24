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
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_apply_edits_do_not_lose_updates() {
    // 回归（2026-11-24 玄女 OS 实测）：同一轮里对**同一文件**发两个 `apply_edit`
    // 时，工具调用是并行执行的（`ToolRegistry` 的 `JoinSet`）。两个任务各自
    // 「读全文 → 改 → 整文件写回」，后写的那个基于**旧内容**，于是把前一个的编辑
    // 覆盖掉 —— 而两个调用**都返回成功**。模型看到文件里没有自己刚写的东西，
    // 判成「报告成功但没落盘」，改用 PowerShell 重写。
    //!
    // 多轮是为了提高命中率：单轮里两个 future 有可能恰好串行（那样不丢更新），
    // 而竞态一旦发生就是必丢。
    for round in 0..20 {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("main.rs");
        tokio::fs::write(&path, "mod a;\nmod b;\n\nfn main() {}\n")
            .await
            .expect("写初始文件");
        let p = path.to_string_lossy().to_string();

        let p1 = p.clone();
        let t1 = tokio::spawn(async move {
            apply_edit_action(&p1, vec![edit("mod a;", "mod a;\nmod mm;", false)]).await
        });
        let p2 = p.clone();
        let t2 = tokio::spawn(async move {
            apply_edit_action(
                &p2,
                vec![edit(
                    "fn main() {}",
                    "static MAGIC: u64 = 1;\n\nfn main() {}",
                    false,
                )],
            )
            .await
        });
        let (r1, r2) = (t1.await.expect("任务1"), t2.await.expect("任务2"));
        r1.expect("第一处编辑应成功");
        r2.expect("第二处编辑应成功");

        let after = tokio::fs::read_to_string(&path).await.expect("读回");
        assert!(
            after.contains("mod mm;"),
            "第 {round} 轮：第一条编辑被并发覆盖掉了（这正是「报告成功但文件没变」）：\n{after}"
        );
        assert!(
            after.contains("static MAGIC"),
            "第 {round} 轮：第二条编辑丢了：\n{after}"
        );
    }
}
