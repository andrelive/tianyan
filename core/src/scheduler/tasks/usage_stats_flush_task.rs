//! 使用统计刷盘任务。
//!
//! 定期调用 [`UsageStats::flush`] 把内存中的热路径计数器批量刷入 SQLite，
//! 使内存有界（无人查询统计端点时计数不会无限累积），并保证统计数据时效。
//! 统计存储经共享 `SqliteDb`（ADR-005），非 VFS 内容，故依赖通过构造器注入，
//! 而非从 [`TaskContext`] 读取（与 [`SnapshotGcTask`] 同模式）。

use std::sync::Arc;

use async_trait::async_trait;

use crate::common::error::TianyanError;
use crate::observability::usage_stats::UsageStats;
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 使用统计刷盘任务。
pub struct UsageStatsFlushTask {
    /// 使用统计追踪器（必须注入——`TaskContext` 不携带可观测性依赖）。
    usage_stats: Arc<UsageStats>,
}

impl UsageStatsFlushTask {
    /// 创建新的统计刷盘任务。
    ///
    /// 统计追踪器为强依赖，不提供 `Default`。
    pub fn new(usage_stats: Arc<UsageStats>) -> Self {
        Self { usage_stats }
    }
}

#[async_trait]
impl TaskHandler for UsageStatsFlushTask {
    async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
        tracing::debug!("开始执行使用统计刷盘...");
        match self.usage_stats.flush().await {
            Ok(()) => {
                tracing::debug!("使用统计刷盘完成");
                TaskResult::success(0)
            }
            Err(e) => {
                tracing::warn!(error = %e, "使用统计刷盘失败");
                TaskResult::failed(TianyanError::Custom(format!(
                    "scheduler: usage_stats_flush: {e}"
                )))
            }
        }
    }

    fn name(&self) -> &str {
        "usage_stats_flush"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::memory::{ExtractionConfig, MemoryExtractor};
    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::backend::sqlite_db::SqliteDb;
    use crate::vfs::SummaryEngine;

    /// 构造任务执行上下文（MockVfs + mock 服务；任务本身不读取上下文）。
    fn make_context(vfs: Arc<MockVfs>) -> TaskContext {
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let skill_reviewer = Arc::new(crate::skills::SkillReviewer::new(
            chat.clone(),
            vfs.clone(),
            "test-model".to_string(),
        ));
        let memory_extractor = Arc::new(MemoryExtractor::new(chat, ExtractionConfig::default()));
        let config = Arc::new(crate::config::TianyanConfig::default());
        TaskContext::new(
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
        )
    }

    /// 构造隔离的 UsageStats（内存 SQLite，模式同 `usage_stats::tests::setup`）。
    async fn make_stats() -> Arc<UsageStats> {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        UsageStats::new(db).unwrap()
    }

    #[test]
    fn test_flush_task_name() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let stats = rt.block_on(make_stats());
        let task = UsageStatsFlushTask::new(stats);
        assert_eq!(task.name(), "usage_stats_flush");
    }

    #[tokio::test]
    async fn test_flush_task_execute_persists_counters() {
        let stats = make_stats().await;
        stats.record_skill_call("skill:planning", true, 5000);
        stats.record_skill_call("read_file", true, 3000); // 工具不计入技能口径
        stats.record_doc_hit("tianyan://knowledge/code/main.rs", 0.8);

        let task = UsageStatsFlushTask::new(stats.clone());
        let result = task.execute(&make_context(Arc::new(MockVfs::new()))).await;

        assert!(result.success, "刷盘应成功");
        assert!(result.error.is_none());
        // 计数器已落库：查询可见（查询前自动刷盘 + 本任务刷盘双保险）；
        // 口径：只有 skill: 前缀计入技能
        let top = stats.query_top_skills(10).await;
        assert_eq!(top.len(), 1, "技能计数应已持久化并可查询");
        assert_eq!(top[0].skill_id, "planning");
        assert_eq!(top[0].total_calls, 1);
    }

    #[tokio::test]
    async fn test_flush_task_execute_without_pending_is_success() {
        let stats = make_stats().await;

        let task = UsageStatsFlushTask::new(stats.clone());
        let result = task.execute(&make_context(Arc::new(MockVfs::new()))).await;

        assert!(result.success, "无待刷数据时刷盘应成功（空批量短路）");
        assert!(result.error.is_none());
    }
}
