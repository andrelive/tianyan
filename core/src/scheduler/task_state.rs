//! 任务作用域状态存储（G5：定时任务跨运行状态）。
//!
//! 每个定时任务可读写自己的持久状态（`tianyan://memory/events/task_states/`），
//! 实现"任务跨运行延续上下文"（Devin Scheduled Devins carries state
//! between runs 模式）：本轮运行可读取上轮摘要，周期任务无需从零开始。
//!
//! 内置任务自包含（不读状态也能正确工作）；此能力面向**自定义周期任务**
//! 与需要跨轮上下文的内置任务增强（如 MemoryTask 记录周期摘要）。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, ContentLevel, TianyanUri};
use crate::vfs::VirtualFileSystem;

/// 任务状态存储：VFS-backed 的 `name -> markdown` 映射。
///
/// 热路径无关（仅任务执行时读写，低频）；内容走 VFS 内容寻址，
/// 与记忆/知识同一套存储语义。
#[derive(Clone)]
pub struct TaskStateStore {
    vfs: Arc<dyn VirtualFileSystem>,
}

impl std::fmt::Debug for TaskStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskStateStore").finish_non_exhaustive()
    }
}

impl TaskStateStore {
    /// 创建任务状态存储。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs }
    }

    /// 任务状态的 VFS URI。
    fn uri(name: &str) -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Memory,
            vec![
                "events".to_string(),
                "task_states".to_string(),
                format!("{}.md", sanitize(name)),
            ],
        )
    }

    /// 读取任务上次运行写入的状态（不存在返回 None，容错读取）。
    ///
    /// 状态不存在或读取失败一律视为"无历史状态"（返回 None），
    /// 不阻塞任务执行——任务状态是增强信息而非依赖。
    pub async fn read(&self, name: &str) -> Result<Option<String>> {
        let uri = Self::uri(name);
        match self.vfs.read_content(&uri, ContentLevel::Detail).await {
            Ok(content) => {
                if content.trim().is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(content))
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// 写入任务状态（覆盖上次内容）。
    pub async fn write(&self, name: &str, content: &str) -> Result<()> {
        let uri = Self::uri(name);
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }
        self.vfs.write_content(&uri, content).await?;
        // L1 摘要：状态内容首行（渐进式披露时可见概览）
        let first_line = content.lines().next().unwrap_or("").to_string();
        self.vfs.write_abstract(&uri, &first_line).await?;
        Ok(())
    }

    /// 删除任务状态（不存在视为已删除，容错）。
    pub async fn delete(&self, name: &str) -> Result<()> {
        let uri = Self::uri(name);
        let _ = self.vfs.delete(&uri).await;
        Ok(())
    }
}

/// 任务名 → 文件名安全片段（仅保留字母数字与下划线）。
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "task".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::MockVfs;

    #[tokio::test]
    async fn test_read_missing_returns_none() {
        let store = TaskStateStore::new(Arc::new(MockVfs::new()));
        assert!(store.read("ghost").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_write_read_roundtrip() {
        let vfs = Arc::new(MockVfs::new());
        let store = TaskStateStore::new(vfs.clone());

        store.write("memory_extraction", "# 周期摘要\n\n提取 5 条记忆\n").await.unwrap();
        let content = store.read("memory_extraction").await.unwrap().unwrap();
        assert!(content.contains("提取 5 条记忆"));

        // 覆盖写入
        store.write("memory_extraction", "# 周期摘要\n\n提取 8 条记忆\n").await.unwrap();
        let content = store.read("memory_extraction").await.unwrap().unwrap();
        assert!(content.contains("提取 8 条记忆"));
        assert!(!content.contains("5 条"));
    }

    #[tokio::test]
    async fn test_delete_removes_state() {
        let vfs = Arc::new(MockVfs::new());
        let store = TaskStateStore::new(vfs.clone());
        store.write("t1", "内容").await.unwrap();
        assert!(store.read("t1").await.unwrap().is_some());
        store.delete("t1").await.unwrap();
        assert!(store.read("t1").await.unwrap().is_none());
    }

    #[test]
    fn test_sanitize_names() {
        assert_eq!(sanitize("memory_extraction"), "memory_extraction");
        // "任务/名字!" = 6 个非 ASCII 字符 → 6 个下划线
        assert_eq!(sanitize("任务/名字!"), "______");
        assert_eq!(sanitize(""), "task");
    }
}
