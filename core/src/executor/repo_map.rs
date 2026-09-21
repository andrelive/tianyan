//! 仓库结构地图（repo map）：跨文件符号骨架 + 引用度排序。
//!
//! 目的：**一次调用**给出「仓库里有什么、大概在哪」的压缩骨架，替代多轮 grep
//! 试探。按需调用（模型自行拉取），零外部依赖、零常驻进程——与 LSP 路线相反
//! （后者需装语言服务器且进程常驻，见 `docs/architecture/code-intelligence-routes.md`）。
//!
//! ## 与其它代码智能路线的关系
//!
//! - `symbol_outline`：单文件大纲。本模块复用其解析器（[`symbol_index`]），
//!   把「单文件」扩成「跨文件聚合 + 引用度排序」。
//! - `grep`：文本检索。本模块的引用计数建立在 tree-sitter **语法节点**上，
//!   注释与字符串子树不计入（文本匹配会把它们算成引用，污染排序）。
//! - **近似性声明**：引用度是**文本级近似**（不做名称解析；宏展开、重导出、
//!   动态分发不可见）。要精确影响面：改完跑 `verify_build`（编译器给精确清单）。
//!
//! ## 输出契约
//!
//! `{ root, files, symbols, truncated, cached, map }`——`map` 为骨架文本，
//! 每行 `相对路径:行 种类 名称(外部引用数)`（引用数为 0 时省略括号）。
//!
//! ## 排序规则
//!
//! `kind 权重 × 真实使用次数`，其中使用次数 = 全局出现次数 − 定义次数；
//! 类型与模块（Struct / Class / Trait / Interface / Enum / Module）权重 ×2，
//! 函数与方法 ×1——「摸清架构」关心的是构件而非 getter。
//! **通用名过滤**：定义次数 ≥ [`MAX_SHARED_DEFINITIONS`] 的名字从地图剔除
//! （`name` / `path` / `new` / `impl` 回退名等）——它们是多个互不相关的同名
//! 方法，不是「一个被频繁引用的符号」。
//!
//! 第二级过滤针对**高频方法名**（[`is_generic_method_name`]）：`path` / `len` /
//! `text` / `state` 这类「短、全小写、无下划线、多处定义」的方法名，其引用大多
//! 来自标准库/依赖类型的方法调用（`.path()` / `.len()`），不代表本仓库构件。
//!
//! **方法不进地图**：`impl` 块内的函数（[`SymbolKind::Method`]）一律排除——
//! 地图回答「仓库由哪些构件组成」，构件是模块 / 类型 / trait，不是 getter。
//! （依据实测：`session_id` / `len` / `text` / `entry` / `is_empty` 全是 impl
//! 内方法，却因调用量大而占据前排。）

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::error::TianyanError;
use crate::executor::symbols::{language_from_extension, symbol_index, SymbolKind};

/// 单次调用默认预算（字节；约 4 字符/token → ≈1000 token）。
pub const DEFAULT_MAX_BYTES: usize = 4096;
/// 预算硬上限（防止单次调用撑爆上下文）。
pub const HARD_MAX_BYTES: usize = 16 * 1024;
/// 扫描文件数上限（超大仓库保护）。
pub const MAX_FILES: usize = 2000;
/// 单文件字节上限（超过即跳过：超大文件通常是生成物或数据）。
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
/// 输出条目上限（预算之外的二道保护）。
pub const MAX_ENTRIES: usize = 600;
/// 重名度阈值：一个名字的定义次数达到该值即视为**通用名**并从地图中过滤
/// （`name` / `path` / `new` / `impl` 回退名等）。
///
/// 依据实测：本仓库 5751 个符号里 `name` 出现 1099 次、`path` 1102 次——
/// 它们不是「被频繁引用的符号」，而是十几个互不相关的同名 getter/trait 方法；
/// 不剔除会把地图顶层完全淹没（前 20 行全是 `path` / `name`）。
pub const MAX_SHARED_DEFINITIONS: usize = 6;
/// focus 命中权重（远大于任何真实使用次数，保证聚焦符号排到最前）。
const FOCUS_BONUS: usize = 10_000;
/// 通用方法名的最大字符数（配合全小写、无下划线判定）。
const GENERIC_NAME_MAX_LEN: usize = 6;
/// 缓存可容纳的仓库根数量（LRU 逐出）。
const MAX_CACHED_ROOTS: usize = 4;

