//! 摘要生成任务。
//!
//! 本模块提供扫描 VFS 并生成摘要的后台任务。

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};
use crate::vfs::{ContextEntry, ABSTRACT_TOKEN_LIMIT};

/// 已处理 URI 缓存的最大容量，超过后清理最旧的记录防止内存泄漏。
const PROCESSED_CACHE_LIMIT: usize = 10000;

/// 摘要生成任务。
///
/// 扫描 VFS 中缺少摘要的内容并自动生成摘要。
pub struct SummaryTask {
    /// 待处理 URI 队列。
    queue: RwLock<VecDeque<TianyanUri>>,
    /// 已处理 URI 缓存（有容量上限，FIFO 淘汰最旧记录）。
    processed: RwLock<std::collections::HashSet<String>>,
    /// 已处理 URI 的插入顺序（配合 processed 实现 FIFO 淘汰，避免全量清空后重复摘要）。
    processed_order: RwLock<VecDeque<String>>,
    /// 已处理缓存容量上限（测试可调小）。
    processed_cache_limit: usize,
}

impl SummaryTask {
    /// 创建新的摘要任务。
    pub fn new() -> Self {
        Self {
            queue: RwLock::new(VecDeque::new()),
            processed: RwLock::new(std::collections::HashSet::new()),
            processed_order: RwLock::new(VecDeque::new()),
            processed_cache_limit: PROCESSED_CACHE_LIMIT,
        }
    }

    /// 记录已处理 URI（FIFO 淘汰：超过上限时移除最旧记录）。
    async fn record_processed(&self, uri_str: &str) {
        let mut processed = self.processed.write().await;
        let mut order = self.processed_order.write().await;
        // 仅在首次出现时记录插入顺序（重复处理不改变顺序）
        if processed.insert(uri_str.to_string()) {
            order.push_back(uri_str.to_string());
        }
        while processed.len() > self.processed_cache_limit {
            if let Some(oldest) = order.pop_front() {
                processed.remove(&oldest);
            }
        }
    }

    /// 扫描缺少摘要的条目。
    async fn scan_missing_summaries(
        &self,
        ctx: &TaskContext,
    ) -> crate::common::error::Result<Vec<TianyanUri>> {
        let mut missing = Vec::new();
        for &category in ContextNamespace::ALL {
            let root_uri = TianyanUri::new(category, vec![]);
            self.scan_directory_recursive(ctx, &root_uri, &mut missing)
                .await?;
        }

        Ok(missing)
    }

    /// 递归扫描目录。
    async fn scan_directory_recursive(
        &self,
        ctx: &TaskContext,
        uri: &TianyanUri,
        missing: &mut Vec<TianyanUri>,
    ) -> crate::common::error::Result<()> {
        let entries = ctx.vfs.list(uri).await?;

        for entry in entries {
            if entry.is_directory() {
                Box::pin(self.scan_directory_recursive(ctx, entry.uri(), missing)).await?;
            } else if self.needs_summary(ctx, &entry).await? {
                missing.push(entry.metadata.uri.clone());
            }
        }

        Ok(())
    }

    /// 检查条目是否需要摘要。
    async fn needs_summary(
        &self,
        ctx: &TaskContext,
        entry: &ContextEntry,
    ) -> crate::common::error::Result<bool> {
        // 检查是否已处理
        let uri_str = entry.metadata.uri.to_string();
        let is_processed = {
            let processed = self.processed.read().await;
            processed.contains(&uri_str)
        };
        if is_processed {
            return Ok(false);
        }

        // 检查元数据
        let metadata = ctx
            .vfs
            .get_all_content_metadata(&entry.metadata.uri)
            .await?;

        let detail_meta = match metadata.get(&ContentLevel::Detail) {
            Some(m) => m,
            _ => return Ok(false),
        };

        let has_abstract = metadata.contains_key(&ContentLevel::Abstract);
        let has_overview = metadata.contains_key(&ContentLevel::Overview);

        if !has_abstract || !has_overview {
            return Ok(true);
        }

        let abstract_meta = match metadata.get(&ContentLevel::Abstract) {
            Some(m) => m,
            None => return Ok(true),
        };
        let overview_meta = match metadata.get(&ContentLevel::Overview) {
            Some(m) => m,
            None => return Ok(true),
        };

        if detail_meta.updated_at > abstract_meta.updated_at
            || detail_meta.updated_at > overview_meta.updated_at
        {
            return Ok(true);
        }

        Ok(false)
    }

