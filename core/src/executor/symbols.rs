//! 符号大纲提取引擎（基于 tree-sitter）。
//!
//! 从源码文本提取结构化符号（函数、结构体、类、方法、接口、枚举、impl、
//! trait、模块），供 `symbol_outline` 工具使用。递归遍历整棵语法树，
//! 因此嵌套声明（如 mod 内的 struct）也会被提取。
//!
//! ## 语言与节点类型映射（经各 grammar 的 node-types.json 验证）
//!
//! | 语言 | 节点类型 → 符号类型 |
//! |------|---------------------|
//! | rust | `function_item`→Function（**位于 `impl` 块内→Method**）、`struct_item`→Struct、`enum_item`→Enum、`impl_item`→Impl、`trait_item`→Trait、`mod_item`→Module |
//! | typescript / typescript-tsx | `function_declaration`→Function、`class_declaration`→Class、`interface_declaration`→Interface、`method_definition`→Method、`enum_declaration`→Enum |
//! | javascript | `function_declaration`→Function、`class_declaration`→Class、`method_definition`→Method |
//! | python | `class_definition`→Class、`function_definition`→Function（父节点为 `class_definition` 时→Method） |
//! | go | `function_declaration`→Function、`type_declaration`→Struct（仅当 `type_spec` 的底层类型为 `struct_type`；`type N int` 等非结构体声明跳过） |
//!
//! 名称取节点的 `name` 字段；缺失时回退（rust `impl_item` 无 `name` 字段 → `"impl"`，
//! 其余 → `"<anonymous>"`）。
//!
//! ## 错误处理
//!
//! 解析完全失败（`parse` 返回 `None`）→ 错误；语法错误（`root.has_error()`）→
//! 仍返回已提取的符号并置 `errors: true`，不失败。符号超过 [`MAX_SYMBOLS`]
//! 时截断并置 `truncated: true`。

use std::collections::HashMap;

use serde::Serialize;

use crate::common::error::TianyanError;

/// 符号类型（序列化为变体名字符串，如 `"Function"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SymbolKind {
    /// 函数（rust `function_item`、ts/js `function_declaration`、py `function_definition`、go `function_declaration`）。
    Function,
    /// 方法（ts/js `method_definition`；py `function_definition` 且父节点为
    /// `class_definition`；rust `function_item` 且祖先链含 `impl_item`）。
    Method,
    /// 结构体（rust `struct_item`；go `struct_type` 的 `type_spec`）。
    Struct,
    /// 类（ts/js `class_declaration`、py `class_definition`）。
    Class,
    /// impl 块（rust `impl_item`，无 `name` 字段 → 名称固定为 `"impl"`）。
    Impl,
    /// trait（rust `trait_item`）。
    Trait,
    /// 接口（ts `interface_declaration`）。
    Interface,
    /// 枚举（rust `enum_item`、ts `enum_declaration`）。
    Enum,
    /// 模块（rust `mod_item`）。
    Module,
}

/// 符号：名称 + 类型 + 1 起始行号 + 字节区间。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Symbol {
    /// 符号名称（无法提取时用各类型的回退名，见模块文档）。
    pub name: String,
    /// 符号类型。
    pub kind: SymbolKind,
    /// 1 起始行号。
    pub line: usize,
    /// 起始字节偏移。
    pub start: usize,
    /// 结束字节偏移（不含）。
    pub end: usize,
}

/// 单次提取的符号数量上限；超出后截断并置 `truncated: true`。
pub const MAX_SYMBOLS: usize = 500;

/// 符号大纲提取结果。
///
/// 设计初稿为 `(String, Vec<Symbol>)` 元组；为承载 `errors`/`truncated`
/// 标志（输出 JSON 契约要求），扩展为结构体。
#[derive(Debug, Clone, Serialize)]
pub struct SymbolOutline {
    /// 实际使用的语言标识（如 `"rust"`）。
    pub language: String,
    /// 提取的符号（可能被 [`MAX_SYMBOLS`] 截断）。
    pub symbols: Vec<Symbol>,
    /// 是否因超过 [`MAX_SYMBOLS`] 被截断。
    pub truncated: bool,
    /// 源码是否存在语法错误（tree-sitter `root.has_error()`）。
    pub errors: bool,
}