/// 符号定义（扫描产物条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// 相对 root 的路径（统一 `/` 分隔）。
    pub path: String,
    /// 1 起始行号。
    pub line: usize,
    /// 符号种类。
    pub kind: SymbolKind,
    /// 符号名。
    pub name: String,
}
/// 「通用方法名」判定（第二级过滤，针对逃过重名阈值的高频方法名）。
///
/// 特征：**短**（≤ [`GENERIC_NAME_MAX_LEN`]）、**全小写 ASCII**、**无下划线**
/// 的函数/方法名——这些名字的引用大多来自标准库或依赖类型的方法调用
/// （`.path()` / `.len()` / `.entry()`），不代表本仓库的结构构件。
///
/// 依据实测：`path` 在本仓库仅定义 3 次却「被引用」1100 次；`len` 663 次 /
/// `text` 584 次 / `entry` 540 次同源。类型与模块名不受此规则影响
/// （`vfs`、`db`、`model` 这类短名只要是 Module/Struct/Enum 即保留）；
/// 含下划线的具体命名（`session_manager`）与驼峰类型名同样保留。
fn is_generic_method_name(kind: SymbolKind, name: &str) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
        && name.len() <= GENERIC_NAME_MAX_LEN
        && !name.contains('_')
        && name.chars().all(|c| c.is_ascii_lowercase())
}

/// 一次扫描的产物（可缓存）。
#[derive(Debug, Clone)]
pub struct ScanOutcome {
    /// 实际参与扫描的源文件数。
    pub files: usize,
    /// 跳过的文件数（不支持的语言 / 过大 / 读取失败 / 测试文件被排除）。
    pub skipped: usize,
    /// 定义条目（未排序）。
    pub definitions: Vec<Definition>,
    /// 标识符 → 出现次数（跨文件聚合）。
    pub references: HashMap<String, usize>,
}

impl ScanOutcome {
    /// 因达到 [`MAX_FILES`] 而被截断的扫描（未遍历完整仓库）。
    pub fn is_partial(&self) -> bool {
        self.files >= MAX_FILES
    }
}

/// 扫描选项。
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// 仓库根（绝对路径）。
    pub root: PathBuf,
    /// 是否包含测试文件（缺省排除）。
    pub include_tests: bool,
}

/// 渲染选项。
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// 关键字：名称命中（大小写不敏感）的符号优先排序。
    pub focus: Option<String>,
    /// 骨架字节预算（已钳制到 [`HARD_MAX_BYTES`]）。
    pub max_bytes: usize,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            focus: None,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// 扫描仓库：遍历源文件 → 逐文件提取（定义 + 标识符计数）→ 跨文件聚合。
///
/// 遍历规则与 `grep`（`search_engine`）同口径：`ignore::WalkBuilder` + 隐藏文件
/// /`.gitignore`/全局/本地排除，且显式排除 `.git` 与 `.tianyan`。
pub fn scan(options: &ScanOptions) -> ScanOutcome {
    let mut outcome = ScanOutcome {
        files: 0,
        skipped: 0,
        definitions: Vec::new(),
        references: HashMap::new(),
    };
    let root = options.root.clone();
    for entry in walk_source_files(&root) {
        if outcome.files >= MAX_FILES {
            break;
        }
        let path = entry;
        let Ok(meta) = std::fs::metadata(&path) else {
            outcome.skipped += 1;
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            outcome.skipped += 1;
            continue;
        }
        let rel = relative_path(&root, &path);
        if !options.include_tests && is_test_file(&rel, &path) {
            outcome.skipped += 1;
            continue;
        }
        let Ok(language) = language_from_extension(&path.to_string_lossy()) else {
            outcome.skipped += 1;
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            outcome.skipped += 1;
            continue;
        };
        let Ok(index) = symbol_index(&source, &language) else {
            outcome.skipped += 1;
            continue;
        };
        outcome.files += 1;
        for symbol in index.symbols {
            outcome.definitions.push(Definition {
                path: rel.clone(),
                line: symbol.line,
                kind: symbol.kind,
                name: symbol.name,
            });
        }
        for (name, count) in index.identifiers {
            *outcome.references.entry(name).or_insert(0) += count;
        }
    }
    outcome
}

