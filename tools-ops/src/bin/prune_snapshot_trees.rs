//! 存量快照树重写（ADR-055 的**存量**落地）：按排除集过滤树内路径并写回。
//!
//! # 为什么需要
//! ADR-055 让**新**捕获的快照排除 `.gitignore` 命中的路径，但**存量树仍引用着**
//! 那些对象——GC 不敢回收，磁盘不降。实测（2026-11-12 `D:\tianyan`）：
//!
//! - 树文件 2,809 个 / 2,042 MB / **1,460 万条目**；
//! - 其中 43.9% 属于构建产物与缓存（Playwright 截图、`dist`、`target`、`*.log` …）；
//! - 仅被这类路径引用的对象 **66,041 个 / 783 MB**（对象库共 4,509 MB）。
//!
//! 本脚本把存量树按「通用排除集」重写（树本身瘦 40% 量级），之后由快照 GC
//! （`snapshot_gc` 定时任务，12 小时）回收孤儿对象。
//!
//! # 语义变化（已拍板）
//! 被排除路径**不再参与回退**：回退既不恢复也不删除它们。消息链回退不受影响
//! （链在 SQLite）。这是「用历史文件回退能力的一部分换取磁盘与遍历成本」的
//! 明确取舍，不是副作用。
//!
//! # 为什么用「通用特征集」而不是 `.gitignore`
//! 捕获侧按**会话工作目录**调 `git ls-files --ignored` 精确排除；旧树没有记录
//! 工作目录（v1 格式），因此这里只能用与项目无关的通用特征集（产物/缓存目录名 +
//! 文件后缀）。覆盖不了项目特有的忽略项——**刻意的保守取舍**：宁可少删（留下
//! 一些无害条目），不可误删用户内容。
//!
//! # 用法
//! ```text
//! cargo run -p tianyan-tools-ops --bin prune_snapshot_trees -- --dry-run   # 只读预览
//! cargo run -p tianyan-tools-ops --bin prune_snapshot_trees                # 执行重写
//! cargo run -p tianyan-tools-ops --bin prune_snapshot_trees -- --gc        # 重写 + 回收（须停服务）
//! ```
//!
//! 其它参数：`--data-dir <path>`（隔离环境）、`--aggressive`（额外剔除
//! `build` / `out` 目录与 `*.map`）、`--limit <n>`（只处理前 n 棵树，试跑）。
//!
//! **重写不需要停服务**：树文件是原子写，与运行中的捕获互不损坏（最坏情况是某棵
//! 「最新树」被本脚本的通用规则版覆盖，下一次捕获会写回精确版）。**`--gc` 必须
//! 停服务**：标记-清扫与运行中的捕获有竞态（对象已写、树未落盘时会被判成孤儿），
//! 脚本探测到 `127.0.0.1:3000` 有监听即拒绝执行。
//!
//! 重写后脚本会自动清掉**陈旧的 mtime/size 缓存**（`latest.cache.json` 与
//! `redo/tree-*.cache.json`）：缓存里的 hash 可能指向本次剔除、随后被 GC 回收的
//! 对象，复用会让新树引用不存在的对象（实测一次清理后有 44,233 条这样的记录）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tianyan::common::error::{Result, TianyanError};
use tianyan::snapshot::SnapshotManager;

/// 通用排除目录名（与项目无关的构建产物 / 缓存 / 依赖目录，大小写不敏感）。
const PRUNED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "dist-old",
    "coverage",
    "test-results",
    "playwright-report",
    ".playwright-mcp",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    "__pycache__",
    ".pytest_cache",
    ".venv",
    "venv",
    // 工具 / IDE 运行数据（不是用户内容；`.vscode` 见 `--aggressive`——它常被团队提交）
    ".idea",
    ".omo",
    ".trae",
];

/// 「父目录名 + 子目录名」成对命中的复合路径。
///
/// 整棵排除会误删源码：`docs/.vitepress/cache` 是构建缓存，而
/// `docs/.vitepress/config.ts` 是仓库里的源码。
const PRUNED_PAIRS: &[(&str, &str)] = &[(".vitepress", "cache")];

/// 产物后缀（大小写不敏感）。
///
/// `.db` 系列刻意包含在内：二进制数据库文件进快照既无意义也危险（回退会把旧库
/// 写回去），且主流 `.gitignore` 都忽略它们。
const PRUNED_SUFFIXES: &[&str] = &[
    ".log",
    ".msi",
    ".wixpdb",
    ".tsbuildinfo",
    ".db",
    ".db-wal",
    ".db-shm",
    ".pyc",
];

/// `--aggressive` 追加的目录名（可能是被跟踪的源码/配置目录，默认不动）。
const AGGRESSIVE_DIRS: &[&str] = &["build", "out", ".vscode"];

