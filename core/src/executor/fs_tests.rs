//! 文件系统浏览工具核心逻辑测试：glob / list_dir / glob 匹配器。
//!
//! 仅依赖 tempfile 与真实文件系统操作，不访问网络或外部服务。
//! `rg` 可用性通过探测决定断言分支（rg 语义 vs 回退语义）。

use std::path::Path;
use std::time::{Duration, SystemTime};

use super::*;

/// 判断 rg 是否可用（决定 .gitignore 断言走 rg 语义还是回退语义）。
fn rg_available() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 将文件的修改时间设为「距今 age 之前」，用于 mtime 排序断言。
/// Windows 上 `File::open` 为只读句柄，`set_modified` 会拒绝访问，需写权限。
fn age_file(path: &Path, age: Duration) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - age).unwrap();
}

/// 创建文件（自动创建父目录）。
fn create_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// 提取结果中的文件名集合。
fn result_names(out: &GlobOutput) -> Vec<String> {
    out.results
        .iter()
        .map(|p| {
            Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

// ── glob ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_glob_finds_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    create_file(&dir.path().join("a.rs"), "");
    create_file(&dir.path().join("b.rs"), "");
    create_file(&dir.path().join("sub/c.rs"), "");
    create_file(&dir.path().join("sub/d.txt"), "");

    let out = execute_glob("**/*.rs", Some(dir.path())).await.unwrap();
    let names = result_names(&out);
    assert_eq!(out.count, 3);
    assert!(!out.truncated);
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"a.rs".to_string()));
    assert!(names.contains(&"b.rs".to_string()));
    assert!(names.contains(&"c.rs".to_string()));
}

#[tokio::test]
async fn test_glob_pattern_within_subdir() {
    let dir = tempfile::tempdir().unwrap();
    create_file(&dir.path().join("d.txt"), "");
    create_file(&dir.path().join("sub/d.txt"), "");
    create_file(&dir.path().join("sub/c.rs"), "");

    let sub = dir.path().join("sub");
    let out = execute_glob("*.txt", Some(&sub)).await.unwrap();
    assert_eq!(out.count, 1);
    assert_eq!(result_names(&out), vec!["d.txt".to_string()]);
}

#[tokio::test]
async fn test_glob_sorted_by_mtime_desc() {
    let dir = tempfile::tempdir().unwrap();
    create_file(&dir.path().join("a.rs"), "");
    create_file(&dir.path().join("b.rs"), "");
    // a.rs 更旧（10s 前），b.rs 更新（2s 前）→ b.rs 应排第一
    age_file(&dir.path().join("a.rs"), Duration::from_secs(10));
    age_file(&dir.path().join("b.rs"), Duration::from_secs(2));

    let out = execute_glob("**/*.rs", Some(dir.path())).await.unwrap();
    let names = result_names(&out);
    assert_eq!(names, vec!["b.rs".to_string(), "a.rs".to_string()]);
}

#[tokio::test]
async fn test_glob_cap_at_max_results() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..205 {
        create_file(&dir.path().join(format!("file_{i:03}.rs")), "");
    }

    let out = execute_glob("*.rs", Some(dir.path())).await.unwrap();
    assert_eq!(out.count, MAX_GLOB_RESULTS);
    assert_eq!(out.results.len(), MAX_GLOB_RESULTS);
    assert!(out.truncated);
}

#[tokio::test]
async fn test_glob_missing_dir_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let err = execute_glob("**/*.rs", Some(&missing)).await.unwrap_err();
    assert!(
        err.to_string().contains("目录不存在"),
        "应报目录不存在：{err}"
    );
    assert!(
        err.is_not_found(),
        "目录缺失应分类为 not_found（ADR-014）：{err}"
    );
}

#[tokio::test]
async fn test_glob_skips_hidden_and_excluded_dirs() {
    let dir = tempfile::tempdir().unwrap();
    create_file(&dir.path().join("visible.txt"), "");
    create_file(&dir.path().join(".hidden.txt"), "");
    create_file(&dir.path().join("node_modules/x.rs"), "");
    create_file(&dir.path().join("target/y.rs"), "");

    let out = execute_glob("**/*", Some(dir.path())).await.unwrap();
    assert_eq!(result_names(&out), vec!["visible.txt".to_string()]);
}

#[tokio::test]
async fn test_glob_gitignore_or_fallback() {
    let dir = tempfile::tempdir().unwrap();
    create_file(&dir.path().join(".gitignore"), "ignored.txt\n");
    create_file(&dir.path().join("ignored.txt"), "");
    create_file(&dir.path().join("kept.txt"), "");

    let out = execute_glob("**/*", Some(dir.path())).await.unwrap();
    let names = result_names(&out);
    if rg_available() {
        // rg 语义：.gitignore 生效，ignored.txt 被排除
        assert!(
            !names.contains(&"ignored.txt".to_string()),
            "rg 模式下 ignored.txt 应被 .gitignore 排除：{names:?}"
        );
        assert!(names.contains(&"kept.txt".to_string()));
    } else {
        // 回退语义（文档化行为）：手工遍历不支持 .gitignore，两文件都出现
        assert!(names.contains(&"ignored.txt".to_string()));
        assert!(names.contains(&"kept.txt".to_string()));
    }
}

