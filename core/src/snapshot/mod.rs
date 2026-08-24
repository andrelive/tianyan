//! 工作区文件快照模块 —— 会话回退时恢复 AI 修改过的文件。
//!
//! 设计（参考 opencode 的 git-based snapshot，存储改为内容寻址文件复制）：
//!
//! - **内容寻址存储**：`objects/{sha256 前 2 位}/{sha256}.bin`，同内容只存一份
//! - **轮前快照**：每条用户消息处理前调用 [`SnapshotManager::capture`]，
//!   遍历工作目录生成 `{path → hash}` 树（`trees/{index}.json`），缺失对象时复制原文
//! - **回退恢复**：[`SnapshotManager::restore`] 读取目标树，增量对比当前工作区，
//!   恢复差异文件、删除快照中不存在的新增文件
//! - **排除规则**：`.git`、`node_modules`、`target` 等大目录与超大文件不纳入快照
//!
//! # 对象压缩（磁盘格式决策）
//!
//! 对象库中的字节自 **2026-08-06** 起以 gzip 流存储（`flate2` 默认压缩级别，
//! 写路径 [`SnapshotManager::capture_file`] 压缩）。对象键**不变**，仍为 raw 内容
//! sha256（`hex_sha256`），全局去重语义保持不变。读取时以 gzip 魔数 `0x1f 0x8b`
//! 识别：命中则 `GzDecoder` 解压，否则按 legacy 未压缩对象处理 —— 旧版本快照库
//! 保持可读，混合存储亦可正确恢复。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use ring::digest::{digest, SHA256};
use tokio::fs;

use crate::common::binary::sniff_binary;
use crate::common::error::{Result, TianyanError};

/// 单文件最大纳入快照的字节数（2MB，与 opencode 一致）。
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 默认排除的目录名（不区分大小写）。
const DEFAULT_EXCLUDED_DIRS: [&str; 4] = [".git", "node_modules", "target", "snapshots"];

/// 工作区快照管理器。
///
/// 同一会话内按消息索引组织快照树；不同会话存储隔离。
#[derive(Debug, Clone)]
pub struct SnapshotManager {
    /// 快照存储根目录（`data_dir/snapshots`）。
    pub(crate) root: PathBuf,
    /// 快照的工作目录根（`[agent] working_directory`）。
    pub(crate) workdir: PathBuf,
    /// 额外排除的相对路径或目录名。
    excludes: Vec<String>,
}

/// 单个快照树条目。
pub(crate) type SnapshotTree = HashMap<String, String>;

/// 缓存文件条目（mtime + size 未变时复用 hash,避免全量读取）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedFile {
    /// 修改时间（毫秒时间戳）。
    mtime_ms: u128,
    /// 文件大小。
    size: u64,
    /// 内容 sha256。
    hash: String,
}

/// 文件缓存表：{相对路径 → 缓存条目}。
pub(crate) type FileCache = HashMap<String, CachedFile>;

/// 垃圾回收统计结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcStats {
    /// 删除的孤儿对象数。
    pub objects_removed: usize,
    /// 保留的对象数（`total_objects - objects_removed`）。
    pub objects_retained: usize,
    /// 删除的孤儿重做缓存文件数。
    pub cache_files_removed: usize,
    /// 扫描时的对象总数。
    pub total_objects: usize,
}

/// 工作区与快照的差异结果（前端展示 / 模型理解用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffResult {
    /// 会话 ID。
    pub session_id: String,
    /// 快照索引。
    pub index: usize,
    /// 文件级差异（按路径排序）。
    pub files: Vec<FileDiff>,
}

/// 单个文件的差异。
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileDiff {
    /// 相对路径。
    pub path: String,
    /// 差异状态：`added` | `removed` | `modified` | `binary`。
    pub status: String,
    /// 变更块（added/removed/binary 为空）。
    pub hunks: Vec<Hunk>,
    /// unified diff 文本（binary 为空）。
    pub unified: String,
    /// 旧内容行数（added 为 0）。
    pub old_lines: usize,
    /// 新内容行数（removed 为 0）。
    pub new_lines: usize,
}

/// 变更块坐标（1 起始）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hunk {
    /// 旧内容起始行（1 起始）。
    pub old_start: usize,
    /// 旧内容行数。
    pub old_len: usize,
    /// 新内容起始行（1 起始）。
    pub new_start: usize,
    /// 新内容行数。
    pub new_len: usize,
}

impl SnapshotManager {
    /// 创建快照管理器。
    ///
    /// # Arguments
    /// - `root`: 快照存储根目录
    /// - `workdir`: 被快照的工作目录
    pub fn new(root: PathBuf, workdir: PathBuf) -> Self {
        Self {
            root,
            workdir,
            excludes: Vec::new(),
        }
    }

    /// 快照存储根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 工作目录。
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// 返回绑定到指定工作目录的实例（root/excludes 共享，workdir 替换）。
    ///
    /// 会话级工作区支持：同一管理器按会话工作目录克隆出实例，快照树与对象
    /// 库仍按 session_id 组织（内容寻址对象全局去重，跨工作目录共享）。
    pub fn with_workdir(&self, workdir: PathBuf) -> Self {
        Self {
            root: self.root.clone(),
            workdir,
            excludes: self.excludes.clone(),
        }
    }

    /// 捕获工作区状态为快照（消息处理前调用）。
    ///
    /// 遍历工作目录生成 `{path → sha256}` 树并持久化；内容寻址对象缺失时复制原文。
    /// 已存在对象（内容未变）不重复复制。
    pub async fn capture(&self, session_id: &str, index: usize) -> Result<()> {
        self.validate_session_id(session_id)?;
        if !self.workdir.exists() {
            return Ok(());
        }

        let trees_dir = self.trees_dir(session_id);
        fs::create_dir_all(&trees_dir)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 创建快照树目录失败: {e}")))?;
        let tree_path = trees_dir.join(format!("{}.json", index));
        let cache_path = trees_dir.join(format!("{}.cache.json", index));

        // 复用上一轮的 mtime/size 缓存,避免未变化文件全量读取
        let prev_cache = if index > 0 {
            self.load_cache(&trees_dir.join(format!("{}.cache.json", index - 1)))
                .await?
        } else {
            None
        };

        self.capture_tree_to(&self.workdir, &tree_path, &cache_path, prev_cache.as_ref())
            .await
    }

