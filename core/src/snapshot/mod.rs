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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ring::digest::{digest, SHA256};
use tokio::fs;

use crate::common::error::{Result, TianyanError};
use crate::common::types::StructuredMessage;

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
    root: PathBuf,
    /// 快照的工作目录根（`[agent] working_directory`）。
    workdir: PathBuf,
    /// 额外排除的相对路径或目录名。
    excludes: Vec<String>,
}

/// 单个快照树条目。
type SnapshotTree = HashMap<String, String>;

/// 缓存文件条目（mtime + size 未变时复用 hash,避免全量读取）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedFile {
    /// 修改时间（毫秒时间戳）。
    mtime_ms: u128,
    /// 文件大小。
    size: u64,
    /// 内容 sha256。
    hash: String,
}

/// 文件缓存表：{相对路径 → 缓存条目}。
type FileCache = HashMap<String, CachedFile>;

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

    /// 添加额外排除项（相对工作目录的路径片段或目录名，匹配即跳过）。
    pub fn with_exclude(mut self, path: impl Into<String>) -> Self {
        self.excludes.push(path.into());
        self
    }

    /// 快照存储根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 工作目录。
    pub fn workdir(&self) -> &Path {
        &self.workdir
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

    /// 回退时保存重做状态：捕获当前工作区树 + 被截断的消息。
    ///
    /// 重做数据存于 `{root}/{session_id}/redo/` 下，按消息索引组织。
    pub async fn save_redo(
        &self,
        session_id: &str,
        index: usize,
        messages: &[StructuredMessage],
    ) -> Result<()> {
        self.validate_session_id(session_id)?;

        let redo_dir = self.redo_dir(session_id);
        fs::create_dir_all(&redo_dir)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 创建重做目录失败: {e}")))?;

        // 1. 捕获当前工作区树（对象库全局去重,内容已存在则不重复存储）
        if self.workdir.exists() {
            let tree_path = redo_dir.join(format!("tree-{}.json", index));
            let cache_path = redo_dir.join(format!("tree-{}.cache.json", index));
            self.capture_tree_to(&self.workdir, &tree_path, &cache_path, None)
                .await?;
        }

        // 2. 保存被截断的消息
        let messages_path = redo_dir.join(format!("messages-{}.json", index));
        let json = serde_json::to_string(messages)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 序列化重做消息失败: {e}")))?;
        fs::write(&messages_path, json)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 写入重做消息失败: {e}")))?;

        tracing::debug!(session = %session_id, index, "已保存重做状态");
        Ok(())
    }

    /// 重做：恢复重做工作区树,返回被截断的消息与恢复的文件数。
    ///
    /// 重做成功后自动清理该索引的重做数据（一次性语义）。
    pub async fn load_redo(
        &self,
        session_id: &str,
        index: usize,
    ) -> Result<Option<(Vec<StructuredMessage>, usize)>> {
        self.validate_session_id(session_id)?;

        let redo_dir = self.redo_dir(session_id);
        let messages_path = redo_dir.join(format!("messages-{}.json", index));
        if !messages_path.exists() {
            return Ok(None);
        }

        // 1. 恢复工作区树
        let mut restored = 0usize;
        let tree_path = redo_dir.join(format!("tree-{}.json", index));
        if tree_path.exists() {
            let content = fs::read_to_string(&tree_path)
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 读取重做树失败: {e}")))?;
            let tree: SnapshotTree = serde_json::from_str(&content)
                .map_err(|e| TianyanError::Custom(format!("snapshot: 解析重做树失败: {e}")))?;
            restored = self.restore_tree(tree, &self.workdir).await?;
        }

        // 2. 读取被截断的消息
        let content = fs::read_to_string(&messages_path)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 读取重做消息失败: {e}")))?;
        let messages: Vec<StructuredMessage> = serde_json::from_str(&content)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 解析重做消息失败: {e}")))?;

        // 3. 清理重做数据（一次性）
        if let Err(e) = fs::remove_file(&messages_path).await {
            tracing::debug!(error = %e, session = %session_id, "snapshot: 清理重做消息文件失败");
        }
        if let Err(e) = fs::remove_file(&tree_path).await {
            tracing::debug!(error = %e, session = %session_id, "snapshot: 清理重做树文件失败");
        }

        tracing::info!(session = %session_id, index, restored, "已重做工作区文件");
        Ok(Some((messages, restored)))
    }

    /// 检查指定索引的快照是否存在。
    pub async fn has_snapshot(&self, session_id: &str, index: usize) -> bool {
        let path = self.trees_dir(session_id).join(format!("{}.json", index));
        path.exists()
    }

    /// 检查指定索引的重做状态是否存在。
    pub async fn has_redo(&self, session_id: &str, index: usize) -> bool {
        self.redo_dir(session_id)
            .join(format!("messages-{}.json", index))
            .exists()
    }

    // ─── 内部实现 ─────────────────────────────────────────────

    /// 捕获工作区到指定树文件（核心逻辑,供 capture / save_redo 复用）。
    ///
    /// 传入上一轮缓存时,仅对 mtime/size 变化的文件重新读取并哈希。
    async fn capture_tree_to(
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
    async fn restore_tree(&self, tree: SnapshotTree, workdir: &Path) -> Result<usize> {
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
            TianyanError::Custom(format!(
                "snapshot: 快照不存在（会话 {session_id} 索引 {index}）: {e}"
            ))
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
    async fn capture_file(&self, path: &Path) -> Result<Option<String>> {
        let bytes = fs::read(path).await.map_err(|e| {
            TianyanError::Custom(format!("snapshot: 读取文件失败 {}: {e}", path.display()))
        })?;
        let hash = hex_sha256(&bytes);
        let object_path = self.object_path(&hash);
        if !object_path.exists() {
            fs::create_dir_all(object_path.parent().unwrap_or(&self.root))
                .await
                .map_err(|e| TianyanError::Custom(format!("snapshot: 创建对象目录失败: {e}")))?;
            fs::write(&object_path, bytes)
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

    fn redo_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id).join("redo")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.root
            .join("objects")
            .join(&hash[..2])
            .join(format!("{}.bin", hash))
    }

    fn validate_session_id(&self, session_id: &str) -> Result<()> {
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

/// 计算内容的 SHA256 十六进制摘要。
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = digest(&SHA256, bytes);
    let mut hex = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

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
        mgr.save_redo("s1", 1, &[]).await.unwrap();
        assert!(mgr.has_redo("s1", 1).await);
        mgr.restore("s1", 0).await.unwrap();
        assert_eq!(read(&mgr.workdir, "a.txt"), "原始");

        // 重做:恢复 v1 + 取回消息
        let (msgs, restored) = mgr.load_redo("s1", 1).await.unwrap().unwrap();
        assert!(msgs.is_empty());
        assert_eq!(restored, 1);
        assert_eq!(read(&mgr.workdir, "a.txt"), "v1");

        // 重做数据一次性:再次加载返回 None
        assert!(!mgr.has_redo("s1", 1).await);
        assert!(mgr.load_redo("s1", 1).await.unwrap().is_none());
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
}
