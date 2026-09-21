//! repo_map 阈值校准探针（评估工具，不参与产品逻辑）。
//!
//! 用法：`cargo run -p tianyan-core --example repo_map_probe -- <仓库根> [--include-tests]`
//!
//! 目的：把 `repo_map` 的排序阈值放到**真实仓库**上量化，回答三个问题：
//!   1. 当前阈值砍掉了什么（误伤清单：高使用次数却被过滤的名字）；
//!   2. 阈值变动如何改变地图内容（重名度阈值 / 通用方法名过滤的敏感度）；
//!   3. 默认预算（4096 字节 ≈ 1000 token）能装下多少构件、长尾在哪。
//!
//! 本文件是 `core/src/executor/repo_map.rs` 私有过滤逻辑的**评估副本**
//! （`is_generic_method_name` / `kind_weight` / 排序）——探针需要注入不同阈值，
//! 而产品代码的阈值是常量。**改动产品阈值时同步此处**，否则评估失真。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use tianyan::executor::repo_map::{
    render, scan, Definition, RenderOptions, ScanOptions, ScanOutcome, MAX_SHARED_DEFINITIONS,
};
use tianyan::executor::symbols::SymbolKind;

/// 与 `repo_map.rs` 的 `GENERIC_NAME_MAX_LEN` 同步。
const GENERIC_NAME_MAX_LEN: usize = 6;

/// 与 `repo_map.rs` 的 `is_generic_method_name` 同步。
fn is_generic_method_name(kind: SymbolKind, name: &str) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
        && name.len() <= GENERIC_NAME_MAX_LEN
        && !name.contains('_')
        && name.chars().all(|c| c.is_ascii_lowercase())
}

/// 与 `repo_map.rs` 的 `kind_weight` 同步。
fn kind_weight(kind: SymbolKind) -> usize {
    match kind {
        SymbolKind::Module
        | SymbolKind::Struct
        | SymbolKind::Class
        | SymbolKind::Trait
        | SymbolKind::Interface
        | SymbolKind::Enum => 2,
        SymbolKind::Function | SymbolKind::Method | SymbolKind::Impl => 1,
    }
}

fn def_counts_of(outcome: &ScanOutcome) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for definition in &outcome.definitions {
        *counts.entry(definition.name.clone()).or_insert(0) += 1;
    }
    counts
}

fn uses_of(
    outcome: &ScanOutcome,
    counts: &HashMap<String, usize>,
    definition: &Definition,
) -> usize {
    let def_count = counts.get(&definition.name).copied().unwrap_or(1);
    outcome
        .references
        .get(&definition.name)
        .copied()
        .unwrap_or(0)
        .saturating_sub(def_count)
}