    /// 恢复到指定消息索引的快照（回退调用）。
    ///
    /// 增量恢复：对比当前工作区与目标树，恢复内容差异的文件、删除目标树中不存在的文件。
    /// 返回恢复/删除的文件数。
    pub async fn restore(&self, session_id: &str, index: usize) -> Result<usize> {
        self.validate_session_id(session_id)?;

        let tree = self.load_tree(session_id, index).await?;
        self.restore_tree(tree, &self.workdir).await
    }

    /// 对比当前工作区与 `(session_id, index)` 快照，返回结构化差异（前端展示 / 模型理解用）。
    ///
    /// 快照树中的文件按内容哈希比较：哈希相同 → 跳过（unchanged）；工作区缺失 → `removed`；
    /// 内容不同 → 读取双方字节，二进制（NUL 字节或控制字符占比 >30%）→ `binary`，
    /// 否则生成 hunks + unified diff → `modified`。当前工作区新增文件 → `added`。
    /// 结果按路径排序，输出确定。
    pub async fn diff(&self, session_id: &str, index: usize) -> Result<DiffResult> {
        self.validate_session_id(session_id)?;
        let tree = self.load_tree(session_id, index).await?;

        let mut current: SnapshotTree = HashMap::new();
        if self.workdir.exists() {
            self.walk_current(&self.workdir, "", &mut current).await?;
        }

        let mut files: Vec<FileDiff> = Vec::new();

        // 1. 快照树侧：removed / binary / modified（unchanged 跳过）
        for (path, tree_hash) in &tree {
            let work_path = self.workdir.join(path);
            match current.get(path) {
                Some(cur_hash) if cur_hash == tree_hash => continue,
                _ => {
                    let meta = match fs::metadata(&work_path).await {
                        Ok(m) => m,
                        Err(_) => {
                            // 工作区缺失 → removed（行数取自对象内容）
                            let old_lines = self.object_line_count(tree_hash).await;
                            files.push(FileDiff {
                                path: path.clone(),
                                status: "removed".to_string(),
                                hunks: Vec::new(),
                                unified: String::new(),
                                old_lines,
                                new_lines: 0,
                            });
                            continue;
                        }
                    };
                    if meta.len() > MAX_FILE_BYTES {
                        files.push(binary_diff(path));
                        continue;
                    }
                    let current_bytes = match fs::read(&work_path).await {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(path = %path, error = %e, "snapshot: diff 跳过无法读取的文件");
                            continue;
                        }
                    };
                    let object_bytes = match fs::read(self.object_path(tree_hash)).await {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(path = %path, error = %e, "snapshot: diff 读取快照对象失败");
                            continue;
                        }
                    };
                    let old_bytes = match read_object_bytes(&object_bytes) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(path = %path, error = %e, "snapshot: diff 解压快照对象失败");
                            continue;
                        }
                    };
                    if sniff_binary(&current_bytes) || sniff_binary(&old_bytes) {
                        files.push(binary_diff(path));
                        continue;
                    }
                    let old_text = String::from_utf8_lossy(&old_bytes).into_owned();
                    let new_text = String::from_utf8_lossy(&current_bytes).into_owned();
                    let text_diff =
                        similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str());
                    files.push(FileDiff {
                        path: path.clone(),
                        status: "modified".to_string(),
                        hunks: build_hunks(text_diff.ops()),
                        unified: text_diff
                            .unified_diff()
                            .header("snapshot", "current")
                            .to_string(),
                        old_lines: old_text.lines().count(),
                        new_lines: new_text.lines().count(),
                    });
                }
            }
        }

        // 2. 当前工作区侧：added（二进制 → binary）
        for path in current.keys() {
            if tree.contains_key(path) {
                continue;
            }
            let work_path = self.workdir.join(path);
            let bytes = match fs::read(&work_path).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "snapshot: diff 跳过无法读取的文件");
                    continue;
                }
            };
            if sniff_binary(&bytes) {
                files.push(binary_diff(path));
            } else {
                files.push(FileDiff {
                    path: path.clone(),
                    status: "added".to_string(),
                    hunks: Vec::new(),
                    unified: String::new(),
                    old_lines: 0,
                    new_lines: String::from_utf8_lossy(&bytes).lines().count(),
                });
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(DiffResult {
            session_id: session_id.to_string(),
            index,
            files,
        })
    }

    /// 垃圾回收：删除所有会话不再引用的对象与孤儿重做缓存文件。
    ///
    /// 标记阶段收集所有会话 `trees/{index}.json` 与 `redo/tree-{index}.json`
    /// 引用的对象哈希（对象库为内容寻址、跨会话全局共享，任一引用即保留）；
    /// 清扫阶段删除 `objects/` 中未被引用的对象并清理空子目录；
    /// 另删除 `redo/` 下缺少对应 `tree-{index}.json` 的孤儿缓存文件
    /// （`load_redo` 遗留的历史泄漏）。`trees/` 下的缓存文件不受影响。
    pub async fn gc(&self) -> Result<GcStats> {
        // 全新数据目录（尚无任何快照）时 root 不存在：无垃圾可清，空操作成功。
        // 否则 scheduler 的 snapshot_gc 任务在首次快照前持续报错（os error 3）。
        if !self.root.is_dir() {
            return Ok(GcStats {
                objects_removed: 0,
                objects_retained: 0,
                cache_files_removed: 0,
                total_objects: 0,
            });
        }

        // ── 标记阶段：收集可达对象哈希 ────────────────────────────
        let mut reachable: HashSet<String> = HashSet::new();
        let mut session_dirs: Vec<PathBuf> = Vec::new();

        let mut root_entries = fs::read_dir(&self.root)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 读取快照根目录失败: {e}")))?;
        while let Some(entry) = root_entries
            .next_entry()
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 遍历快照根目录失败: {e}")))?
        {
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && entry.file_name().to_string_lossy() != "objects" {
                session_dirs.push(entry.path());
            }
        }

        for session_dir in &session_dirs {
            let trees_dir = session_dir.join("trees");
            if trees_dir.is_dir() {
                collect_tree_hashes(&trees_dir, "", &mut reachable).await;
            }
            let redo_dir = session_dir.join("redo");
            if redo_dir.is_dir() {
                collect_tree_hashes(&redo_dir, "tree-", &mut reachable).await;
            }
        }

        // ── 清扫阶段：删除不可达对象并清理空子目录 ──────────────────
        let mut objects_removed = 0usize;
        let mut total_objects = 0usize;
        let objects_dir = self.root.join("objects");
        if objects_dir.is_dir() {
            let mut dir_entries = fs::read_dir(&objects_dir)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取对象目录失败: {e}")))?;
            while let Some(dir_entry) = dir_entries
                .next_entry()
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 遍历对象目录失败: {e}")))?
            {
                if !dir_entry
                    .file_type()
                    .await
                    .map(|t| t.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }
                let dir_path = dir_entry.path();
                let mut file_entries = fs::read_dir(&dir_path).await.map_err(|e| {
                    TianyanError::Custom(format!("snapshot: 读取对象子目录失败: {e}"))
                })?;
                while let Some(file_entry) = file_entries.next_entry().await.map_err(|e| {
                    TianyanError::Custom(format!("snapshot: 遍历对象子目录失败: {e}"))
                })? {
                    let fname = file_entry.file_name().to_string_lossy().into_owned();
                    if !fname.ends_with(".bin") {
                        continue;
                    }
                    total_objects += 1;
                    let hash = fname.trim_end_matches(".bin").to_string();
                    if !reachable.contains(&hash) {
                        match fs::remove_file(file_entry.path()).await {
                            Ok(()) => objects_removed += 1,
                            Err(e) => tracing::warn!(
                                error = %e, hash = %hash,
                                "snapshot: GC 删除孤儿对象失败"
                            ),
                        }
                    }
                }
                // 子目录已空则移除（非空时 remove_dir 失败属预期,不视为错误）
                if let Err(e) = fs::remove_dir(&dir_path).await {
                    tracing::debug!(error = %e, ?dir_path, "snapshot: GC 清理对象子目录失败");
                }
            }
        }

        // ── 孤儿重做缓存清理 ─────────────────────────────────────
        let mut cache_files_removed = 0usize;
        for session_dir in &session_dirs {
            let redo_dir = session_dir.join("redo");
            if !redo_dir.is_dir() {
                continue;
            }
            let mut entries = fs::read_dir(&redo_dir)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取重做目录失败: {e}")))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 遍历重做目录失败: {e}")))?
            {
                let fname = entry.file_name().to_string_lossy().into_owned();
                // 匹配 tree-{index}.cache.json 且无对应 tree-{index}.json → 孤儿
                let Some(index_part) = fname.strip_suffix(".cache.json") else {
                    continue;
                };
                let Some(index) = index_part.strip_prefix("tree-") else {
                    continue;
                };
                if index.is_empty() {
                    continue;
                }
                if !redo_dir.join(format!("{index_part}.json")).exists() {
                    match fs::remove_file(entry.path()).await {
                        Ok(()) => cache_files_removed += 1,
                        Err(e) => tracing::warn!(
                            error = %e, path = %fname,
                            "snapshot: GC 删除孤儿重做缓存失败"
                        ),
                    }
                }
            }
        }

        Ok(GcStats {
            objects_removed,
            objects_retained: total_objects.saturating_sub(objects_removed),
            cache_files_removed,
            total_objects,
        })
    }

    // ─── 内部实现 ─────────────────────────────────────────────

    /// 捕获工作区到指定树文件（核心逻辑,供 capture / save_redo 复用）。
    ///
    /// 传入上一轮缓存时,仅对 mtime/size 变化的文件重新读取并哈希。
    pub(crate) async fn capture_tree_to(
        &self,
        workdir: &Path,
        tree_path: &Path,
        cache_path: &Path,
        prev_cache: Option<&FileCache>,
    ) -> Result<()> {
        let mut tree: SnapshotTree = HashMap::new();
        let mut cache: FileCache = HashMap::new();
        self.walk_and_capture(workdir, "", &mut tree, &mut cache, prev_cache)
            .await?;

        if let Some(parent) = tree_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 创建目录失败: {e}")))?;
        }
        let json = serde_json::to_string(&tree)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 序列化快照树失败: {e}")))?;
        fs::write(tree_path, json)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 写入快照树失败: {e}")))?;

        // 写入本轮缓存,供下一轮复用
        let cache_json = serde_json::to_string(&cache)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 序列化缓存失败: {e}")))?;
        fs::write(cache_path, cache_json)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 写入缓存失败: {e}")))?;

        tracing::debug!(files = tree.len(), "已捕获工作区快照");
        Ok(())
    }

    /// 将工作区恢复到目标树（核心逻辑,供 restore / load_redo 复用）。
    ///
    /// 增量恢复：对比当前工作区与目标树，恢复内容差异的文件、删除目标树中不存在的文件。
    /// 返回恢复/删除的文件数。
    pub(crate) async fn restore_tree(&self, tree: SnapshotTree, workdir: &Path) -> Result<usize> {
        if !workdir.exists() {
            return Ok(0);
        }

        // 1. 收集当前工作区状态
        let mut current: SnapshotTree = HashMap::new();
        self.walk_current(workdir, "", &mut current).await?;

        let mut restored = 0usize;

        // 2. 恢复目标树中的文件（内容不同或缺失时从对象库复制）
        for (rel_path, hash) in &tree {
            match current.get(rel_path) {
                Some(cur_hash) if cur_hash == hash => continue,
                _ => {
                    let object_path = self.object_path(hash);
                    let bytes = fs::read(&object_path).await.map_err(|e| {
                        TianyanError::Custom(format!(
                            "snapshot: 读取快照对象失败（{} 可能未在快照中）: {e}",
                            rel_path
                        ))
                    })?;
                    let bytes = read_object_bytes(&bytes)?;
                    let target = workdir.join(rel_path);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).await.map_err(|e| {
                            TianyanError::Custom(format!("snapshot: 创建目录失败: {e}"))
                        })?;
                    }
                    fs::write(&target, bytes).await.map_err(|e| {
                        TianyanError::Custom(format!("snapshot: 恢复文件失败 {rel_path}: {e}"))
                    })?;
                    restored += 1;
                }
            }
        }

        // 3. 删除目标树中不存在（回退点之后新增）的文件
        for rel_path in current.keys() {
            if !tree.contains_key(rel_path) {
                let target = workdir.join(rel_path);
                if fs::remove_file(&target).await.is_ok() {
                    restored += 1;
                }
            }
        }

        // 4. 清理空目录（可选，尽力而为）
        Self::prune_empty_dirs(workdir).await;

        Ok(restored)
    }

    async fn load_tree(&self, session_id: &str, index: usize) -> Result<SnapshotTree> {
        let tree_path = self.trees_dir(session_id).join(format!("{}.json", index));
        let content = fs::read_to_string(&tree_path).await.map_err(|e| {
            TianyanError::not_found(format!("快照不存在（会话 {session_id} 索引 {index}）: {e}"))
        })?;
        serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 解析快照树失败: {e}")))
    }

    /// 递归遍历工作目录,将纳入快照的文件写入 tree 并复制缺失对象。
    ///
    /// 有上一轮缓存时,仅对 mtime/size 变化的文件重新读取并哈希。
    async fn walk_and_capture(
        &self,
        dir: &Path,
        prefix: &str,
        tree: &mut SnapshotTree,
        cache: &mut FileCache,
        prev_cache: Option<&FileCache>,
    ) -> Result<()> {
        let mut entries = fs::read_dir(dir).await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 读取目录失败 {}: {e}", dir.display()))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 遍历目录失败 {}: {e}", dir.display()))
        })? {
            let name = entry.file_name().to_string_lossy().to_string();
            if self.is_excluded(&name) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取文件类型失败: {e}")))?;

            if file_type.is_dir() {
                Box::pin(self.walk_and_capture(&entry.path(), &rel, tree, cache, prev_cache))
                    .await?;
            } else if file_type.is_file() {
                let meta = entry
                    .metadata()
                    .await
                    .map_err(|e| TianyanError::Custom(format!("snapshot: 读取元数据失败: {e}")))?;
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);

                // 命中缓存:同 mtime + 同 size → 复用 hash,不读内容
                if let Some(prev) = prev_cache.and_then(|c| c.get(&rel)) {
                    if prev.mtime_ms == mtime_ms && prev.size == meta.len() {
                        tree.insert(rel.clone(), prev.hash.clone());
                        cache.insert(
                            rel,
                            CachedFile {
                                mtime_ms,
                                size: meta.len(),
                                hash: prev.hash.clone(),
                            },
                        );
                        continue;
                    }
                }

                match self.capture_file(&entry.path()).await {
                    Ok(Some(hash)) => {
                        tree.insert(rel.clone(), hash.clone());
                        cache.insert(
                            rel,
                            CachedFile {
                                mtime_ms,
                                size: meta.len(),
                                hash,
                            },
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(path = %rel, error = %e, "snapshot: 跳过无法读取的文件");
                    }
                }
            }
        }
        Ok(())
    }

    /// 读取缓存文件。
    async fn load_cache(&self, cache_path: &Path) -> Result<Option<FileCache>> {
        if !cache_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(cache_path)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 读取缓存失败: {e}")))?;
        let cache: FileCache = serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 解析缓存失败: {e}")))?;
        Ok(Some(cache))
    }

    /// 复制单个文件到内容寻址对象库,返回其 sha256（文件不可读时返回错误由调用方跳过）。
    ///
    /// 对象以 gzip 流写入（见模块文档「对象压缩」）；去重键为 raw 字节 sha256,
    /// 已存在对象（内容未变）不重复压缩写入。
    async fn capture_file(&self, path: &Path) -> Result<Option<String>> {
        let bytes = fs::read(path).await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 读取文件失败 {}: {e}", path.display()))
        })?;
        let hash = hex_sha256(&bytes);
        let object_path = self.object_path(&hash);
        if !object_path.exists() {
            let compressed = gzip_compress(&bytes)?;
            fs::create_dir_all(object_path.parent().unwrap_or(&self.root))
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 创建对象目录失败: {e}")))?;
            fs::write(&object_path, compressed)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 写入对象失败: {e}")))?;
        }
        Ok(Some(hash))
    }

    /// 遍历当前工作区,收集 {path → sha256}（与 capture 相同的排除规则）。
    async fn walk_current(&self, dir: &Path, prefix: &str, tree: &mut SnapshotTree) -> Result<()> {
        let mut entries = fs::read_dir(dir).await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 读取目录失败 {}: {e}", dir.display()))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 遍历目录失败 {}: {e}", dir.display()))
        })? {
            let name = entry.file_name().to_string_lossy().to_string();
            if self.is_excluded(&name) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            let file_type = entry
                .file_type()
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取文件类型失败: {e}")))?;

            if file_type.is_dir() {
                Box::pin(self.walk_current(&entry.path(), &rel, tree)).await?;
            } else if file_type.is_file() {
                let meta = entry
                    .metadata()
                    .await
                    .map_err(|e| TianyanError::Custom(format!("snapshot: 读取元数据失败: {e}")))?;
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                if let Ok(bytes) = fs::read(entry.path()).await {
                    tree.insert(rel, hex_sha256(&bytes));
                }
            }
        }
        Ok(())
    }

    /// 读取对象内容并返回其行数（对象缺失/解压失败时返回 0，供 diff 的 removed 状态使用）。
    async fn object_line_count(&self, hash: &str) -> usize {
        let Ok(object_bytes) = fs::read(self.object_path(hash)).await else {
            return 0;
        };
        let Ok(bytes) = read_object_bytes(&object_bytes) else {
            return 0;
        };
        String::from_utf8_lossy(&bytes).lines().count()
    }

    /// 判断路径片段是否应被排除。
    fn is_excluded(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        if DEFAULT_EXCLUDED_DIRS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&lower))
        {
            return true;
        }
        self.excludes
            .iter()
            .any(|e| e.eq_ignore_ascii_case(name) || e.eq_ignore_ascii_case(&lower))
    }

    /// 清理空目录（尽力而为）。
    async fn prune_empty_dirs(root: &Path) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(mut rd) = fs::read_dir(&dir).await {
                let mut empty = true;
                while let Ok(Some(entry)) = rd.next_entry().await {
                    empty = false;
                    if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                        stack.push(entry.path());
                    }
                }
                if empty && dir != root {
                    dirs.push(dir);
                }
            }
        }
        // 从最深目录开始删除
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for dir in dirs {
            if let Err(e) = fs::remove_dir(&dir).await {
                tracing::debug!(error = %e, ?dir, "snapshot: 清理空目录失败");
            }
        }
    }

    fn trees_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id).join("trees")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.root
            .join("objects")
            .join(&hash[..2])
            .join(format!("{}.bin", hash))
    }

    pub(crate) fn validate_session_id(&self, session_id: &str) -> Result<()> {
        if session_id.is_empty()
            || session_id.contains('/')
            || session_id.contains('\\')
            || session_id.contains("..")
        {
            return Err(TianyanError::Custom(format!(
                "snapshot: 无效的会话 ID: {session_id}"
            )));
        }
        Ok(())
    }
}

