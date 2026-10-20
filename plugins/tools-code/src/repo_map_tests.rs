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
    // 夹具用「含下划线」的具体名：短名会被通用方法名规则过滤（见该规则）
    let mut outcome = outcome_of(&[
        ("a.rs", 1, SymbolKind::Function, "hot_topic"),
        ("b.rs", 2, SymbolKind::Function, "cold_topic"),
    ]);
    // hot_topic 被引用 5 次（含定义 → 输出 5），cold_topic 只定义（输出 0）
    outcome.references.insert("hot_topic".to_string(), 6);

    let (map, entries, truncated) = render(&outcome, &RenderOptions::default());
    assert_eq!(entries, 2);
    assert!(!truncated);
    let lines: Vec<&str> = map.lines().collect();
    assert!(lines[0].contains("hot_topic"), "高引用应排最前：{map}");
    assert!(lines[0].contains("(5)"), "外部引用数应为 6-1=5：{map}");
    assert!(lines[1].contains("cold_topic"));
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
    // 名字必须互不相同：同名会被「通用名过滤」剔除（见 MAX_SHARED_DEFINITIONS）
    let mut definitions = Vec::new();
    let mut references = HashMap::new();
    for i in 0..200 {
        let name = format!("filler_symbol_{i}");
        definitions.push(Definition {
            path: "very/long/path/file.rs".to_string(),
            line: i + 1,
            kind: SymbolKind::Function,
            name: name.clone(),
        });
        references.insert(name, 1);
    }
    let outcome = ScanOutcome {
        files: 1,
        skipped: 0,
        definitions,
        references,
    };

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
fn test_render_filters_shared_names() {
    // 判别力：通用名（定义次数 ≥ MAX_SHARED_DEFINITIONS）被剔除。
    // 实测依据：本仓库 `name` 出现 1099 次 / `path` 1102 次，曾把地图前 20 行
    // 全部占满（全是互不相关的同名 getter/trait 方法）。
    let mut definitions = Vec::new();
    let mut references = HashMap::new();
    for i in 1..=MAX_SHARED_DEFINITIONS {
        definitions.push(Definition {
            path: format!("src/mod_{i}.rs"),
            line: 1,
            kind: SymbolKind::Function,
            name: "name".to_string(),
        });
    }
    references.insert("name".to_string(), 999);
    definitions.push(Definition {
        path: "src/core.rs".to_string(),
        line: 9,
        kind: SymbolKind::Struct,
        name: "SessionStore".to_string(),
    });
    references.insert("SessionStore".to_string(), 6);
    // 边界判别力（2026-09-21 阈值校准 6→8 的回归锚点）：定义次数恰好
    // MAX_SHARED_DEFINITIONS − 1 的**模块**必须保留——`config` / `uri` / `error`
    // 正是「7 处定义、被引用上千次」的核心模块，阈值 6 时曾被误剔。
    for i in 1..MAX_SHARED_DEFINITIONS {
        definitions.push(Definition {
            path: format!("src/feature_{i}.rs"),
            line: 1,
            kind: SymbolKind::Module,
            name: "config".to_string(),
        });
    }
    references.insert("config".to_string(), 200);
    let outcome = ScanOutcome {
        files: 2,
        skipped: 0,
        definitions,
        references,
    };

    let (map, entries, _) = render(&outcome, &RenderOptions::default());
    assert!(
        !map.contains("Function name"),
        "定义达到阈值的通用名应被过滤：{map}"
    );
    assert_eq!(
        entries,
        1 + (MAX_SHARED_DEFINITIONS - 1),
        "应剩具体名 + 阈值内的同名模块（每个定义各占一行）：{map}"
    );
    assert!(
        map.contains("SessionStore(5)"),
        "使用次数 = 6 次出现 − 1 次定义 = 5：{map}"
    );
    assert!(
        map.contains("Module config(193)"),
        "定义次数 < 阈值的同名模块应保留（使用次数 = 200 − 7）：{map}"
    );
}

