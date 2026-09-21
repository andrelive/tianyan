//! repo_map 引擎测试：扫描 / 引用度排序 / focus / 预算截断 / 缓存失效 / 噪声过滤。

use super::*;
use std::fs;
use tempfile::TempDir;

fn write(dir: &TempDir, rel: &str, content: &str) {
    let path = dir.path().join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn scan_fixture(dir: &TempDir, include_tests: bool) -> ScanOutcome {
    scan(&ScanOptions {
        root: dir.path().to_path_buf(),
        include_tests,
    })
}

/// 手工构造扫描产物（排序/渲染测试用，避免解析噪声干扰）。
fn outcome_of(entries: &[(&str, usize, SymbolKind, &str)]) -> ScanOutcome {
    let mut definitions = Vec::new();
    let mut references = HashMap::new();
    for (path, line, kind, name) in entries {
        definitions.push(Definition {
            path: (*path).to_string(),
            line: *line,
            kind: *kind,
            name: (*name).to_string(),
        });
        // references 含定义处自身 1 次（render 会扣 1）
        *references.entry((*name).to_string()).or_insert(0) += 1;
    }
    ScanOutcome {
        files: 1,
        skipped: 0,
        definitions,
        references,
    }
}

// ── 扫描 ───────────────────────────────────────────────────────────────

#[test]
fn test_scan_collects_definitions_across_files() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "src/a.rs", "pub struct Alpha;\npub fn helper() {}\n");
    write(&dir, "src/b.rs", "fn caller() { helper(); }\n");

    let outcome = scan_fixture(&dir, false);
    assert_eq!(outcome.files, 2, "两个源文件都应参与扫描");
    let names: Vec<&str> = outcome
        .definitions
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(names.contains(&"Alpha"), "{names:?}");
    assert!(names.contains(&"helper"), "{names:?}");
    assert!(names.contains(&"caller"), "{names:?}");
    // helper 出现 2 次：定义 + b.rs 调用
    assert_eq!(outcome.references.get("helper").copied(), Some(2));
    // 路径为相对 root 且用 `/` 分隔
    assert!(
        outcome.definitions.iter().any(|d| d.path == "src/a.rs"),
        "路径应为相对路径：{:?}",
        outcome.definitions
    );
}

#[test]
fn test_scan_ignores_identifiers_in_comments_and_strings() {
    // 判别力（相对 grep 的本质优势）：注释与字符串里的同名内容不算引用，
    // 若被计入则 references 会多出 2 次。
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "src/a.rs",
        "// needle_fn 出现在注释里\npub fn needle_fn() {}\n",
    );
    write(
        &dir,
        "src/b.rs",
        "const S: &str = \"needle_fn\";\nfn call() { needle_fn(); }\n",
    );

    let outcome = scan_fixture(&dir, false);
    assert_eq!(
        outcome.references.get("needle_fn").copied(),
        Some(2),
        "只应计「定义 + 真实调用」两次（注释与字符串不计）：{:?}",
        outcome.references.get("needle_fn")
    );
}

#[test]
fn test_scan_excludes_test_files_by_default() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "src/lib.rs", "pub fn prod_fn() {}\n");
    write(&dir, "src/lib_tests.rs", "fn test_only() {}\n");
    write(&dir, "tests/integration.rs", "fn integration_only() {}\n");

    let excluded = scan_fixture(&dir, false);
    let names: Vec<String> = excluded
        .definitions
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert!(names.contains(&"prod_fn".to_string()));
    assert!(
        !names.contains(&"test_only".to_string()),
        "默认应排除 *_tests.rs：{names:?}"
    );
    assert!(!names.contains(&"integration_only".to_string()));

    let included = scan_fixture(&dir, true);
    assert!(included.files > excluded.files, "开启后应多扫到测试文件");
}

#[test]
fn test_is_test_file_rules() {
    let cases = [
        ("core/src/x_tests.rs", true),
        ("core/src/y_test.rs", true),
        ("tests/integration.rs", true),
        ("gui/__tests__/a.tsx", true),
        ("pkg/test/test_x.py", true),
        ("pkg/test_x.py", true),
        ("pkg/x_test.py", true),
        ("web/a.test.ts", true),
        ("web/b.spec.ts", true),
        // 反例（不是测试文件，不能被误判）
        ("core/src/executor/test_discovery.rs", false),
        ("core/src/agent/tool_registry/test_ops.rs", false),
        ("core/src/lib.rs", false),
        ("docs/notes.md", false),
    ];
    for (rel, expected) in cases {
        let path = Path::new(rel);
        assert_eq!(is_test_file(rel, path), expected, "{rel}");
    }
}

#[test]
fn test_scan_counts_skipped_source_files_only() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "src/a.rs", "pub fn ok_fn() {}\n");
    write(&dir, "src/a_tests.rs", "fn excluded() {}\n");
    // 非支持语言：遍历阶段即过滤，既不计入 files 也不计入 skipped
    write(&dir, "README.md", "# docs\n");
    write(&dir, "Cargo.toml", "[package]\n");

    let outcome = scan_fixture(&dir, false);
    assert_eq!(outcome.files, 1, "只有 a.rs 参与扫描");
    assert_eq!(outcome.skipped, 1, "被排除的 *_tests.rs 计入 skipped");
}

// ── 渲染：排序 / focus / 预算 ───────────────────────────────────────────