/// 例如解析目录下形如 `{prefix}{key}.json` 的树文件,收集其引用的对象哈希（GC 标记阶段）。
///
/// `trees/` 传空前缀匹配 `0.json`（消息处理前快照，数字索引）；
/// `redo/` 传 `tree-` 匹配 `tree-{message_id}.json`（重做树，消息 ID 键）。
/// `.cache.json` 等缓存文件不匹配命名规则,直接忽略；格式损坏的树文件以
/// warn 日志跳过（不影响其余文件的回收）。
async fn collect_tree_hashes(dir: &Path, prefix: &str, reachable: &mut HashSet<String>) {
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(index_part) = name.strip_suffix(".json") else {
            continue;
        };
        let Some(index) = index_part.strip_prefix(prefix) else {
            continue;
        };
        if index.is_empty() {
            continue;
        }
        match fs::read_to_string(entry.path()).await {
            Ok(content) => match serde_json::from_str::<SnapshotTree>(&content) {
                Ok(tree) => reachable.extend(tree.values().cloned()),
                Err(e) => {
                    tracing::warn!(path = %name, error = %e, "snapshot: GC 跳过无法解析的树文件");
                }
            },
            Err(e) => {
                tracing::warn!(path = %name, error = %e, "snapshot: GC 读取树文件失败");
            }
        }
    }
}