/// 复刻 `render` 的过滤 + 排序，但阈值可注入。
fn ranked<'a>(
    outcome: &'a ScanOutcome,
    counts: &HashMap<String, usize>,
    def_threshold: usize,
    apply_generic_filter: bool,
) -> Vec<(usize, usize, &'a Definition)> {
    let mut scored: Vec<(usize, usize, &Definition)> = outcome
        .definitions
        .iter()
        .filter(|definition| !matches!(definition.kind, SymbolKind::Method))
        .filter(|definition| counts.get(&definition.name).copied().unwrap_or(0) < def_threshold)
        .filter(|definition| {
            !apply_generic_filter || !is_generic_method_name(definition.kind, &definition.name)
        })
        .map(|definition| {
            let uses = uses_of(outcome, counts, definition);
            let score = kind_weight(definition.kind) * uses;
            (score, uses, definition)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.path.cmp(&b.2.path))
            .then_with(|| a.2.line.cmp(&b.2.line))
    });
    scored
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = PathBuf::from(args.first().cloned().unwrap_or_else(|| ".".to_string()));
    let include_tests = args.iter().any(|a| a == "--include-tests");
    println!("root = {}  include_tests = {include_tests}", root.display());

    let outcome = scan(&ScanOptions {
        root,
        include_tests,
    });
    let counts = def_counts_of(&outcome);

    // ── A. 基础统计 ──────────────────────────────────────────────────────
    println!(
        "\n== A. 基础统计 ==\nfiles={} skipped={} definitions={} distinct_names={} reference_keys={}",
        outcome.files,
        outcome.skipped,
        outcome.definitions.len(),
        counts.len(),
        outcome.references.len()
    );
    let mut kind_hist: HashMap<String, usize> = HashMap::new();
    for definition in &outcome.definitions {
        *kind_hist
            .entry(format!("{:?}", definition.kind))
            .or_insert(0) += 1;
    }
    let mut kind_hist: Vec<_> = kind_hist.into_iter().collect();
    kind_hist.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    println!(
        "kind 分布: {}",
        kind_hist
            .iter()
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // ── B. 重名度（def_count）分布 ───────────────────────────────────────
    let mut def_count_hist: HashMap<usize, usize> = HashMap::new();
    for count in counts.values() {
        *def_count_hist.entry(*count).or_insert(0) += 1;
    }
    let mut def_count_hist: Vec<_> = def_count_hist.into_iter().collect();
    def_count_hist.sort_by_key(|(count, _)| *count);
    println!("\n== B. 名字的定义次数分布（前 20 档）==");
    for (count, names) in def_count_hist.iter().take(20) {
        println!("  def_count={count:>3}  名字数={names}");
    }
    let over: usize = def_count_hist
        .iter()
        .filter(|(count, _)| *count >= MAX_SHARED_DEFINITIONS)
        .map(|(_, names)| names)
        .sum();
    println!(
        "  ≥{MAX_SHARED_DEFINITIONS} 次的名字 = {over} 个（占 distinct 的 {:.1}%）",
        over as f64 * 100.0 / counts.len().max(1) as f64
    );

    // ── C. 阈值敏感度：重名度阈值 ────────────────────────────────────────
    println!("\n== C. 阈值敏感度（固定通用名过滤开启）==");
    for threshold in [2usize, 3, 4, 5, 6, 8, 10, 12, 20, 1000] {
        let entries = ranked(&outcome, &counts, threshold, true);
        let head: Vec<String> = entries
            .iter()
            .take(10)
            .map(|(_, uses, definition)| {
                format!("{}({uses},{:?})", definition.name, definition.kind)
            })
            .collect();
        println!(
            "  阈值={threshold:>4} 保留条目={:>5}  前 10: {}",
            entries.len(),
            head.join(" ")
        );
    }

    // ── D. 被重名度过滤砍掉的「高使用」名字（误伤候选）─────────────────
    println!(
        "\n== D. 被重名度过滤（def_count ≥ {MAX_SHARED_DEFINITIONS}）的误伤候选（按 uses 降序）=="
    );
    let mut killed: Vec<(usize, usize, SymbolKind, String)> = outcome
        .definitions
        .iter()
        .filter(|definition| !matches!(definition.kind, SymbolKind::Method))
        .filter(|definition| counts[&definition.name] >= MAX_SHARED_DEFINITIONS)
        .map(|definition| {
            (
                uses_of(&outcome, &counts, definition),
                counts[&definition.name],
                definition.kind,
                definition.name.clone(),
            )
        })
        .collect();
    killed.sort_by_key(|(uses, _, _, _)| std::cmp::Reverse(*uses));
    let mut seen: HashMap<String, ()> = HashMap::new();
    for (uses, def_count, kind, name) in killed.iter().take(400) {
        if seen.insert(name.clone(), ()).is_some() {
            continue;
        }
        println!("  {name:<40} uses={uses:>5} def_count={def_count:>3} kind={kind:?}");
        if seen.len() >= 25 {
            break;
        }
    }

    // ── E. 被通用名过滤砍掉的 Function（误伤候选）───────────────────────
    println!("\n== E. 被通用名过滤（短/全小写/无下划线/≤{GENERIC_NAME_MAX_LEN}）砍掉的顶层函数 ==");
    let mut killed_fn: Vec<(usize, usize, String, String)> = outcome
        .definitions
        .iter()
        .filter(|definition| definition.kind == SymbolKind::Function)
        .filter(|definition| is_generic_method_name(definition.kind, &definition.name))
        .map(|definition| {
            (
                uses_of(&outcome, &counts, definition),
                counts[&definition.name],
                definition.name.clone(),
                definition.path.clone(),
            )
        })
        .collect();
    killed_fn.sort_by_key(|(uses, _, _, _)| std::cmp::Reverse(*uses));
    let mut seen_fn: HashMap<String, ()> = HashMap::new();
    for (uses, def_count, name, path) in killed_fn.iter() {
        if seen_fn.insert(name.clone(), ()).is_some() {
            continue;
        }
        println!("  {name:<20} uses={uses:>5} def_count={def_count:>3}  {path}");
        if seen_fn.len() >= 25 {
            break;
        }
    }
    println!("  合计砍掉 Function 条目 = {}", killed_fn.len());

    // ── F. 预算覆盖度 ────────────────────────────────────────────────────
    println!("\n== F. 预算覆盖度（默认阈值 + 默认过滤）==");
    for bytes in [1024usize, 2048, 4096, 8192, 16384] {
        let (text, entries, truncated) = render(
            &outcome,
            &RenderOptions {
                focus: None,
                max_bytes: bytes,
            },
        );
        println!(
            "  max_bytes={bytes:>6}  条目={entries:>4} 截断={truncated}  文本字节={}",
            text.len()
        );
    }

    // ── G. 不过滤 top 40（看原始噪声有多强）────────────────────────────
    println!("\n== G. 若不过滤（仅排除 Method），按 uses×kind_weight 的 top 40 ==");
    for (score, uses, definition) in ranked(&outcome, &counts, usize::MAX, false).iter().take(40) {
        println!(
            "  score={score:>6} uses={uses:>5} {:?} {}  ({})",
            definition.kind, definition.name, definition.path
        );
    }

    // ── H. 默认地图全文 ──────────────────────────────────────────────────
    let (map, entries, truncated) = render(&outcome, &RenderOptions::default());
    println!("\n== H. 默认地图（max_bytes=默认，entries={entries}，truncated={truncated}）==");
    print!("{map}");
}