/// 渲染骨架文本：按「引用度 + focus 加权」降序，预算内逐行累积。
///
/// 返回 `(骨架文本, 条目数, 是否因预算/条数上限截断)`。
pub fn render(outcome: &ScanOutcome, options: &RenderOptions) -> (String, usize, bool) {
    let budget = options.max_bytes.clamp(256, HARD_MAX_BYTES);
    let focus = options
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_lowercase);

    // 名字 → 定义次数（重名度）：识别「通用名」，见 [`MAX_SHARED_DEFINITIONS`]。
    let mut def_counts: HashMap<&str, usize> = HashMap::new();
    for definition in &outcome.definitions {
        *def_counts.entry(definition.name.as_str()).or_insert(0) += 1;
    }

    let mut scored: Vec<(usize, usize, &Definition)> = outcome
        .definitions
        .iter()
        // 方法不是构件：地图只保留模块 / 类型 / trait / 顶层函数。
        .filter(|definition| !matches!(definition.kind, SymbolKind::Method))
        .filter(|definition| def_counts[definition.name.as_str()] < MAX_SHARED_DEFINITIONS)
        .filter(|definition| !is_generic_method_name(definition.kind, &definition.name))
        .map(|definition| {
            let def_count = def_counts[definition.name.as_str()];
            // 真实「被使用次数」= 全局出现次数 − 所有定义处出现次数。
            let uses = outcome
                .references
                .get(&definition.name)
                .copied()
                .unwrap_or(0)
                .saturating_sub(def_count);
            let focused = focus
                .as_deref()
                .map(|f| definition.name.to_lowercase().contains(f))
                .unwrap_or(false);
            let score = kind_weight(definition.kind) * uses + if focused { FOCUS_BONUS } else { 0 };
            (score, uses, definition)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.path.cmp(&b.2.path))
            .then_with(|| a.2.line.cmp(&b.2.line))
    });

    let mut text = String::new();
    let mut rendered = 0usize;
    let mut truncated = false;
    for (_, uses, definition) in scored {
        if rendered >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let line = format_line(definition, uses);
        if text.len() + line.len() > budget {
            truncated = true;
            break;
        }
        text.push_str(&line);
        text.push('\n');
        rendered += 1;
    }
    if truncated {
        let marker = format!(
            "... (地图已截断：共 {} 个符号 / {} 个文件；已显示 {rendered} 条，\
             用 focus 或 path 缩小范围)\n",
            outcome.definitions.len(),
            outcome.files
        );
        text.push_str(&marker);
    }
    (text, rendered, truncated)
}

/// 符号种类权重：类型与模块（架构构件）×2，函数与方法 ×1。
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

/// 单行骨架格式：`路径:行 种类 名称(使用次数)`（使用次数 0 时省略括号）。
fn format_line(definition: &Definition, uses: usize) -> String {
    if uses == 0 {
        format!(
            "{}:{} {:?} {}",
            definition.path, definition.line, definition.kind, definition.name
        )
    } else {
        format!(
            "{}:{} {:?} {}({})",
            definition.path, definition.line, definition.kind, definition.name, uses
        )
    }
}

/// 按仓库根缓存扫描产物：文件指纹（相对路径 + mtime + size）不变即复用。
///
/// 仅缓存**内存**（不落盘、不进 VFS）——它是运行时结构缓存，与 LSP 服务器池、
/// snapshot 同类的运行时产物（`AGENTS.md` 硬约束：不得做成第二存储或第二检索
/// 管道）。
#[derive(Debug, Default)]
pub struct RepoMapCache {
    inner: Mutex<HashMap<PathBuf, CacheEntry>>,
}