/// 计算内容的 SHA256 十六进制摘要。
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = digest(&SHA256, bytes);
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

/// gzip 魔数（前 2 字节）,用于区分压缩对象与 legacy 未压缩对象。
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// 将内容压缩为 gzip 流（对象写路径,默认压缩级别）。
fn gzip_compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|e| TianyanError::Custom(format!("snapshot: gzip 压缩失败: {e}")))?;
    encoder
        .finish()
        .map_err(|e| TianyanError::Custom(format!("snapshot: gzip 压缩失败: {e}")))
}

/// 读取对象字节：以 `0x1f 0x8b` 魔数识别 gzip 流并解压,否则按 legacy 未压缩字节返回。
fn read_object_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() >= GZIP_MAGIC.len() && bytes[..GZIP_MAGIC.len()] == GZIP_MAGIC {
        let mut decoder = GzDecoder::new(bytes);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| TianyanError::Custom(format!("snapshot: gzip 解压对象失败: {e}")))?;
        Ok(out)
    } else {
        Ok(bytes.to_vec())
    }
}

/// 构造二进制文件差异条目（无 hunks / unified，行数均为 0）。
fn binary_diff(path: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        status: "binary".to_string(),
        hunks: Vec::new(),
        unified: String::new(),
        old_lines: 0,
        new_lines: 0,
    }
}

