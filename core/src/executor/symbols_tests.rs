//! 符号大纲提取引擎测试（真实 tree-sitter 语法解析，无外部依赖）。
//!
//! 每个语言一份内联夹具，断言精确的符号列表（名称 + 类型 + 行号），
//! 以及字节区间与源文本的对应关系（避免手数字节偏移的脆弱断言）。

use super::*;

/// 提取 `(名称, 类型)` 列表，便于顺序断言。
fn names_kinds(outline: &SymbolOutline) -> Vec<(String, SymbolKind)> {
    outline
        .symbols
        .iter()
        .map(|s| (s.name.clone(), s.kind))
        .collect()
}

// ── rust ───────────────────────────────────────────────────────────────

#[test]
fn test_rust_outline() {
    let source =
        "fn main() {}\nstruct Foo {}\nenum Bar { A }\nimpl Foo {}\ntrait Baz {}\nmod utils {}\n";
    let outline = symbol_outline(source, "rust").unwrap();
    assert_eq!(outline.language, "rust");
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("main".to_string(), SymbolKind::Function),
            ("Foo".to_string(), SymbolKind::Struct),
            ("Bar".to_string(), SymbolKind::Enum),
            ("impl".to_string(), SymbolKind::Impl),
            ("Baz".to_string(), SymbolKind::Trait),
            ("utils".to_string(), SymbolKind::Module),
        ]
    );
    for (i, sym) in outline.symbols.iter().enumerate() {
        assert_eq!(sym.line, i + 1, "符号应位于 1 起始行号");
        // 字节区间必须与源文本对应（节点覆盖完整声明）
        assert_eq!(&source[sym.start..sym.end], sym_text(i));
    }
    assert!(!outline.truncated);
    assert!(!outline.errors);
}

/// 与 test_rust_outline 夹具对应的声明文本。
fn sym_text(i: usize) -> &'static str {
    [
        "fn main() {}",
        "struct Foo {}",
        "enum Bar { A }",
        "impl Foo {}",
        "trait Baz {}",
        "mod utils {}",
    ][i]
}

/// 嵌套：mod 内的 struct / fn 都应被提取（递归遍历全部节点）。
#[test]
fn test_rust_nested_symbols() {
    let source =
        "mod outer {\n    struct Inner {\n        field: i32,\n    }\n    fn helper() {}\n}\n";
    let outline = symbol_outline(source, "rust").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("outer".to_string(), SymbolKind::Module),
            ("Inner".to_string(), SymbolKind::Struct),
            ("helper".to_string(), SymbolKind::Function),
        ]
    );
}

// ── typescript / tsx ───────────────────────────────────────────────────

#[test]
fn test_typescript_outline() {
    let source = "function add(a: number) {}\nclass C {}\ninterface I {}\nenum E {}\n";
    let outline = symbol_outline(source, "typescript").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("add".to_string(), SymbolKind::Function),
            ("C".to_string(), SymbolKind::Class),
            ("I".to_string(), SymbolKind::Interface),
            ("E".to_string(), SymbolKind::Enum),
        ]
    );
    assert_eq!(
        outline.symbols.iter().map(|s| s.line).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

#[test]
fn test_typescript_method_definition() {
    let source = "class C {\n  m(): void {}\n}\n";
    let outline = symbol_outline(source, "typescript").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("C".to_string(), SymbolKind::Class),
            ("m".to_string(), SymbolKind::Method),
        ]
    );
    assert_eq!(outline.symbols[0].line, 1);
    assert_eq!(outline.symbols[1].line, 2);
}

#[test]
fn test_tsx_outline() {
    let source = "function C(): JSX.Element {\n  return <div />;\n}\n";
    let outline = symbol_outline(source, "typescript-tsx").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![("C".to_string(), SymbolKind::Function)]
    );
}

// ── javascript ─────────────────────────────────────────────────────────