    /// 处理单个 URI。
    async fn process_uri(
        &self,
        ctx: &TaskContext,
        uri: &TianyanUri,
    ) -> crate::common::error::Result<()> {
        // 读取 Detail 内容
        let detail_content = ctx
            .vfs
            .read_content(uri, ContentLevel::Detail)
            .await
            .map_err(|_| {
                crate::common::error::TianyanError::Custom(format!(
                    "摘要生成错误：无法读取 Detail：{}",
                    uri
                ))
            })?;

        let (abstract_content, overview_content) =
            if detail_content.len() < ABSTRACT_TOKEN_LIMIT * 4 {
                tracing::debug!("短内容直接用作摘要：{}", uri);
                (detail_content.clone(), detail_content)
            } else {
                tracing::debug!("长内容生成摘要：{}", uri);
                ctx.summary_engine
                    .generate_summaries(&detail_content)
                    .await?
            };

        // 写入摘要
        ctx.vfs.write_abstract(uri, &abstract_content).await?;
        ctx.vfs.write_overview(uri, &overview_content).await?;

        // 更新向量
        ctx.vfs
            .update_summary_vectors(uri, &abstract_content, &overview_content)
            .await?;

        // 标记为已处理（带容量限制，FIFO 淘汰最旧记录）
        self.record_processed(uri.as_str()).await;

        tracing::info!("摘要生成完成：{}", uri);
        Ok(())
    }
}

impl Default for SummaryTask {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskHandler for SummaryTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行摘要生成任务");

        // 1. 扫描缺少摘要的条目
        let missing = match self.scan_missing_summaries(ctx).await {
            Ok(uris) => uris,
            Err(e) => {
                return TaskResult::failed(format!("扫描失败：{}", e));
            }
        };

        if missing.is_empty() {
            tracing::debug!("没有需要生成摘要的条目");
            return TaskResult::success(0);
        }

        tracing::info!("发现 {} 个需要生成摘要的条目", missing.len());

        // 2. 加入队列
        {
            let mut queue = self.queue.write().await;
            for uri in missing {
                queue.push_back(uri);
            }
        }

        // 3. 处理队列
        let mut processed_count = 0;
        loop {
            let uri = {
                let mut queue = self.queue.write().await;
                queue.pop_front()
            };

            let Some(uri) = uri else {
                break;
            };

            if let Err(e) = self.process_uri(ctx, &uri).await {
                tracing::warn!("处理失败：{} - {}", uri, e);
            } else {
                processed_count += 1;
            }
        }

        tracing::info!("摘要生成任务完成，处理了 {} 个条目", processed_count);
        TaskResult::success(processed_count)
    }

    fn name(&self) -> &str {
        "summary_generation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_task_new() {
        let task = SummaryTask::new();
        assert_eq!(task.name(), "summary_generation");
    }

    #[test]
    fn test_summary_task_default() {
        let task: SummaryTask = Default::default();
        assert_eq!(task.name(), "summary_generation");
    }

    #[tokio::test]
    async fn test_processed_cache_fifo_eviction() {
        // 容量超限时淘汰最旧记录，而非全量清空（避免已摘要条目被重复处理）
        let mut task = SummaryTask::new();
        task.processed_cache_limit = 3;
        for i in 0..5 {
            task.record_processed(&format!("uri-{}", i)).await;
        }
        let processed = task.processed.read().await;
        assert_eq!(processed.len(), 3);
        assert!(!processed.contains("uri-0"));
        assert!(!processed.contains("uri-1"));
        assert!(processed.contains("uri-2"));
        assert!(processed.contains("uri-3"));
        assert!(processed.contains("uri-4"));
    }

    #[tokio::test]
    async fn test_processed_cache_duplicate_keeps_original_order() {
        let mut task = SummaryTask::new();
        task.processed_cache_limit = 2;
        task.record_processed("a").await;
        task.record_processed("b").await;
        task.record_processed("a").await; // 重复处理不改变插入顺序
        task.record_processed("c").await; // 淘汰最旧的 a，b 保留
        let processed = task.processed.read().await;
        assert!(processed.contains("b"));
        assert!(processed.contains("c"));
        assert!(!processed.contains("a"));
    }
}