/// 从 diff ops 构建变更块：合并相邻的增删改操作（跳过 Equal），每个连续变更簇生成一个 hunk。
///
/// 坐标 1 起始；Delete + Insert 相邻时合并为一个 hunk（同一变更区域）。
fn build_hunks(ops: &[similar::DiffOp]) -> Vec<Hunk> {
    fn flush(hunks: &mut Vec<Hunk>, current: &mut Option<(usize, usize, usize, usize)>) {
        if let Some((old_start, old_end, new_start, new_end)) = current.take() {
            hunks.push(Hunk {
                old_start: old_start + 1,
                old_len: old_end - old_start,
                new_start: new_start + 1,
                new_len: new_end - new_start,
            });
        }
    }

    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<(usize, usize, usize, usize)> = None;
    for op in ops {
        if matches!(op, similar::DiffOp::Equal { .. }) {
            flush(&mut hunks, &mut current);
            continue;
        }
        let old = op.old_range();
        let new = op.new_range();
        match &mut current {
            Some((old_start, old_end, new_start, new_end)) => {
                *old_start = (*old_start).min(old.start);
                *old_end = (*old_end).max(old.end);
                *new_start = (*new_start).min(new.start);
                *new_end = (*new_end).max(new.end);
            }
            None => current = Some((old.start, old.end, new.start, new.end)),
        }
    }
    flush(&mut hunks, &mut current);
    hunks
}