/// `--aggressive` 追加的后缀（sourcemap 有时随发布物提交）。
const AGGRESSIVE_SUFFIXES: &[&str] = &[".map"];

/// 判定一条树内路径在「通用排除集」下是否应被剔除，返回**折叠后**的 `skipped` 条目。
///
/// 折叠规则：命中的是目录 → 记最浅命中目录的路径（`target`、`gui-vite/dist`），
/// 而不是逐个文件——否则 `skipped` 会膨胀到与 `files` 同量级（实测被排除条目
/// 640 万条）；命中的是文件后缀 → 记该文件路径本身。
fn prune_entry(rel: &str, aggressive: bool) -> Option<String> {
    let segs: Vec<&str> = rel.split('/').collect();
    let last = segs.len().checked_sub(1)?;

    // 1) 目录名命中（只看最后一段之前的目录段）
    for (i, seg) in segs.iter().enumerate().take(last) {
        if is_pruned_dir(seg, aggressive) {
            return Some(segs[..=i].join("/"));
        }
    }

    // 2) 复合路径成对命中
    for i in 0..last {
        for (parent, child) in PRUNED_PAIRS {
            if segs[i].eq_ignore_ascii_case(parent) && segs[i + 1].eq_ignore_ascii_case(child) {
                return Some(segs[..=i + 1].join("/"));
            }
        }
    }

    // 3) 后缀命中
    let name = segs[last].to_ascii_lowercase();
    let hit = PRUNED_SUFFIXES
        .iter()
        .chain(AGGRESSIVE_SUFFIXES.iter().filter(|_| aggressive))
        .any(|s| name.ends_with(s));
    hit.then(|| rel.to_string())
}

/// 目录名是否在排除集内。
fn is_pruned_dir(seg: &str, aggressive: bool) -> bool {
    PRUNED_DIRS.iter().any(|d| d.eq_ignore_ascii_case(seg))
        || (aggressive && AGGRESSIVE_DIRS.iter().any(|d| d.eq_ignore_ascii_case(seg)))
}

/// 单次运行统计。
#[derive(Default)]
struct Stats {
    trees_scanned: usize,
    trees_rewritten: usize,
    trees_unchanged: usize,
    trees_failed: usize,
    entries_before: usize,
    entries_after: usize,
    entries_pruned: usize,
    skipped_folded: usize,
    bytes_before: u64,
    bytes_after: u64,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("prune_snapshot_trees 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let aggressive = args.iter().any(|a| a == "--aggressive");
    let gc = args.iter().any(|a| a == "--gc");
    let data_dir_override: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let limit: Option<usize> = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());

    let config = tianyan_tools_ops::load_config(data_dir_override)?;
    let snap_root = config.storage.data_dir.join("snapshots");
    println!("数据目录：{}", config.storage.data_dir.display());
    println!("快照根：{}", snap_root.display());
    println!(
        "模式：{}  排除集：{}{}",
        if dry_run {
            "dry-run（只读预览）"
        } else {
            "执行"
        },
        if aggressive { "aggressive" } else { "默认" },
        match limit {
            Some(n) => format!("  限 {n} 棵树"),
            None => String::new(),
        }
    );
    if !snap_root.is_dir() {
        return Err(TianyanError::not_found(format!(
            "快照根不存在：{}",
            snap_root.display()
        )));
    }

    let mut trees = collect_tree_files(&snap_root);
    let total_found = trees.len();
    if let Some(n) = limit {
        trees.truncate(n);
    }
    println!(
        "树文件：{} 个（本次处理 {} 个）\n",
        total_found,
        trees.len()
    );

    let mgr = SnapshotManager::new(snap_root.clone(), snap_root.clone());
    let mut stats = Stats::default();
    let started = Instant::now();
    for (idx, path) in trees.iter().enumerate() {
        if let Err(e) = process_tree(&mgr, path, aggressive, dry_run, &mut stats).await {
            stats.trees_failed += 1;
            eprintln!("  [跳过] {}：{e}", path.display());
        }
        if (idx + 1) % 200 == 0 {
            println!(
                "  进度：{}/{}（已瘦身 {} 条）",
                idx + 1,
                trees.len(),
                stats.entries_pruned
            );
        }
    }
    let elapsed = started.elapsed();

