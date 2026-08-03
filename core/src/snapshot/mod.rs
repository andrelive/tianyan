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

        let mut tree: SnapshotTree = HashMap::new();
        self.walk_and_capture(&self.workdir, "", &mut tree).await?;

        let trees_dir = self.trees_dir(session_id);
        fs::create_dir_all(&trees_dir)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 创建快照树目录失败: {e}")))?;
        let tree_path = trees_dir.join(format!("{}.json", index));
        let json = serde_json::to_string(&tree)
            .map_err(|e| TianyanError::Custom(format!("snapshot: 序列化快照树失败: {e}")))?;
        fs::write(&tree_path, json)
            .await
            .map_err(|e| TianyanError::Custom(format!("snapshot: 写入快照树失败: {e}")))?;

        tracing::debug!(
            session = %session_id,
            index,
            files = tree.len(),
            "已捕获工作区快照"
        );
        Ok(())
    }

    /// 恢复到指定消息索引的快照（回退调用）。
    ///
    /// 增量恢复：对比当前工作区与目标树，恢复内容差异的文件、删除目标树中不存在的文件。
    /// 返回恢复/删除的文件数。
    pub async fn restore(&self, session_id: &str, index: usize) -> Result<usize> {
        self.validate_session_id(session_id)?;

        let tree = self.load_tree(session_id, index).await?;
        if !self.workdir.exists() {
            return Ok(0);
        }

        // 1. 收集当前工作区状态
        let mut current: SnapshotTree = HashMap::new();
        self.walk_current(&self.workdir, "", &mut current).await?;

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
                    let target = self.workdir.join(rel_path);
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
                let target = self.workdir.join(rel_path);
                if fs::remove_file(&target).await.is_ok() {
                    restored += 1;
                }
            }
        }

        // 4. 清理空目录（可选，尽力而为）
        Self::prune_empty_dirs(&self.workdir).await;

        tracing::info!(
            session = %session_id,
            index,
            restored,
            "已回退工作区文件"
        );
        Ok(restored)
    }

    /// 检查指定索引的快照是否存在。
    pub async fn has_snapshot(&self, session_id: &str, index: usize) -> bool {
        let path = self.trees_dir(session_id).join(format!("{}.json", index));
        path.exists()
    }

    // ─── 内部实现 ─────────────────────────────────────────────

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
    async fn walk_and_capture(
        &self,
        dir: &Path,
        prefix: &str,
        tree: &mut SnapshotTree,
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
                Box::pin(self.walk_and_capture(&entry.path(), &rel, tree)).await?;
            } else if file_type.is_file() {
                let meta = entry
                    .metadata()
                    .await
                    .map_err(|e| TianyanError::Custom(format!("snapshot: 读取元数据失败: {e}")))?;
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                match self.capture_file(&entry.path(), &rel).await {
                    Ok(Some(hash)) => {
                        tree.insert(rel, hash);
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

    /// 复制单个文件到内容寻址对象库,返回其 sha256（文件不可读时返回错误由调用方跳过）。
    async fn capture_file(&self, path: &Path, _rel: &str) -> Result<Option<String>> {
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
            let _ = fs::remove_dir(&dir).await;
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
