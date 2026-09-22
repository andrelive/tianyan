//! 摘要生成任务。
//!
//! 本模块提供扫描 VFS 并生成摘要的后台任务。

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::common::token_estimator::estimate_tokens;
use crate::common::types::memory_paths;
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
            // ADR-017：会话回忆改 FTS5 倒排索引，会话命名空间不再生成 L0/L1
            if category == ContextNamespace::Session {
                continue;
            }
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
            // 运维数据子域（extraction_state/evolution_reports/task_states/archive）
            // 不参与摘要：非记忆内容，不生成 L0/L1 与向量
            if memory_paths::is_operational_path(entry.uri()) {
                continue;
            }
            if entry.is_directory() {
                Box::pin(self.scan_directory_recursive(ctx, entry.uri(), missing)).await?;
            } else if self.needs_summary(ctx, &entry).await? {
                missing.push(entry.metadata.uri.clone());
            }
        }

        Ok(())
    }

    /// 检查条目是否需要摘要。
    ///
    /// 判定顺序（T0-4）：**内容更新时间戳检测优先于 processed 缓存**——
    /// Detail 比任一摘要新说明内容被编辑过，必须重建摘要与向量；此前
    /// 缓存短路在时间戳检测之前（条目一旦处理过即跳过），内容更新后
    /// 摘要/向量永不刷新，直到缓存 FIFO 淘汰或进程重启。缓存只用于
    /// “内容未变时不重复处理”（防重复 LLM 调用）。
    async fn needs_summary(
        &self,
        ctx: &TaskContext,
        entry: &ContextEntry,
    ) -> crate::common::error::Result<bool> {
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

        if has_abstract && has_overview {
            // 内容被编辑（Detail 比摘要新）→ 必须重建（无视 processed 缓存）
            let detail_newer_than = |level: &ContentLevel| {
                metadata
                    .get(level)
                    .map(|m| detail_meta.updated_at > m.updated_at)
                    .unwrap_or(true)
            };
            if detail_newer_than(&ContentLevel::Abstract)
                || detail_newer_than(&ContentLevel::Overview)
            {
                return Ok(true);
            }
        }

        // 内容未变（或摘要缺失）：processed 缓存防重复处理
        let uri_str = entry.metadata.uri.to_string();
        let is_processed = {
            let processed = self.processed.read().await;
            processed.contains(&uri_str)
        };
        if is_processed {
            return Ok(false);
        }

        Ok(!has_abstract || !has_overview)
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

        // L0：极短内容全文直用；否则走 LLM 摘要
        let abstract_content = if estimate_tokens(&detail_content) < ABSTRACT_TOKEN_LIMIT {
            tracing::debug!("短内容直接用作摘要：{}", uri);
            detail_content.clone()
        } else {
            tracing::debug!("长内容生成摘要：{}", uri);
            ctx.summary_engine
                .generate_abstract(&detail_content)
                .await?
        };
        // L1：概览压缩契约由引擎单点（短内容直用 / 超容量生成 + 长度守卫）
        let overview_content = ctx
            .summary_engine
            .generate_overview(&detail_content)
            .await?;

        // 先更新向量、后写摘要（T0-4 第二面）：摘要是“完成标记”（真实 VFS
        // 写入即刷新 updated_at，扫描据此判“已覆盖当前内容”）——若先写摘要
        // 而向量更新失败，下次扫描会判“已完成”，向量永久缺失（语义检索漏
        // 掉该条目）。反序后任一环节失败都不留“看似完成”的摘要，下一轮自
        // 动重建（向量 upsert 幂等，重复更新无害）。
        ctx.vfs
            .update_summary_vectors(uri, &abstract_content, &overview_content)
            .await?;

        // 写入摘要（提交点）
        ctx.vfs.write_abstract(uri, &abstract_content).await?;
        ctx.vfs.write_overview(uri, &overview_content).await?;

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
                return TaskResult::failed(crate::common::error::TianyanError::Custom(format!(
                    "摘要生成任务：扫描失败：{}",
                    e
                )));
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
    use std::sync::Arc;

    use crate::common::token_estimator::estimate_tokens;
    use crate::common::types::TokenUsage;
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::{ChatService, MockChatService};
    use crate::test_utils::MockVfs;
    use crate::vfs::SummaryEngine;

    /// 构造测试用的 ChatCompletionResponse。
    fn mock_chat_response(content: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: crate::common::types::Message::assistant(content),
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    /// 构造摘要任务测试上下文（MockVfs + 真实 SummaryEngine 包装 mock 聊天服务）。
    fn make_context(vfs: Arc<MockVfs>, chat: MockChatService) -> TaskContext {
        let chat: Arc<dyn ChatService> = Arc::new(chat);
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let config = Arc::new(crate::config::TianyanConfig::default());
        TaskContext::new(vfs, summary_engine, None, config)
    }

    /// 约 100 个汉字的中文文本（字节 < 400 且 token < 100）。
    fn short_chinese() -> String {
        "天演系统是一个基于大语言模型的本地智能代理系统。".repeat(4)
    }

    /// 约 300 个汉字的中文文本（字节 ≥ 400 且 token ≥ 100）。
    fn long_chinese() -> String {
        "天演系统是一个基于大语言模型的本地智能代理系统。".repeat(13)
    }

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

    // ── 短内容 / 长内容分支（CJK 字节 vs token 回归） ─────────────

    #[tokio::test]
    async fn test_short_chinese_uses_full_text_as_abstract() {
        // 约 100 个汉字（96 字 ≈ 288 字节，64 token）：字节数与 token 数都低于阈值，
        // 应走"短内容直接作摘要"路径
        let content = short_chinese();
        assert!(
            estimate_tokens(&content) < ABSTRACT_TOKEN_LIMIT,
            "前置：{} token 应低于阈值",
            estimate_tokens(&content)
        );

        // 不设置任何期望：若摘要引擎被调用，mockall 会直接 panic
        let chat = MockChatService::new();
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["short-cn".into()]);
        vfs.set_content(&uri, ContentLevel::Detail, &content);
        let ctx = make_context(vfs, chat);

        SummaryTask::new().process_uri(&ctx, &uri).await.unwrap();

        let abstract_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap();
        let overview_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Overview)
            .await
            .unwrap();
        assert_eq!(
            abstract_content, content,
            "短中文应全文直接作为 L0 Abstract"
        );
        assert_eq!(
            overview_content, content,
            "短中文应全文直接作为 L1 Overview"
        );
    }

    #[tokio::test]
    async fn test_long_chinese_calls_llm_summary() {
        // 约 300 个汉字（≈936 字节，≈208 token ≥ 100）→ 长路径：调用摘要引擎
        let content = long_chinese();
        assert!(
            content.len() >= 400,
            "前置：字节 {} 应超过旧字节阈值（约 133 汉字）",
            content.len()
        );
        assert!(
            estimate_tokens(&content) >= ABSTRACT_TOKEN_LIMIT,
            "前置：{} token 应达到阈值",
            estimate_tokens(&content)
        );

        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(mock_chat_response("模拟摘要内容")));
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["long-cn".into()]);
        vfs.set_content(&uri, ContentLevel::Detail, &content);
        let ctx = make_context(vfs, chat);

        SummaryTask::new().process_uri(&ctx, &uri).await.unwrap();

        let abstract_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap();
        let overview_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Overview)
            .await
            .unwrap();
        assert_eq!(abstract_content, "模拟摘要内容", "长中文应调用 LLM 生成 L0");
        assert_eq!(
            overview_content, content,
            "输入未超过概览容量时 L1 直用原文（不调 LLM）"
        );
        assert_ne!(abstract_content, content, "长中文摘要不应等于全文");
    }

    #[tokio::test]
    async fn test_long_chinese_does_not_write_full_text_to_l0() {
        // 长中文走 LLM 摘要路径：L0 Abstract 必须是摘要输出，不得直接写入全文
        let content = long_chinese();
        assert!(estimate_tokens(&content) >= ABSTRACT_TOKEN_LIMIT);

        let mut chat = MockChatService::new();
        chat.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(mock_chat_response("生成摘要结果")));
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["long-cn-no-full".into()]);
        vfs.set_content(&uri, ContentLevel::Detail, &content);
        let ctx = make_context(vfs, chat);

        SummaryTask::new().process_uri(&ctx, &uri).await.unwrap();

        let abstract_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap();
        assert_ne!(
            abstract_content, content,
            "长中文不得把全文直接写入 L0 Abstract"
        );
        assert_eq!(abstract_content, "生成摘要结果");
    }

    #[tokio::test]
    async fn test_cjk_bytes_over_limit_but_tokens_under_limit_stays_short() {
        // 回归：约 144 个汉字（432 字节 ≥ 400 旧阈值）但仅 96 token < 100 ——
        // 旧实现按字节数判断会误走 LLM 摘要路径；正确行为是仍走短路径
        let content = "天演系统是一个基于大语言模型的本地智能代理系统。".repeat(6);
        assert!(
            content.len() >= 400,
            "前置：字节 {} 必须超过旧字节阈值（触发字节误判）",
            content.len()
        );
        assert!(
            estimate_tokens(&content) < ABSTRACT_TOKEN_LIMIT,
            "前置：{} token 必须低于阈值（应走短路径）",
            estimate_tokens(&content)
        );

        // 短路径（token 低于阈值）：L0/L1 均直用原文，不得调用 LLM
        let mut chat = MockChatService::new();
        chat.expect_chat_completion().times(0);
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["boundary-cn".into()]);
        vfs.set_content(&uri, ContentLevel::Detail, &content);
        let ctx = make_context(vfs, chat);

        SummaryTask::new().process_uri(&ctx, &uri).await.unwrap();

        let abstract_content = ctx
            .vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap();
        assert_eq!(
            abstract_content, content,
            "token 低于阈值的中文内容不应被 LLM 摘要（字节/token 误判 bug）"
        );
    }

    // ── 运维子域排除（状态/日志类不参与摘要） ───────────────────────

    #[tokio::test]
    async fn test_operational_paths_excluded_from_summary_scan() {
        // evolution_reports（运维日志）不参与摘要扫描：即便缺少 L1，
        // 也不进入 missing 队列；普通 cases 条目正常进入（判别基准）
        let vfs = Arc::new(MockVfs::new());
        let mem_root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let events = mem_root.append("events");
        let reports = events.append("evolution_reports");
        let cases = mem_root.append("cases");
        vfs.add_directory(&mem_root, &events);
        vfs.add_directory(&events, &reports);
        vfs.add_directory(&mem_root, &cases);

        let report = reports.append("r1.md");
        let case = cases.append("c1");
        vfs.add_entry(&reports, &report);
        vfs.add_entry(&cases, &case);
        vfs.add_content_metadata(&report, ContentLevel::Detail);
        vfs.add_content_metadata(&case, ContentLevel::Detail);

        let ctx = make_context(vfs.clone(), MockChatService::new());
        let missing = SummaryTask::new()
            .scan_missing_summaries(&ctx)
            .await
            .unwrap();

        assert!(
            missing.iter().any(|u| u.as_str().contains("/cases/")),
            "普通记忆条目应进入 missing（判别基准）：{missing:?}"
        );
        assert!(
            missing
                .iter()
                .all(|u| !u.as_str().contains("evolution_reports")),
            "运维子域不得进入 missing：{missing:?}"
        );
    }

    /// T0-4 主回归：内容更新（Detail 比摘要新）必须触发摘要/向量重建——
    /// 修复前 `processed` 缓存短路在时间戳检测之前：条目一旦处理过，即使
    /// 内容被编辑也不会重新摘要（摘要与向量长期陈旧，直到缓存 FIFO 淘汰
    /// 或进程重启）。
    #[tokio::test]
    async fn test_content_update_rebuilds_despite_processed_cache() {
        let vfs = Arc::new(MockVfs::new());
        let root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = root.append("cases");
        vfs.add_directory(&root, &cases);
        let uri = cases.append("c1");
        vfs.add_entry(&cases, &uri);

        // 摘要生成于 2 小时前；内容 1 分钟前被编辑（Detail 更新）
        let summary_time = chrono::Utc::now() - chrono::Duration::hours(2);
        let edited_time = chrono::Utc::now() - chrono::Duration::minutes(1);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Abstract, summary_time);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Overview, summary_time);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Detail, edited_time);

        let ctx = make_context(vfs.clone(), MockChatService::new());
        let task = SummaryTask::new();
        // 模拟该条目此前已处理（缓存命中）
        task.record_processed(uri.as_str()).await;

        let missing = task.scan_missing_summaries(&ctx).await.unwrap();

        assert!(
            missing.iter().any(|u| u.as_str() == uri.as_str()),
            "内容更新后必须重新摘要（不得被 processed 缓存短路）：{missing:?}"
        );
    }

    /// 回归：内容未变（摘要比 Detail 新）→ 不重复摘要（摘要齐全且未更新
    /// 不调用 LLM）。
    #[tokio::test]
    async fn test_unchanged_content_not_reprocessed() {
        let vfs = Arc::new(MockVfs::new());
        let root = TianyanUri::new(ContextNamespace::Memory, vec![]);
        let cases = root.append("cases");
        vfs.add_directory(&root, &cases);
        let uri = cases.append("c1");
        vfs.add_entry(&cases, &uri);

        // Detail 1 小时前；摘要 10 分钟前（比 Detail 新 → 已覆盖当前内容）
        let detail_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let summary_time = chrono::Utc::now() - chrono::Duration::minutes(10);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Detail, detail_time);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Abstract, summary_time);
        vfs.add_content_metadata_with_updated_at(&uri, ContentLevel::Overview, summary_time);

        let ctx = make_context(vfs.clone(), MockChatService::new());
        let task = SummaryTask::new();

        let missing = task.scan_missing_summaries(&ctx).await.unwrap();
        assert!(
            !missing.iter().any(|u| u.as_str() == uri.as_str()),
            "内容未变不应重复摘要：{missing:?}"
        );
    }

    /// T0-4（第二面）：向量更新失败不得让摘要内容落盘——
    /// 修复前顺序 `write_abstract → write_overview → update_summary_vectors`：
    /// 向量失败时摘要已写入（真实 VFS 下同时刷新 updated_at），下次扫描判
    /// “已完成”→ 向量永久缺失（语义检索漏掉该条目）。修复后顺序为
    /// 「先向量、后摘要（提交点）」，任一环节失败都不留“看似完成”的摘要。
    #[tokio::test]
    async fn test_vector_failure_does_not_persist_summary() {
        let content = short_chinese();
        let vfs = Arc::new(MockVfs::new());
        let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["vec-fail".into()]);
        vfs.set_content(&uri, ContentLevel::Detail, &content);
        vfs.set_summary_vector_error(Some("vector store down".to_string()));

        let ctx = make_context(vfs.clone(), MockChatService::new());

        assert!(
            SummaryTask::new().process_uri(&ctx, &uri).await.is_err(),
            "向量更新失败应上抛"
        );

        let has_abstract = ctx
            .vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .is_ok();
        let has_overview = ctx
            .vfs
            .read_content(&uri, ContentLevel::Overview)
            .await
            .is_ok();
        assert!(
            !has_abstract && !has_overview,
            "向量失败时摘要不得落盘（否则下次扫描判已完成，向量永不重建）"
        );
    }
}