#[derive(Debug)]
struct CacheEntry {
    fingerprint: u64,
    outcome: Arc<ScanOutcome>,
    last_used: u64,
}

impl RepoMapCache {
    /// 创建空缓存。
    pub fn new() -> Self {
        Self::default()
    }

    /// 取扫描产物：指纹命中复用缓存，否则重扫并写入（返回第二项为是否命中缓存）。
    pub fn get_or_scan(&self, options: &ScanOptions) -> (Arc<ScanOutcome>, bool) {
        let fingerprint = file_fingerprint(&options.root, options.include_tests);
        let now = now_secs();
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            // 锁中毒（持锁线程 panic）：退化为不缓存，绝不让工具失败。
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = guard.get_mut(&options.root) {
            if entry.fingerprint == fingerprint {
                entry.last_used = now;
                return (entry.outcome.clone(), true);
            }
        }
        let outcome = Arc::new(scan(options));
        guard.insert(
            options.root.clone(),
            CacheEntry {
                fingerprint,
                outcome: outcome.clone(),
                last_used: now,
            },
        );
        if guard.len() > MAX_CACHED_ROOTS {
            if let Some(oldest) = guard
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(root, _)| root.clone())
            {
                guard.remove(&oldest);
            }
        }
        (outcome, false)
    }
}

/// 文件指纹：`(相对路径, mtime 秒, size)` 三元组集合的哈希。
///
/// 只做 `walk + metadata`（**不读文件内容**），代价远低于扫描本身。任何源文件
/// 的新增/删除/改写/改名都会改变指纹；`include_tests` 参与哈希（切换开关即失效）。
fn file_fingerprint(root: &Path, include_tests: bool) -> u64 {
    let mut triples: Vec<(String, i64, u64)> = Vec::new();
    for path in walk_source_files(root) {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            // 毫秒精度：秒级会让「同一秒内连续两次编辑」指纹不变（旧结果被复用）
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        triples.push((relative_path(root, &path), modified, meta.len()));
    }
    triples.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    include_tests.hash(&mut hasher);
    triples.hash(&mut hasher);
    hasher.finish()
}

/// 遍历源文件（按扩展名过滤，遍历规则与 `grep` 同口径）。
///
/// 只返回**支持语言**的文件（`symbols::language_from_extension` 命中）；
/// 其余文件在遍历阶段丢弃，不计入 [`ScanOutcome::skipped`]。
fn walk_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            entry.file_name().to_str() != Some(".git")
                && entry.file_name().to_str() != Some(".tianyan")
        });
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if language_from_extension(&path.to_string_lossy()).is_ok() {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}

/// 相对 root 的路径（统一 `/` 分隔；root 之外的原样返回）。
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 测试文件判定：`*_tests.rs` / `test_*.py` / `*_test.py` / `*.test.*` /
/// `*.spec.*` / 路径含 `tests`、`test`、`__tests__` 目录段。
fn is_test_file(rel: &str, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with("_tests.rs") || name.ends_with("_test.rs") {
        return true;
    }
    if (name.starts_with("test_") && name.ends_with(".py")) || name.ends_with("_test.py") {
        return true;
    }
    if name.contains(".test.") || name.contains(".spec.") {
        return true;
    }
    rel.split('/')
        .any(|segment| segment == "tests" || segment == "__tests__" || segment == "test")
}

/// 当前 Unix 秒（缓存 LRU 计时用；失败回退 0）。
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 供工具层使用的参数校验：预算钳制 + 根路径校验。
pub fn resolve_budget(max_tokens: Option<usize>) -> usize {
    let tokens = max_tokens
        .unwrap_or(DEFAULT_MAX_BYTES / 4)
        .clamp(64, HARD_MAX_BYTES / 4);
    (tokens * 4).clamp(256, HARD_MAX_BYTES)
}

/// 校验根路径存在且为目录。
pub fn ensure_root(root: &Path) -> Result<(), TianyanError> {
    if !root.is_dir() {
        return Err(TianyanError::not_found(format!(
            "executor: repo_map: 路径不存在或不是目录：{}",
            root.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "repo_map_tests.rs"]
mod tests;