    println!("\n=== 汇总 ===");
    println!("  扫描树文件：{}", stats.trees_scanned);
    println!("  重写：{}", stats.trees_rewritten);
    println!("  无需改动：{}", stats.trees_unchanged);
    println!("  失败/跳过：{}", stats.trees_failed);
    println!(
        "  条目：{} → {}（剔除 {}，占 {:.1}%）",
        stats.entries_before,
        stats.entries_after,
        stats.entries_pruned,
        percent(stats.entries_pruned, stats.entries_before)
    );
    println!("  新增 skipped 折叠条目：{}", stats.skipped_folded);
    if dry_run {
        println!(
            "  树文件体积：{:.1} MB（dry-run 不写盘；按剔除比例预计约 {:.1} MB）",
            stats.bytes_before as f64 / 1_048_576.0,
            stats.bytes_before as f64 / 1_048_576.0
                * (1.0 - percent(stats.entries_pruned, stats.entries_before) / 100.0)
        );
    } else {
        println!(
            "  树文件体积：{:.1} MB → {:.1} MB",
            stats.bytes_before as f64 / 1_048_576.0,
            stats.bytes_after as f64 / 1_048_576.0
        );
    }
    println!("  耗时：{:.1} s", elapsed.as_secs_f64());
    if dry_run {
        println!("\n>>> dry-run：未写入任何文件（去掉 --dry-run 即执行）");
    } else {
        // 重写之后**必须**清掉 mtime/size 缓存：缓存记录 `{路径 → 捕获时 hash}`，
        // 本次剔除的条目（以及随后被 GC 回收的对象）仍留在缓存里——下次捕获遇到
        // 「mtime/size 未变」会直接复用那个 hash，让新树引用不存在的对象（该锚点
        // 回退在预校验阶段中止）。缓存是纯优化：删掉只是下一次捕获全量重算一遍哈希。
        if stats.trees_rewritten > 0 {
            let (files, bytes) = clear_caches(&snap_root);
            println!(
                "  清理陈旧 mtime/size 缓存：{files} 个文件 / {:.1} MB（下次捕获全量重算一次）",
                bytes as f64 / 1_048_576.0
            );
        }
        println!(
            "\n下一步：孤儿对象由快照 GC 回收（`snapshot_gc` 定时任务，12 小时一次）；\
             想立刻回收就停服务后加 --gc 重跑。"
        );
    }

    if gc {
        run_gc(&snap_root).await?;
    }
    Ok(())
}

/// 处理单个树文件：过滤 → 写回（原子写；内容无变化则跳过）。
async fn process_tree(
    mgr: &SnapshotManager,
    path: &Path,
    aggressive: bool,
    dry_run: bool,
    stats: &mut Stats,
) -> Result<()> {
    let _ = mgr;
    let bytes_before = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut tree = SnapshotManager::read_tree_file(path).await?;
    stats.trees_scanned += 1;
    stats.bytes_before += bytes_before;
    stats.entries_before += tree.files.len();

    // 旧格式树（v1）没有 `skipped` 的落点：仍记录（内存里），写盘时丢弃——
    // 不为旧树编造归属信息（见 `SnapshotManager::write_tree_file` 文档）
    let can_carry_skipped = tree.workdir.is_some();

    let mut keep: HashMap<String, String> = HashMap::with_capacity(tree.files.len());
    let mut pruned = 0usize;
    let mut folded = 0usize;
    for (rel, hash) in std::mem::take(&mut tree.files) {
        match prune_entry(&rel, aggressive) {
            Some(fold) => {
                pruned += 1;
                if can_carry_skipped && tree.skipped.insert(fold) {
                    folded += 1;
                }
            }
            None => {
                keep.insert(rel, hash);
            }
        }
    }
    tree.files = keep;

    if pruned == 0 {
        stats.trees_unchanged += 1;
        stats.entries_after += tree.files.len();
        stats.bytes_after += bytes_before;
        return Ok(());
    }

    if !dry_run {
        SnapshotManager::write_tree_file(path, &tree).await?;
        stats.bytes_after += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    } else {
        stats.bytes_after += bytes_before;
    }
    stats.trees_rewritten += 1;
    stats.entries_pruned += pruned;
    stats.skipped_folded += folded;
    stats.entries_after += tree.files.len();
    Ok(())
}