/// 重做子系统（回退后恢复；与 capture/restore/diff/gc 正交，见 redo.rs）。
mod redo;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn setup() -> (tempfile::TempDir, SnapshotManager) {
        let dir = tempdir().unwrap();
        let workdir = dir.path().join("work");
        let snap_root = dir.path().join("snapshots");
        std::fs::create_dir_all(&workdir).unwrap();
        let mgr = SnapshotManager::new(snap_root, workdir);
        (dir, mgr)
    }

    fn write(workdir: &Path, rel: &str, content: &str) {
        let path = workdir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(workdir: &Path, rel: &str) -> String {
        std::fs::read_to_string(workdir.join(rel)).unwrap()
    }

    #[test]
    fn test_with_workdir_swaps_workdir_keeps_root() {
        // 会话级工作区：with_workdir 克隆出绑定不同目录的实例（root/excludes 共享）
        let mgr = SnapshotManager::new(PathBuf::from("root-a"), PathBuf::from("ws-a"));
        let other = mgr.with_workdir(PathBuf::from("ws-b"));
        assert_eq!(other.root(), Path::new("root-a"), "root 应共享");
        assert_eq!(other.workdir(), Path::new("ws-b"), "workdir 应替换");
        assert_eq!(mgr.workdir(), Path::new("ws-a"), "原实例不受影响");
    }

    #[tokio::test]
    async fn test_capture_and_restore_modified_file() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "原始内容");

        mgr.capture("s1", 0).await.unwrap();
        write(&mgr.workdir, "a.txt", "被修改的内容");
        mgr.capture("s1", 1).await.unwrap();

        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "a.txt"), "原始内容");
    }

    #[tokio::test]
    async fn test_restore_deletes_added_files() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "原始内容");
        mgr.capture("s1", 0).await.unwrap();

        write(&mgr.workdir, "new.txt", "新增文件");
        mgr.capture("s1", 1).await.unwrap();

        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 1);
        assert!(!mgr.workdir.join("new.txt").exists());
    }

    #[tokio::test]
    async fn test_restore_recovers_deleted_file() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "keep.txt", "保留");
        write(&mgr.workdir, "gone.txt", "将被删除");
        mgr.capture("s1", 0).await.unwrap();

        std::fs::remove_file(mgr.workdir.join("gone.txt")).unwrap();
        mgr.capture("s1", 1).await.unwrap();

        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "gone.txt"), "将被删除");
    }

    #[tokio::test]
    async fn test_excluded_dirs_not_snapshotted() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "src/main.rs", "fn main() {}");
        write(&mgr.workdir, "node_modules/pkg/index.js", "ignored");
        write(&mgr.workdir, "target/debug/app", "ignored");

        mgr.capture("s1", 0).await.unwrap();
        let tree = mgr.load_tree("s1", 0).await.unwrap();
        assert!(tree.contains_key("src/main.rs"));
        assert!(!tree.contains_key("node_modules/pkg/index.js"));
        assert!(!tree.contains_key("target/debug/app"));
    }

    #[tokio::test]
    async fn test_large_files_skipped() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "small.txt", "small");
        let big = vec![b'x'; (MAX_FILE_BYTES + 1) as usize];
        std::fs::write(mgr.workdir.join("big.bin"), big).unwrap();

        mgr.capture("s1", 0).await.unwrap();
        let tree = mgr.load_tree("s1", 0).await.unwrap();
        assert!(tree.contains_key("small.txt"));
        assert!(!tree.contains_key("big.bin"));
    }

    #[tokio::test]
    async fn test_capture_after_no_changes_is_cheap() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "内容");
        mgr.capture("s1", 0).await.unwrap();

        // 无变化时再次捕获,对象库不应新增对象
        let objects_before = count_files(&mgr.root.join("objects"));
        mgr.capture("s1", 1).await.unwrap();
        let objects_after = count_files(&mgr.root.join("objects"));
        assert_eq!(objects_before, objects_after);
    }

    #[tokio::test]
    async fn test_redo_roundtrip() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "原始");
        mgr.capture("s1", 0).await.unwrap();

        // 模拟消息0修改文件
        write(&mgr.workdir, "a.txt", "v1");
        mgr.capture("s1", 1).await.unwrap();

        // 回退:保存 redo(当前=v1) + 恢复快照0(原始)
        mgr.save_redo("s1", "msg_1", &[]).await.unwrap();
        assert!(mgr.has_redo("s1", "msg_1").await);
        mgr.restore("s1", 0).await.unwrap();
        assert_eq!(read(&mgr.workdir, "a.txt"), "原始");

        // 重做:恢复 v1 + 取回消息
        let (msgs, restored) = mgr.load_redo("s1", "msg_1").await.unwrap().unwrap();
        assert!(msgs.is_empty());
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "a.txt"), "v1");

        // 重做数据一次性:再次加载返回 None
        assert!(!mgr.has_redo("s1", "msg_1").await);
        assert!(mgr.load_redo("s1", "msg_1").await.unwrap().is_none());
    }

    /// 读取对象库中指定哈希的原始字节（不做解压）。
    fn object_bytes(mgr: &SnapshotManager, hash: &str) -> Vec<u8> {
        std::fs::read(
            mgr.root
                .join("objects")
                .join(&hash[..2])
                .join(format!("{}.bin", hash)),
        )
        .unwrap()
    }

    /// 手动写入 legacy 未压缩对象（模拟旧版本快照库）。
    fn write_object_raw(mgr: &SnapshotManager, hash: &str, bytes: &[u8]) {
        let path = mgr
            .root
            .join("objects")
            .join(&hash[..2])
            .join(format!("{}.bin", hash));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn test_compressed_object_roundtrip() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "原始内容");

        mgr.capture("s1", 0).await.unwrap();
        let tree = mgr.load_tree("s1", 0).await.unwrap();
        let hash = tree.get("a.txt").unwrap();

        // 对象文件必须是 gzip 流（魔数 0x1f 0x8b）
        let bytes = object_bytes(&mgr, hash);
        assert_eq!(&bytes[..2], &[0x1f, 0x8b], "对象应以 gzip 魔数开头");

        // 删除工作区文件后恢复 → 逐字节一致
        std::fs::remove_file(mgr.workdir.join("a.txt")).unwrap();
        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "a.txt"), "原始内容");
    }

    #[tokio::test]
    async fn test_legacy_raw_object_still_readable() {
        let (_dir, mgr) = setup().await;
        let content = "遗留未压缩对象内容";
        // 手动构造 legacy 对象库:raw 字节 + sha256 文件名,不经 gzip
        let hash = hex_sha256(content.as_bytes());
        write_object_raw(&mgr, &hash, content.as_bytes());
        let mut tree: SnapshotTree = HashMap::new();
        tree.insert("legacy.txt".to_string(), hash);
        let tree_dir = mgr.trees_dir("s1");
        std::fs::create_dir_all(&tree_dir).unwrap();
        std::fs::write(
            tree_dir.join("0.json"),
            serde_json::to_string(&tree).unwrap(),
        )
        .unwrap();
        // 工作区写入不同内容,触发真实恢复路径
        write(&mgr.workdir, "legacy.txt", "不同内容");

        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "legacy.txt"), "遗留未压缩对象内容");
    }

    #[tokio::test]
    async fn test_compression_ratio() {
        let (_dir, mgr) = setup().await;
        let content = "hello world\n".repeat(5000);
        std::fs::write(mgr.workdir.join("rep.txt"), &content).unwrap();

        mgr.capture("s1", 0).await.unwrap();
        let tree = mgr.load_tree("s1", 0).await.unwrap();
        let hash = tree.get("rep.txt").unwrap();
        let stored = object_bytes(&mgr, hash);

        assert!(
            stored.len() < content.len(),
            "压缩后 {} 字节应小于原始 {} 字节",
            stored.len(),
            content.len()
        );
    }

    #[tokio::test]
    async fn test_dedup_semantics_preserved() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "跨会话相同内容");
        mgr.capture("s1", 0).await.unwrap();
        mgr.capture("s2", 0).await.unwrap();

        // 跨会话全局去重:同内容只存一个对象（基于 raw 字节 sha256）
        assert_eq!(count_files(&mgr.root.join("objects")), 1);
    }

    #[tokio::test]
    async fn test_mixed_store_restores() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "comp.txt", "压缩对象内容");
        write(&mgr.workdir, "raw.txt", "原始对象内容");
        mgr.capture("s1", 0).await.unwrap();

        let tree = mgr.load_tree("s1", 0).await.unwrap();
        // 把 raw.txt 的对象替换为 legacy 未压缩字节 → 混合存储
        let raw_hash = tree.get("raw.txt").unwrap();
        write_object_raw(&mgr, raw_hash, "原始对象内容".as_bytes());

        write(&mgr.workdir, "comp.txt", "修改1");
        write(&mgr.workdir, "raw.txt", "修改2");

        let restored = mgr.restore("s1", 0).await.unwrap();
        assert_eq!(restored, 2);
        assert_eq!(read(&mgr.workdir, "comp.txt"), "压缩对象内容");
        assert_eq!(read(&mgr.workdir, "raw.txt"), "原始对象内容");
    }

    #[tokio::test]
    async fn test_gc_missing_root_is_noop() {
        // 新数据目录（尚无任何快照）时 root 不存在：GC 应为空操作成功，
        // 而非报错——否则 scheduler 的 snapshot_gc 任务在全新安装上持续失败。
        let dir = tempdir().unwrap();
        let workdir = dir.path().join("work");
        let missing_root = dir.path().join("snapshots"); // 故意不创建
        std::fs::create_dir_all(&workdir).unwrap();
        let mgr = SnapshotManager::new(missing_root, workdir);

        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.objects_removed, 0);
        assert_eq!(stats.objects_retained, 0);
        assert_eq!(stats.cache_files_removed, 0);
        assert_eq!(stats.total_objects, 0);
    }

    #[tokio::test]
    async fn test_gc_removes_unreferenced_objects() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "keep.txt", "保留内容");
        mgr.capture("s1", 0).await.unwrap();
        write(&mgr.workdir, "drop.txt", "将被丢弃的内容");
        mgr.capture("s2", 0).await.unwrap();

        // 模拟删除会话 s2:整个 trees 目录被移除
        std::fs::remove_dir_all(mgr.trees_dir("s2")).unwrap();

        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.objects_removed, 1);
        assert_eq!(stats.objects_retained, 1);
        assert_eq!(stats.total_objects, 2);
        assert_eq!(count_files(&mgr.root.join("objects")), 1);
    }

    #[tokio::test]
    async fn test_gc_keeps_shared_objects() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "same.txt", "跨会话共享内容");
        mgr.capture("s1", 0).await.unwrap();
        mgr.capture("s2", 0).await.unwrap();

        // 删除会话 s2 的树,但对象仍被 s1 引用（内容寻址全局共享）
        std::fs::remove_dir_all(mgr.trees_dir("s2")).unwrap();

        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.objects_removed, 0);
        assert!(stats.objects_retained >= 1);
        assert_eq!(count_files(&mgr.root.join("objects")), 1);
    }

    #[tokio::test]
    async fn test_gc_removes_orphaned_redo_cache() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "原始");
        mgr.capture("s1", 0).await.unwrap();

        // 模拟泄漏:save_redo 写入 tree/cache/messages,load_redo 消费后遗留孤儿 cache
        mgr.save_redo("s1", "msg_0", &[]).await.unwrap();
        mgr.load_redo("s1", "msg_0").await.unwrap();
        // 对照组:tree-msg_1.json 与其 cache 同时存在（不应被 GC 删除）
        mgr.save_redo("s1", "msg_1", &[]).await.unwrap();

        let redo_dir = mgr.redo_dir("s1");
        assert!(
            redo_dir.join("tree-msg_0.cache.json").exists(),
            "孤儿重做缓存必须存在"
        );
        assert!(redo_dir.join("tree-msg_1.cache.json").exists());
        assert!(redo_dir.join("tree-msg_1.json").exists());

        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.cache_files_removed, 1);
        assert!(!redo_dir.join("tree-msg_0.cache.json").exists());
        assert!(
            redo_dir.join("tree-msg_1.cache.json").exists(),
            "匹配的重做缓存不应被删除"
        );
        assert!(redo_dir.join("tree-msg_1.json").exists());
    }

    #[tokio::test]
    async fn test_gc_idempotent() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "内容");
        mgr.capture("s1", 0).await.unwrap();

        mgr.gc().await.unwrap();
        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.objects_removed, 0);
        assert_eq!(stats.cache_files_removed, 0);
    }

    #[tokio::test]
    async fn test_gc_keeps_all_live_trees() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "v0");
        mgr.capture("s1", 0).await.unwrap();
        write(&mgr.workdir, "a.txt", "v1");
        mgr.capture("s1", 1).await.unwrap();
        write(&mgr.workdir, "a.txt", "v2");
        mgr.capture("s1", 2).await.unwrap();
        mgr.save_redo("s1", "msg_0", &[]).await.unwrap();

        let stats = mgr.gc().await.unwrap();
        assert_eq!(stats.objects_removed, 0);
        assert_eq!(stats.objects_retained, 3);
        assert_eq!(count_files(&mgr.root.join("objects")), 3);
    }

    fn count_files(dir: &Path) -> usize {
        if !dir.exists() {
            return 0;
        }
        let mut count = 0;
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for entry in rd.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        stack.push(entry.path());
                    } else {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    // ─── diff 测试 ────────────────────────────────────────────

    #[tokio::test]
    async fn test_diff_unchanged_is_empty() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "内容");
        mgr.capture("s1", 0).await.unwrap();

        let result = mgr.diff("s1", 0).await.unwrap();
        assert_eq!(result.files.len(), 0);
        assert_eq!(result.session_id, "s1");
        assert_eq!(result.index, 0);
    }

    #[tokio::test]
    async fn test_diff_modified_file_hunks() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "a.txt", "a\nb\nc\nd\n");
        mgr.capture("s1", 0).await.unwrap();
        write(&mgr.workdir, "a.txt", "a\nX\nc\nd\n");

        let result = mgr.diff("s1", 0).await.unwrap();
        assert_eq!(result.files.len(), 1);
        let file = &result.files[0];
        assert_eq!(file.path, "a.txt");
        assert_eq!(file.status, "modified");
        assert_eq!(file.hunks.len(), 1, "单处修改应生成一个 hunk");
        assert!(file.hunks[0].old_len >= 1);
        assert!(file.hunks[0].new_len >= 1);
        assert_eq!(file.old_lines, 4);
        assert_eq!(file.new_lines, 4);
        assert!(
            file.unified.contains("-b"),
            "unified diff 应包含删除行 -b: {}",
            file.unified
        );
        assert!(
            file.unified.contains("+X"),
            "unified diff 应包含新增行 +X: {}",
            file.unified
        );
    }

    #[tokio::test]
    async fn test_diff_added_file() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "keep.txt", "保留");
        mgr.capture("s1", 0).await.unwrap();
        write(&mgr.workdir, "new.txt", "新增内容");

        let result = mgr.diff("s1", 0).await.unwrap();
        assert_eq!(result.files.len(), 1);
        let file = &result.files[0];
        assert_eq!(file.path, "new.txt");
        assert_eq!(file.status, "added");
        assert_eq!(file.old_lines, 0);
        assert!(file.new_lines >= 1);
        assert!(file.hunks.is_empty());
    }

    #[tokio::test]
    async fn test_diff_removed_file() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "gone.txt", "将被删除");
        mgr.capture("s1", 0).await.unwrap();
        std::fs::remove_file(mgr.workdir.join("gone.txt")).unwrap();

        let result = mgr.diff("s1", 0).await.unwrap();
        assert_eq!(result.files.len(), 1);
        let file = &result.files[0];
        assert_eq!(file.path, "gone.txt");
        assert_eq!(file.status, "removed");
        assert!(file.old_lines >= 1);
        assert_eq!(file.new_lines, 0);
        assert!(file.hunks.is_empty());
    }

    #[tokio::test]
    async fn test_diff_binary_file() {
        let (_dir, mgr) = setup().await;
        write(&mgr.workdir, "bin.dat", "text content");
        mgr.capture("s1", 0).await.unwrap();
        std::fs::write(mgr.workdir.join("bin.dat"), [0u8, 1, 2, 3, 0]).unwrap();

        let result = mgr.diff("s1", 0).await.unwrap();
        assert_eq!(result.files.len(), 1);
        let file = &result.files[0];
        assert_eq!(file.path, "bin.dat");
        assert_eq!(file.status, "binary");
        assert!(file.hunks.is_empty());
        assert!(file.unified.is_empty());
        assert_eq!(file.old_lines, 0);
        assert_eq!(file.new_lines, 0);
    }

    #[tokio::test]
    async fn test_diff_missing_snapshot_errors() {
        let (_dir, mgr) = setup().await;
        let err = mgr.diff("s1", 0).await.unwrap_err();
        assert!(
            err.to_string().contains("快照"),
            "错误信息应包含「快照」: {err}"
        );
    }
}