/// 从文件扩展名推断语言标识。
///
/// - `rs` → `rust`；`ts` → `typescript`；`tsx` → `typescript-tsx`；
///   `js` → `javascript`；`py` → `python`；`go` → `go`
/// - 其余（或无）扩展名 → `executor: symbol_outline: 不支持的语言: {ext}`
pub fn language_from_extension(path: &str) -> Result<String, TianyanError> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let language = match ext {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescript-tsx",
        "js" => "javascript",
        "py" => "python",
        "go" => "go",
        other => {
            return Err(TianyanError::Custom(format!(
                "executor: symbol_outline: 不支持的语言: {other}"
            )));
        }
    };
    Ok(language.to_string())
}

/// 提取源码文件的符号大纲。
///
/// - `language` 为语言标识（见 [`language_from_extension`]）
/// - 解析完全失败 → 错误；语法错误 → 符号照常返回且 `errors: true`
/// - 符号数超过 [`MAX_SYMBOLS`] → 截断且 `truncated: true`
pub fn symbol_outline(source: &str, language: &str) -> Result<SymbolOutline, TianyanError> {
    let tree = parse_tree(source, language)?;
    let root = tree.root_node();
    let mut symbols = Vec::new();
    collect_symbols(root, source, language, &mut symbols);
    let truncated = symbols.len() > MAX_SYMBOLS;
    symbols.truncate(MAX_SYMBOLS);
    Ok(SymbolOutline {
        language: language.to_string(),
        symbols,
        truncated,
        errors: root.has_error(),
    })
}

/// 符号索引：一次解析同时产出**符号定义**与**标识符出现计数**。
///
/// 与 [`symbol_outline`] 共用同一解析与符号收集路径（单一解析器实现）；
/// 额外统计标识符（`kind` 以 `identifier` 结尾且为叶节点，跳过注释与字符串
/// 子树）——供 `repo_map` 的「引用度排序」使用。**注意这是文本级近似**：
/// 不做名称解析（宏展开、重导出、动态分发不可见），故只用于排序权重，
/// 不可当作精确引用关系。
#[derive(Debug, Clone)]
pub struct SymbolIndex {
    /// 符号定义（可能被 [`MAX_SYMBOLS`] 截断）。
    pub symbols: Vec<Symbol>,
    /// 标识符 → 出现次数（跨文件聚合由调用方完成）。
    pub identifiers: HashMap<String, usize>,
    /// 是否因超过 [`MAX_SYMBOLS`] 截断符号定义。
    pub truncated: bool,
    /// 源码是否存在语法错误。
    pub errors: bool,
}

/// 提取符号索引（定义 + 标识符计数），见 [`SymbolIndex`]。
pub fn symbol_index(source: &str, language: &str) -> Result<SymbolIndex, TianyanError> {
    let tree = parse_tree(source, language)?;
    let root = tree.root_node();
    let mut symbols = Vec::new();
    collect_symbols(root, source, language, &mut symbols);
    let truncated = symbols.len() > MAX_SYMBOLS;
    symbols.truncate(MAX_SYMBOLS);
    let mut identifiers = HashMap::new();
    collect_identifiers(root, source, &mut identifiers);
    Ok(SymbolIndex {
        symbols,
        identifiers,
        truncated,
        errors: root.has_error(),
    })
}

/// 按语言标识取得 tree-sitter grammar（语言分派单点）。
fn grammar_for(language: &str) -> Result<tree_sitter::Language, TianyanError> {
    let grammar: tree_sitter::Language = match language {
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "typescript-tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        other => {
            return Err(TianyanError::Custom(format!(
                "executor: symbol_outline: 不支持的语言: {other}"
            )));
        }
    };
    Ok(grammar)
}

/// 解析源码为语法树（解析器初始化单点）。
fn parse_tree(source: &str, language: &str) -> Result<tree_sitter::Tree, TianyanError> {
    let grammar = grammar_for(language)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar).map_err(|e| {
        TianyanError::Custom(format!("executor: symbol_outline: 解析器初始化失败: {e}"))
    })?;
    parser
        .parse(source, None)
        .ok_or_else(|| TianyanError::Custom("executor: symbol_outline: 解析失败".to_string()))
}

/// 递归统计标识符出现次数。
///
/// 跳过 `comment` / `string` 子树（注释与字符串里的同名内容不是引用，
/// 计进去会污染排序）；只取**叶**标识符节点（`kind` 以 `identifier` 结尾
/// 且无子节点）——`scoped_identifier`（如 Rust 的 `a::b`）含子节点，
/// 整体文本不是单个标识符，交由子节点分别计数。
fn collect_identifiers(node: tree_sitter::Node, source: &str, out: &mut HashMap<String, usize>) {
    let kind = node.kind();
    if kind.contains("comment") || kind.contains("string") {
        return;
    }
    if kind.ends_with("identifier") {
        if node.child_count() == 0 {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                if !text.is_empty() {
                    *out.entry(text.to_string()).or_insert(0) += 1;
                }
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, out);
    }
}