/// 收集所有树文件（`*/trees/*.json` 与 `*/redo/tree-*.json`，排除缓存文件）。
fn collect_tree_files(snap_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(sessions) = std::fs::read_dir(snap_root) else {
        return out;
    };
    for session in sessions.flatten() {
        let dir = session.path();
        if !dir.is_dir() {
            continue;
        }
        for sub in ["trees", "redo"] {
            let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                // 缓存指针（`{index}.cache.json`、`tree-*.cache.json`）不是树
                if !name.ends_with(".json") || name.ends_with(".cache.json") {
                    continue;
                }
                // `redo/` 下还有 `messages-{key}.json`（被回退的消息本身，不是树）：
                // 只认 `tree-` 前缀，否则会把消息文件当树解析（必然失败并刷警告）
                if sub == "redo" && !name.starts_with("tree-") {
                    continue;
                }
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// 删除所有会话的 mtime/size 缓存（`latest.cache.json` 与 `redo/tree-*.cache.json`）。
///
/// 为什么必须清：缓存记录 `{路径 → 捕获时 hash}`，重写剔除的条目（以及随后被 GC
/// 回收的对象）仍留在缓存里——下次捕获遇到「mtime/size 未变」会直接复用那个 hash，
/// 让新树引用不存在的对象（该锚点回退在预校验阶段中止）。缓存是纯优化，删掉只是
/// 下一次捕获全量重算一遍哈希（各会话一次）。`SnapshotManager` 侧另有「对象存在才
/// 复用」的兜底检查，但清掉更彻底。
fn clear_caches(snap_root: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let Ok(sessions) = std::fs::read_dir(snap_root) else {
        return (files, bytes);
    };
    for session in sessions.flatten() {
        let dir = session.path();
        if !dir.is_dir() {
            continue;
        }
        let mut targets = vec![dir.join("latest.cache.json")];
        if let Ok(entries) = std::fs::read_dir(dir.join("redo")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("tree-") && name.ends_with(".cache.json") {
                    targets.push(entry.path());
                }
            }
        }
        for path in targets {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if std::fs::remove_file(&path).is_ok() {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

/// 跑一次快照 GC（孤儿对象 + 空会话目录）。要求服务已停止。
async fn run_gc(snap_root: &Path) -> Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 3000));
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(800)).is_ok() {
        return Err(TianyanError::invalid_input(
            "检测到 127.0.0.1:3000 有服务在监听：--gc 必须在**停止服务**后执行\
             （标记-清扫与运行中的捕获有竞态：对象已写、树未落盘时会被判成孤儿）。\
             也可以不跑 --gc，交给 12 小时一次的 snapshot_gc 定时任务回收。",
        ));
    }
    println!("\n=== 快照 GC ===");
    let mgr = SnapshotManager::new(snap_root.to_path_buf(), snap_root.to_path_buf());
    let stats = mgr.gc().await?;
    println!("  对象总数：{}", stats.total_objects);
    println!(
        "  删除孤儿对象：{}（保留 {}）",
        stats.objects_removed, stats.objects_retained
    );
    println!("  删除孤儿重做缓存：{}", stats.cache_files_removed);
    println!("  删除空会话目录：{}", stats.session_dirs_removed);
    Ok(())
}

/// 百分比（分母 0 时返回 0）。
fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dir_hit_collapses_to_shallowest() {
        assert_eq!(
            prune_entry("target/debug/build/x.bin", false).as_deref(),
            Some("target")
        );
        assert_eq!(
            prune_entry("gui-vite/dist/assets/a.js", false).as_deref(),
            Some("gui-vite/dist")
        );
        assert_eq!(
            prune_entry("node_modules/pkg/index.js", false).as_deref(),
            Some("node_modules")
        );
    }

    #[test]
    fn test_vitepress_cache_pair_not_whole_dir() {
        // 缓存目录整棵剔除
        assert_eq!(
            prune_entry("docs/.vitepress/cache/deps/x.json", false).as_deref(),
            Some("docs/.vitepress/cache")
        );
        // 同目录下的源码必须留下（整棵排除会误删）
        assert_eq!(prune_entry("docs/.vitepress/config.ts", false), None);
    }

    #[test]
    fn test_suffix_hit_keeps_file_path() {
        assert_eq!(
            prune_entry("gui-vite/lint.log", false).as_deref(),
            Some("gui-vite/lint.log")
        );
        assert_eq!(
            prune_entry("gui-native/installer/x-1.2.5-x64.msi", false).as_deref(),
            Some("gui-native/installer/x-1.2.5-x64.msi")
        );
    }

    #[test]
    fn test_source_paths_survive() {
        for rel in [
            "core/src/snapshot/mod.rs",
            "docs/architecture/decisions/055-snapshot-exclusion-and-deletion-guard.md",
            "gui-native/src/pages/tools.rs",
            "Cargo.toml",
            // 只是名字里带 target/dist，但不是目录段
            "src/distribution.rs",
        ] {
            assert_eq!(prune_entry(rel, false), None, "{rel} 不应被剔除");
        }
    }

    #[test]
    fn test_aggressive_only_extends() {
        // 默认不动 build / out / *.map
        assert_eq!(prune_entry("build/tool.ps1", false), None);
        assert_eq!(prune_entry("out/readme.txt", false), None);
        assert_eq!(prune_entry("web/app.js.map", false), None);
        // aggressive 才剔除
        assert_eq!(
            prune_entry("build/tool.ps1", true).as_deref(),
            Some("build")
        );
        assert_eq!(prune_entry("out/readme.txt", true).as_deref(), Some("out"));
        assert_eq!(
            prune_entry("web/app.js.map", true).as_deref(),
            Some("web/app.js.map")
        );
    }

    #[test]
    fn test_percent_guards_zero() {
        assert!((percent(0, 0) - 0.0).abs() < f64::EPSILON);
        assert!((percent(44, 100) - 44.0).abs() < f64::EPSILON);
    }
}