#[tokio::test]
async fn test_glob_empty_dir_returns_empty_results() {
    let dir = tempfile::tempdir().unwrap();
    let out = execute_glob("**/*.rs", Some(dir.path())).await.unwrap();
    assert_eq!(out.count, 0);
    assert!(out.results.is_empty());
    assert!(!out.truncated);
}

// ── list_dir ────────────────────────────────────────────────────────────

/// 创建混合目录（dirs: another_dir/subdir, files: a.txt/z.txt）。
fn mixed_dir(dir: &Path) {
    std::fs::create_dir_all(dir.join("subdir")).unwrap();
    std::fs::create_dir_all(dir.join("another_dir")).unwrap();
    create_file(&dir.join("z.txt"), "");
    create_file(&dir.join("a.txt"), "");
}

#[tokio::test]
async fn test_list_dir_dirs_first_then_alpha() {
    let dir = tempfile::tempdir().unwrap();
    mixed_dir(dir.path());

    let out = execute_list_dir(dir.path(), None, None).await.unwrap();
    let names: Vec<String> = out.entries.iter().map(|e| e.name.clone()).collect();
    let types: Vec<&str> = out.entries.iter().map(|e| e.entry_type.as_str()).collect();
    assert_eq!(names, vec!["another_dir/", "subdir/", "a.txt", "z.txt"]);
    assert_eq!(types, vec!["dir", "dir", "file", "file"]);
    assert_eq!(out.count, 4);
    assert_eq!(out.total, 4);
    assert!(!out.truncated);
    // 目录条目路径为完整绝对路径
    assert!(out.entries[0].path.ends_with("another_dir"));
}

#[tokio::test]
async fn test_list_dir_pagination() {
    let dir = tempfile::tempdir().unwrap();
    mixed_dir(dir.path());

    // offset=1, limit=2 → [subdir/, a.txt]，截断（1+2 < 4）
    let out = execute_list_dir(dir.path(), Some(1), Some(2))
        .await
        .unwrap();
    let names: Vec<String> = out.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["subdir/".to_string(), "a.txt".to_string()]);
    assert_eq!(out.count, 2);
    assert_eq!(out.total, 4);
    assert!(out.truncated);

    // offset=0, limit=2 → 前两个
    let out = execute_list_dir(dir.path(), Some(0), Some(2))
        .await
        .unwrap();
    let names: Vec<String> = out.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(
        names,
        vec!["another_dir/".to_string(), "subdir/".to_string()]
    );
    assert!(out.truncated);

    // offset=3, limit=10 → 最后一个，不截断（3+10 >= 4）
    let out = execute_list_dir(dir.path(), Some(3), Some(10))
        .await
        .unwrap();
    let names: Vec<String> = out.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["z.txt".to_string()]);
    assert!(!out.truncated);
}

#[tokio::test]
async fn test_list_dir_nonexistent_path_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let err = execute_list_dir(&missing, None, None).await.unwrap_err();
    assert!(
        err.to_string().contains("目录不存在"),
        "应报目录不存在：{err}"
    );
    assert!(
        err.is_not_found(),
        "目录缺失应分类为 not_found（ADR-014）：{err}"
    );
}

// ── glob 匹配器（回退路径核心） ─────────────────────────────────────────

#[test]
fn test_glob_match_core_cases() {
    assert!(glob_match("**/*.rs", "a.rs"));
    assert!(glob_match("**/*.rs", "sub/c.rs"));
    assert!(!glob_match("*.rs", "sub/c.rs"), "* 不应跨目录段");
    assert!(glob_match("*.toml", "Cargo.toml"));
    assert!(glob_match("src/**", "src/lib.rs"));
    assert!(glob_match("**", "deep/nested/file.txt"));
}

#[test]
fn test_glob_match_wildcards_and_classes() {
    assert!(glob_match("?.rs", "a.rs"));
    assert!(!glob_match("?.rs", "ab.rs"));
    assert!(glob_match("[ab].rs", "a.rs"));
    assert!(glob_match("[ab].rs", "b.rs"));
    assert!(!glob_match("[ab].rs", "c.rs"));
    assert!(glob_match("[!ab].rs", "c.rs"));
    assert!(!glob_match("[!ab].rs", "a.rs"));
    assert!(glob_match("file[0-9].txt", "file5.txt"));
    assert!(!glob_match("file[0-9].txt", "filex.txt"));
    assert!(glob_match("literal.txt", "literal.txt"));
    assert!(!glob_match("literal.txt", "other.txt"));
}