/// 递归遍历所有节点，收集匹配的符号（含嵌套，如 mod 内的 struct）。
fn collect_symbols(node: tree_sitter::Node, source: &str, language: &str, out: &mut Vec<Symbol>) {
    if let Some(symbol) = classify(node, source, language) {
        out.push(symbol);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols(child, source, language, out);
    }
}

/// 按语言将节点类型映射为符号；不匹配的节点返回 `None`。
fn classify(node: tree_sitter::Node, source: &str, language: &str) -> Option<Symbol> {
    let (kind, name) = match (language, node.kind()) {
        // impl 块内的函数是**方法**（与 Python 的 class_definition 判定同型）：
        // `repo_map` 据此把方法排除出架构地图（构件 ≠ 方法）。
        ("rust", "function_item") => (
            if has_ancestor_kind(node, "impl_item") {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            symbol_name(node, source, "<anonymous>"),
        ),
        ("rust", "struct_item") => (SymbolKind::Struct, symbol_name(node, source, "<anonymous>")),
        ("rust", "enum_item") => (SymbolKind::Enum, symbol_name(node, source, "<anonymous>")),
        // impl_item 无 name 字段（fields: body/trait/type/type_parameters）→ 回退 "impl"
        ("rust", "impl_item") => (SymbolKind::Impl, symbol_name(node, source, "impl")),
        ("rust", "trait_item") => (SymbolKind::Trait, symbol_name(node, source, "<anonymous>")),
        ("rust", "mod_item") => (SymbolKind::Module, symbol_name(node, source, "<anonymous>")),
        ("typescript" | "typescript-tsx" | "javascript", "function_declaration") => (
            SymbolKind::Function,
            symbol_name(node, source, "<anonymous>"),
        ),
        ("typescript" | "typescript-tsx" | "javascript", "class_declaration") => {
            (SymbolKind::Class, symbol_name(node, source, "<anonymous>"))
        }
        ("typescript" | "typescript-tsx" | "javascript", "method_definition") => {
            (SymbolKind::Method, symbol_name(node, source, "<anonymous>"))
        }
        ("typescript" | "typescript-tsx", "interface_declaration") => (
            SymbolKind::Interface,
            symbol_name(node, source, "<anonymous>"),
        ),
        ("typescript" | "typescript-tsx", "enum_declaration") => {
            (SymbolKind::Enum, symbol_name(node, source, "<anonymous>"))
        }
        ("python", "class_definition") => {
            (SymbolKind::Class, symbol_name(node, source, "<anonymous>"))
        }
        ("python", "function_definition") => {
            // python 的 class body 是 `block` 节点（class_definition → block →
            // function_definition），因此必须沿祖先链判断是否在类内。
            let kind = if has_ancestor_kind(node, "class_definition") {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            (kind, symbol_name(node, source, "<anonymous>"))
        }
        ("go", "function_declaration") => (
            SymbolKind::Function,
            symbol_name(node, source, "<anonymous>"),
        ),
        ("go", "type_declaration") => {
            // 仅当 type_spec 的底层类型为 struct_type 时映射为 Struct；
            // `type N int`、type_alias 等其余声明跳过。
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() != "type_spec" {
                    continue;
                }
                let is_struct = child
                    .child_by_field_name("type")
                    .is_some_and(|t| t.kind() == "struct_type");
                if is_struct {
                    return Some(Symbol {
                        name: symbol_name(child, source, "<anonymous>"),
                        kind: SymbolKind::Struct,
                        line: node.start_position().row + 1,
                        start: node.start_byte(),
                        end: node.end_byte(),
                    });
                }
            }
            return None;
        }
        _ => return None,
    };
    Some(Symbol {
        name,
        kind,
        line: node.start_position().row + 1,
        start: node.start_byte(),
        end: node.end_byte(),
    })
}

/// 节点是否有指定 kind 的祖先节点（沿 `parent` 链上溯）。
fn has_ancestor_kind(node: tree_sitter::Node, target: &str) -> bool {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if p.kind() == target {
            return true;
        }
        parent = p.parent();
    }
    false
}

/// 提取节点的 `name` 字段文本；缺失、非 UTF-8 或为空时回退到 `fallback`。
///
/// `utf8_text` 的失败理论上不可能发生（`source` 本身是 `&str`），
/// 但按设计以 `.ok()` 降级而非 expect。
fn symbol_name(node: tree_sitter::Node, source: &str, fallback: &str) -> String {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
#[path = "symbols_tests.rs"]
mod tests;