#[test]
fn test_render_sorts_by_external_references() {
    let mut outcome = outcome_of(&[
        ("a.rs", 1, SymbolKind::Function, "hot"),
        ("b.rs", 2, SymbolKind::Function, "cold"),
    ]);
    // hot 被引用 5 次（含定义 → 输出 4），cold 只定义（输出 0）
    outcome.references.insert("hot".to_string(), 6);

    let (map, entries, truncated) = render(&outcome, &RenderOptions::default());
    assert_eq!(entries, 2);
    assert!(!truncated);
    let lines: Vec<&str> = map.lines().collect();
    assert!(lines[0].contains("hot"), "高引用应排最前：{map}");
    assert!(lines[0].contains("(5)"), "外部引用数应为 6-1=5：{map}");
    assert!(lines[1].contains("cold"));
    assert!(
        !lines[1].contains('('),
        "零引用不显示括号（省 token）：{map}"
    );
}

#[test]
fn test_render_focus_prioritizes_matching_symbols() {
    let mut outcome = outcome_of(&[
        ("a.rs", 1, SymbolKind::Function, "unrelated_hot"),
        ("b.rs", 2, SymbolKind::Function, "session_restore"),
    ]);
    outcome.references.insert("unrelated_hot".to_string(), 50);

    let (map, _, _) = render(
        &outcome,
        &RenderOptions {
            focus: Some("session".to_string()),
            ..RenderOptions::default()
        },
    );
    let lines: Vec<&str> = map.lines().collect();
    assert!(
        lines[0].contains("session_restore"),
        "focus 命中应压过引用度：{map}"
    );
}

#[test]
fn test_render_truncates_at_budget_and_marks() {
    let entries: Vec<(&str, usize, SymbolKind, &str)> = (0..200)
        .map(|i| ("very/long/path/file.rs", i + 1, SymbolKind::Function, "sym"))
        .collect();
    let outcome = outcome_of(&entries);

    let (map, rendered, truncated) = render(
        &outcome,
        &RenderOptions {
            focus: None,
            max_bytes: 512,
        },
    );
    assert!(truncated, "预算内放不下应标记截断");
    assert!(rendered < 200, "已渲染条目应少于总数：{rendered}");
    assert!(map.contains("地图已截断"), "应含截断标记：{map}");
    assert!(map.len() < 1024, "标记后总量仍应接近预算：{}", map.len());
}

#[test]
fn test_render_empty_outcome_is_empty_map() {
    let outcome = ScanOutcome {
        files: 0,
        skipped: 0,
        definitions: Vec::new(),
        references: HashMap::new(),
    };
    let (map, entries, truncated) = render(&outcome, &RenderOptions::default());
    assert_eq!(entries, 0);
    assert!(map.is_empty());
    assert!(!truncated);
}

// ── 缓存 ───────────────────────────────────────────────────────────────

#[test]
fn test_cache_hits_until_files_change() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "src/a.rs", "pub fn first_fn() {}\n");
    let cache = RepoMapCache::new();
    let options = ScanOptions {
        root: dir.path().to_path_buf(),
        include_tests: false,
    };

    let (first, cached) = cache.get_or_scan(&options);
    assert!(!cached, "首次应实际扫描");
    assert_eq!(first.files, 1);

    let (second, cached) = cache.get_or_scan(&options);
    assert!(cached, "指纹未变应命中缓存");
    assert_eq!(second.files, 1);

    // 改文件（内容长度变化 → 指纹必变，不依赖 mtime 精度）
    write(&dir, "src/a.rs", "pub fn second_fn_longer_name() {}\n");
    let (third, cached) = cache.get_or_scan(&options);
    assert!(!cached, "文件变化后应重扫");
    assert!(
        third
            .definitions
            .iter()
            .any(|d| d.name == "second_fn_longer_name"),
        "重扫结果应包含新符号：{:?}",
        third.definitions
    );

    // 新增文件同样失效
    write(&dir, "src/b.rs", "pub fn added() {}\n");
    let (fourth, cached) = cache.get_or_scan(&options);
    assert!(!cached, "新增文件后应重扫");
    assert_eq!(fourth.files, 2);
}

#[test]
fn test_cache_invalidated_by_include_tests_toggle() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "src/a.rs", "pub fn prod() {}\n");
    write(&dir, "src/a_tests.rs", "fn t() {}\n");
    let cache = RepoMapCache::new();

    let (excluded, _) = cache.get_or_scan(&ScanOptions {
        root: dir.path().to_path_buf(),
        include_tests: false,
    });
    let (included, cached) = cache.get_or_scan(&ScanOptions {
        root: dir.path().to_path_buf(),
        include_tests: true,
    });
    assert!(!cached, "include_tests 切换应使缓存失效");
    assert!(included.files > excluded.files);
}

// ── 参数辅助 ───────────────────────────────────────────────────────────

#[test]
fn test_resolve_budget_clamps() {
    assert_eq!(resolve_budget(None), DEFAULT_MAX_BYTES);
    assert_eq!(resolve_budget(Some(500)), 2000);
    // 上界：钳到硬上限
    assert_eq!(resolve_budget(Some(100_000)), HARD_MAX_BYTES);
    // 下界：不至于小到无意义
    assert_eq!(resolve_budget(Some(0)), 256);
}

#[test]
fn test_ensure_root_rejects_missing_path() {
    let err = ensure_root(Path::new("Z:\\definitely-not-here-xyz")).unwrap_err();
    assert!(err.is_not_found(), "{err}");
}