#[test]
fn test_render_prefers_types_over_functions() {
    // 同使用次数下，类型/模块（架构构件）优先于函数——「摸清架构」关心构件。
    let mut outcome = outcome_of(&[
        ("a.rs", 1, SymbolKind::Function, "fn_symbol"),
        ("b.rs", 2, SymbolKind::Struct, "struct_symbol"),
    ]);
    outcome.references.insert("fn_symbol".to_string(), 6);
    outcome.references.insert("struct_symbol".to_string(), 6);

    let (map, _, _) = render(&outcome, &RenderOptions::default());
    let lines: Vec<&str> = map.lines().collect();
    assert!(lines[0].contains("struct_symbol"), "类型应优先：{map}");
}
#[test]
fn test_render_excludes_methods() {
    // 判别力：impl 内方法（SymbolKind::Method）不进地图——地图回答「仓库由哪些
    // 构件组成」，构件是模块/类型/trait，不是 getter。实测：session_id(963) /
    // len(664) / text(584) / entry(540) / is_empty(474) 全是 impl 内方法，
    // 却因调用量大占据前排。
    let mut outcome = outcome_of(&[
        ("a.rs", 3, SymbolKind::Method, "session_id"),
        ("b.rs", 7, SymbolKind::Struct, "SessionStore"),
    ]);
    outcome.references.insert("session_id".to_string(), 964);
    outcome.references.insert("SessionStore".to_string(), 6);

    let (map, entries, _) = render(&outcome, &RenderOptions::default());
    assert!(!map.contains("session_id"), "方法不应进地图：{map}");
    assert_eq!(entries, 1, "只应剩构件：{map}");
    assert!(map.contains("SessionStore"), "{map}");
}

#[test]
fn test_render_filters_generic_method_names() {
    // 判别力（第二级过滤）：`path` 类高频方法名——短、全小写、无下划线、多处定义
    // （引用多来自标准库/依赖的方法调用）——被过滤；短名但属模块/类型的保留，
    // 长名或含下划线的函数同样保留。
    let mut definitions = Vec::new();
    let mut references = HashMap::new();
    for i in 1..=2 {
        definitions.push(Definition {
            path: format!("src/a{i}.rs"),
            line: 1,
            kind: SymbolKind::Function,
            name: "path".to_string(),
        });
    }
    references.insert("path".to_string(), 1100);
    // 唯一但被标准库同名 API 刷分的短名顶层函数（`HashMap::entry`）
    definitions.push(Definition {
        path: "src/d.rs".to_string(),
        line: 11,
        kind: SymbolKind::Function,
        name: "entry".to_string(),
    });
    references.insert("entry".to_string(), 540);
    definitions.push(Definition {
        path: "src/b.rs".to_string(),
        line: 3,
        kind: SymbolKind::Module,
        name: "vfs".to_string(),
    });
    references.insert("vfs".to_string(), 700);
    definitions.push(Definition {
        path: "src/c.rs".to_string(),
        line: 5,
        kind: SymbolKind::Function,
        name: "session_manager".to_string(),
    });
    references.insert("session_manager".to_string(), 150);
    let outcome = ScanOutcome {
        files: 3,
        skipped: 0,
        definitions,
        references,
    };

    let (map, _, _) = render(&outcome, &RenderOptions::default());
    assert!(
        !map.contains("Function path"),
        "高频通用方法名应被过滤：{map}"
    );
    assert!(
        !map.contains("Function entry"),
        "短名顶层函数（引用来自标准库同名 API）也应被过滤：{map}"
    );
    assert!(
        map.contains("Module vfs"),
        "短名但属模块：应保留（不受此规则影响）：{map}"
    );
    assert!(
        map.contains("session_manager"),
        "含下划线的长名：应保留：{map}"
    );
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
