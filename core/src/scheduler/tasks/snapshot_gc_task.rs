//! 快照垃圾回收任务。
//!
//! 定期调用 [`SnapshotManager::gc`] 清理工作区快照中的孤儿对象
//! （对象级标记-清除 + 孤儿重做缓存清理）。
//! 快照存储独立于 VFS（ADR-006 例外），故依赖通过构造器注入，
//! 而非从 [`TaskContext`] 读取。

use std::sync::Arc;

use async_trait::async_trait;

use crate::common::error::TianyanError;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};
use crate::snapshot::SnapshotManager;

/// 快照垃圾回收任务。
pub struct SnapshotGcTask {
    /// 快照管理器（必须注入——`TaskContext` 不携带快照依赖）。
    snapshot_manager: Arc<SnapshotManager>,
}

impl SnapshotGcTask {
    /// 创建新的快照垃圾回收任务。
    ///
    /// 快照管理器为强依赖，不提供 `Default`。
    pub fn new(snapshot_manager: Arc<SnapshotManager>) -> Self {
        Self { snapshot_manager }
    }
}

#[async_trait]
impl TaskHandler for SnapshotGcTask {
    async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行快照垃圾回收...");

        match self.snapshot_manager.gc().await {
            Ok(stats) => {
                let removed = stats.objects_removed + stats.cache_files_removed;
                tracing::info!(
                    objects_removed = stats.objects_removed,
                    objects_retained = stats.objects_retained,
                    cache_files_removed = stats.cache_files_removed,
                    total_objects = stats.total_objects,
                    "快照垃圾回收完成"
                );
                TaskResult::success(removed)
            }
            Err(e) => {
                tracing::warn!(error = %e, "快照垃圾回收失败");
                TaskResult::failed(TianyanError::Custom(format!(
                    "scheduler: snapshot_gc: {}",
                    e
                )))
            }
        }
    }

    fn name(&self) -> &str {
        "snapshot_gc"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    use crate::memory::{ExtractionConfig, MemoryExtractor};
    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::SummaryEngine;

    /// 构造快照 GC 测试上下文（MockVfs + mock 服务；任务本身不读取上下文）。
    fn make_context(vfs: Arc<MockVfs>) -> TaskContext {
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let memory_extractor = Arc::new(MemoryExtractor::new(
            chat.clone(),
            ExtractionConfig::default(),
        ));
        let skill_reviewer = Arc::new(crate::skills::SkillReviewer::new(
            chat,
            vfs.clone(),
            "test-model".to_string(),
        ));
        let config = Arc::new(crate::config::TianyanConfig::default());
        TaskContext::new(
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
        )
    }

    /// 构造隔离的 SnapshotManager（临时目录，模式同 `snapshot::tests::setup`）。
    fn make_manager() -> (tempfile::TempDir, Arc<SnapshotManager>) {
        let dir = tempdir().unwrap();
        let workdir = dir.path().join("work");
        let snap_root = dir.path().join("snapshots");
        std::fs::create_dir_all(&workdir).unwrap();
        (dir, Arc::new(SnapshotManager::new(snap_root, workdir)))
    }

    fn write(workdir: &std::path::Path, rel: &str, content: &str) {
        let path = workdir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn test_snapshot_gc_task_name() {
        let (_dir, mgr) = make_manager();
        let task = SnapshotGcTask::new(mgr);
        assert_eq!(task.name(), "snapshot_gc");
    }

    #[tokio::test]
    async fn test_snapshot_gc_execute_removes_orphan_objects() {
        let (_dir, mgr) = make_manager();
        write(mgr.workdir(), "keep.txt", "保留内容");
        mgr.capture("s1", 0).await.unwrap();
        write(mgr.workdir(), "drop.txt", "将被丢弃的内容");
        mgr.capture("s2", 0).await.unwrap();

        // 模拟删除会话 s2：整个会话目录被移除 → drop.txt 的对象成为孤儿
        std::fs::remove_dir_all(mgr.root().join("s2")).unwrap();

        let task = SnapshotGcTask::new(mgr);
        let result = task.execute(&make_context(Arc::new(MockVfs::new()))).await;

        assert!(result.success, "GC 应成功");
        assert_eq!(result.processed_count, 1, "应回收 1 个孤儿对象");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_snapshot_gc_execute_removes_orphaned_redo_cache() {
        let (_dir, mgr) = make_manager();
        write(mgr.workdir(), "a.txt", "原始");
        mgr.capture("s1", 0).await.unwrap();

        // 模拟泄漏：save_redo 写入后 load_redo 消费，遗留孤儿 cache 文件
        mgr.save_redo("s1", "msg_0", &[]).await.unwrap();
        mgr.load_redo("s1", "msg_0").await.unwrap();
        assert!(
            mgr.root().join("s1/redo/tree-msg_0.cache.json").exists(),
            "孤儿重做缓存必须存在"
        );

        let task = SnapshotGcTask::new(mgr);
        let result = task.execute(&make_context(Arc::new(MockVfs::new()))).await;

        assert!(result.success, "GC 应成功");
        assert_eq!(result.processed_count, 1, "应回收 1 个孤儿重做缓存");
    }

    #[tokio::test]
    async fn test_snapshot_gc_execute_clean_workspace_succeeds() {
        let (_dir, mgr) = make_manager();
        write(mgr.workdir(), "a.txt", "内容");
        mgr.capture("s1", 0).await.unwrap();

        let task = SnapshotGcTask::new(mgr);
        let result = task.execute(&make_context(Arc::new(MockVfs::new()))).await;

        assert!(result.success, "干净工作区 GC 应成功");
        assert_eq!(result.processed_count, 0, "无可回收内容");
        assert!(result.error.is_none());
    }
}
