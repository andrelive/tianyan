//! 搜索执行器测试：内嵌引擎 + 临时目录夹具（无外部 rg 依赖）。

use super::*;
use tempfile::TempDir;

/// 基础夹具：a.rs（两行匹配）+ b.txt（一行匹配），均含 "foo"。
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.rs"),
        "fn foo() {\n    // comment foo\n    let x = 1;\n}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("b.txt"), "foo here\n").unwrap();
    dir
}

/// content 模式的默认选项（其余字段取默认）。
fn content_opts(dir: &TempDir) -> SearchOptions {
    SearchOptions {
        path: Some(dir.path().to_string_lossy().into_owned()),
        output_mode: Some(OutputMode::Content),
        ..Default::default()
    }
}

// ── 基本搜索 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn content_search_returns_line_number_and_text() {
    let dir = fixture();
    let opts = content_opts(&dir);
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["output_mode"].as_str(), Some("content"));
    assert_eq!(out["count"].as_u64(), Some(3)); // a.rs 两行 + b.txt 一行
    assert_eq!(out["total"].as_u64(), Some(3));
    assert_eq!(out["path"].as_str(), Some(dir.path().to_str().unwrap()));
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    // rg 文件遍历顺序不保证：按路径定位 a.rs 记录再断言。
    let a_rs = results
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("a.rs"))
        .expect("应含 a.rs 记录");
    assert_eq!(a_rs["line_number"].as_u64(), Some(1));
    assert!(a_rs["text"].as_str().unwrap().contains("fn foo()"));
    let matches = a_rs["matches"].as_array().unwrap();
    assert_eq!(matches[0]["start"].as_u64(), Some(3));
    assert_eq!(matches[0]["end"].as_u64(), Some(6));
}

#[tokio::test]
async fn default_mode_lists_files_with_matches() {
    let dir = fixture();
    let opts = SearchOptions {
        path: Some(dir.path().to_string_lossy().into_owned()),
        ..Default::default()
    };
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["output_mode"].as_str(), Some("files_with_matches"));
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 2); // a.rs + b.txt，每个文件一条
    assert_eq!(out["count"].as_u64(), Some(2));
    for r in results {
        assert_eq!(r["line_number"].as_u64(), Some(1));
        assert_eq!(r["text"].as_str(), Some(""));
        assert!(r["matches"].as_array().unwrap().is_empty());
    }
}

// ── 高级参数 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn ignore_case_matches_lowercase() {
    let dir = fixture();
    let mut opts = content_opts(&dir);
    opts.ignore_case = true;
    let out = execute_search_code("FOO", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(3)); // a.rs 两行 + b.txt 一行
    assert!(out["results"][0]["text"].as_str().unwrap().contains("foo"));
}

#[tokio::test]
async fn context_includes_neighbor_lines() {
    let dir = fixture();
    let mut opts = content_opts(&dir);
    opts.context = Some(1);
    // "comment" 仅命中 a.rs 第 2 行：-C 1 应携带第 1、3 行。
    let out = execute_search_code("comment", &opts).await.unwrap();
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let text = results[0]["text"].as_str().unwrap();
    assert!(text.contains("fn foo() {"), "应含前一行: {text}");
    assert!(text.contains("let x = 1;"), "应含后一行: {text}");
    assert!(text.contains("// comment foo"), "应含匹配行: {text}");
}

#[tokio::test]
async fn glob_filter_limits_file_types() {
    let dir = fixture();
    let mut opts = content_opts(&dir);
    opts.glob = Some("*.rs".to_string());
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(2)); // 仅 a.rs；b.txt 被 glob 过滤
    for r in out["results"].as_array().unwrap() {
        assert!(r["path"].as_str().unwrap().ends_with(".rs"));
    }
}

#[tokio::test]
async fn type_filter_restricts_language() {
    let dir = fixture();
    let mut opts = content_opts(&dir);
    opts.type_ = Some("rust".to_string());
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(2)); // 仅 a.rs（rust 类型）
    for r in out["results"].as_array().unwrap() {
        assert!(r["path"].as_str().unwrap().ends_with(".rs"));
    }
}