#[test]
fn test_javascript_outline() {
    let source = "function f() {}\nclass K {\n  g() {}\n}\n";
    let outline = symbol_outline(source, "javascript").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("f".to_string(), SymbolKind::Function),
            ("K".to_string(), SymbolKind::Class),
            ("g".to_string(), SymbolKind::Method),
        ]
    );
    assert_eq!(
        outline.symbols.iter().map(|s| s.line).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

// ── python ─────────────────────────────────────────────────────────────

#[test]
fn test_python_outline() {
    let source = "def f():\n    pass\nclass K:\n    def m(self):\n        pass\n";
    let outline = symbol_outline(source, "python").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("f".to_string(), SymbolKind::Function),
            ("K".to_string(), SymbolKind::Class),
            ("m".to_string(), SymbolKind::Method),
        ]
    );
    assert_eq!(
        outline.symbols.iter().map(|s| s.line).collect::<Vec<_>>(),
        vec![1, 3, 4]
    );
}

// ── go ─────────────────────────────────────────────────────────────────

#[test]
fn test_go_outline() {
    let source = "func main() {}\ntype T struct {}\n";
    let outline = symbol_outline(source, "go").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![
            ("main".to_string(), SymbolKind::Function),
            ("T".to_string(), SymbolKind::Struct),
        ]
    );
    assert_eq!(outline.symbols[0].line, 1);
    assert_eq!(outline.symbols[1].line, 2);
}

/// 非 struct 的 type 声明（如 `type N int`）不映射为 Struct，跳过。
#[test]
fn test_go_non_struct_type_declaration_skipped() {
    let source = "type N int\ntype S struct{}\n";
    let outline = symbol_outline(source, "go").unwrap();
    assert_eq!(
        names_kinds(&outline),
        vec![("S".to_string(), SymbolKind::Struct)]
    );
}

// ── 语言识别 ───────────────────────────────────────────────────────────

#[test]
fn test_language_from_extension() {
    assert_eq!(language_from_extension("a.rs").unwrap(), "rust");
    assert_eq!(language_from_extension("a.ts").unwrap(), "typescript");
    assert_eq!(language_from_extension("a.tsx").unwrap(), "typescript-tsx");
    assert_eq!(language_from_extension("a.js").unwrap(), "javascript");
    assert_eq!(language_from_extension("a.py").unwrap(), "python");
    assert_eq!(language_from_extension("a.go").unwrap(), "go");
}

#[test]
fn test_language_from_extension_unsupported() {
    let err = language_from_extension("a.txt").unwrap_err();
    assert!(err.to_string().contains("不支持的语言"), "实际: {err}");
    let err = language_from_extension("no_extension").unwrap_err();
    assert!(err.to_string().contains("不支持的语言"), "实际: {err}");
}

#[test]
fn test_symbol_outline_unsupported_language() {
    let err = symbol_outline("fn main() {}", "cobol").unwrap_err();
    assert!(err.to_string().contains("不支持的语言"), "实际: {err}");
}

// ── 边界情况 ───────────────────────────────────────────────────────────

/// 语法错误：仍返回已提取符号，且 errors 标志为 true（不 panic、不失败）。
#[test]
fn test_parse_errors_flag() {
    let outline = symbol_outline("fn broken(\n", "rust").unwrap();
    assert!(outline.errors);
}

#[test]
fn test_empty_source() {
    let outline = symbol_outline("", "rust").unwrap();
    assert!(outline.symbols.is_empty());
    assert!(!outline.truncated);
    assert!(!outline.errors);
}

/// 超过 MAX_SYMBOLS 时截断并置 truncated 标志。
#[test]
fn test_truncated_at_max_symbols() {
    let mut source = String::new();
    for i in 0..600 {
        source.push_str(&format!("fn f{i}() {{}}\n"));
    }
    let outline = symbol_outline(&source, "rust").unwrap();
    assert_eq!(outline.symbols.len(), MAX_SYMBOLS);
    assert!(outline.truncated);
    assert_eq!(outline.symbols[0].name, "f0");
    assert_eq!(
        outline.symbols[MAX_SYMBOLS - 1].name,
        format!("f{}", MAX_SYMBOLS - 1)
    );
}