// ── 错误与边界 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn invalid_regex_reports_error() {
    let dir = fixture();
    let opts = content_opts(&dir);
    let err = execute_search_code("[", &opts).await.unwrap_err();
    assert!(err.to_string().contains("正则无效"), "应报正则无效: {err}");
}

#[tokio::test]
async fn no_matches_returns_empty_not_exhausted() {
    let dir = fixture();
    let opts = content_opts(&dir);
    let out = execute_search_code("zzzz_nothing", &opts).await.unwrap();
    assert_eq!(out["results"].as_array().unwrap().len(), 0);
    assert_eq!(out["count"].as_u64(), Some(0));
    assert_eq!(out["total"].as_u64(), Some(0));
    assert_eq!(out["truncated"].as_bool(), Some(false));
    assert_eq!(out["exhausted"].as_bool(), Some(false));
}

#[tokio::test]
async fn offset_beyond_total_is_exhausted() {
    let dir = fixture();
    let mut opts = content_opts(&dir);
    opts.offset = 5; // 仅有 2 条匹配
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(0));
    assert_eq!(out["total"].as_u64(), Some(3)); // a.rs 两行 + b.txt 一行
    assert_eq!(out["truncated"].as_bool(), Some(false));
    assert_eq!(out["exhausted"].as_bool(), Some(true));
}

// ── 分页 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn pagination_applies_offset_and_head_limit() {
    let dir = tempfile::tempdir().unwrap();
    let lines: Vec<String> = (1..=5).map(|i| format!("match line {i}")).collect();
    std::fs::write(dir.path().join("p.rs"), lines.join("\n") + "\n").unwrap();

    let mut first_opts = content_opts(&dir);
    first_opts.head_limit = Some(2);
    let first = execute_search_code("match", &first_opts).await.unwrap();
    assert_eq!(first["count"].as_u64(), Some(2));
    assert_eq!(first["total"].as_u64(), Some(5));
    assert_eq!(first["truncated"].as_bool(), Some(true));
    assert_eq!(first["exhausted"].as_bool(), Some(false));

    let mut second_opts = content_opts(&dir);
    second_opts.head_limit = Some(2);
    second_opts.offset = 2;
    let second = execute_search_code("match", &second_opts).await.unwrap();
    assert_eq!(second["count"].as_u64(), Some(2));
    assert_eq!(second["total"].as_u64(), Some(5));
    let line_numbers: Vec<u64> = second["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["line_number"].as_u64().unwrap())
        .collect();
    assert_eq!(line_numbers, vec![3, 4]); // 第 3、4 条匹配
}

// ── 输出守卫 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn long_line_is_truncated_with_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("long.rs"),
        format!("// {}\n", "x".repeat(2500)),
    )
    .unwrap();
    let opts = content_opts(&dir);
    let out = execute_search_code("xxxx", &opts).await.unwrap();
    let results = out["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let text = results[0]["text"].as_str().unwrap();
    assert!(
        text.contains(crate::executor::truncate::TRUNCATED_MARKER),
        "应含截断标记: {text}"
    );
    assert!(text.len() < 2500, "应被截断: len={}", text.len());
}

#[tokio::test]
async fn multiline_pattern_matches_across_lines() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.rs"), "let a = 1;\nlet b = 2;\n").unwrap();
    let mut opts = content_opts(&dir);
    opts.multiline = true;
    let out = execute_search_code("a = 1;\nlet b", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(1));
    let text = out["results"][0]["text"].as_str().unwrap();
    assert!(text.contains("let b = 2;"), "跨行文本: {text}");
}

#[tokio::test]
async fn git_directory_is_excluded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ok.rs"), "fn foo() {}\n").unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    std::fs::write(
        dir.path().join(".git").join("hooks").join("x.rs"),
        "fn foo() {}\n",
    )
    .unwrap();
    let opts = content_opts(&dir);
    let out = execute_search_code("foo", &opts).await.unwrap();
    assert_eq!(out["count"].as_u64(), Some(1), "仅 ok.rs");
    for r in out["results"].as_array().unwrap() {
        assert!(
            !r["path"].as_str().unwrap().contains(".git"),
            "不应包含 .git 路径"
        );
    }
}
