//! 双层检索器实现。
//!
//! 本模块实现基于多向量融合的检索系统：
//! - 使用 RRF (Reciprocal Rank Fusion) 算法融合 abstract 和 overview 向量搜索结果

use std::sync::Arc;

use tracing::{debug, info, instrument};

use crate::observability::usage_stats::UsageStats;

use super::types::RetrievalResult;
use crate::common::error::Result;
use crate::common::types::ContentLevel;
use crate::context::compression::estimate_tokens;
use crate::vfs::VirtualFileSystem;

use super::intent::{Intent, IntentAnalyzer};
use super::loader::ContentLoadStrategy;
use super::trace::RetrievalTraceBuilder;

/// 组合评分（新鲜度恒为 1.0——`source_updated_at` 无数据源，字段已移除，
/// 常数项内联，与旧行为逐位一致）。
const DEFAULT_FRESHNESS_WEIGHT: f32 = 0.2;

fn compute_combined_score(semantic_score: f32) -> f32 {
    semantic_score * (1.0 - DEFAULT_FRESHNESS_WEIGHT) + DEFAULT_FRESHNESS_WEIGHT
}

/// 用于基于向量的内容检索的融合检索器。
///
/// 此检索器使用 RRF 算法融合多个命名向量的搜索结果，
/// 提供更准确和全面的检索能力。
pub struct DualLayerRetriever {
    /// VFS 实例，提供搜索和内容加载。
    vfs: Arc<dyn VirtualFileSystem>,
    /// 意图分析器。
    intent_analyzer: IntentAnalyzer,
    /// Memory 命名空间的分数偏置倍数。
    memory_bias: f32,
    /// 使用统计追踪器。
    usage_stats: Option<Arc<UsageStats>>,
}

impl DualLayerRetriever {
    /// 创建新的融合检索器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self {
            vfs,
            intent_analyzer: IntentAnalyzer::new(),
            memory_bias: 1.15,
            usage_stats: None,
        }
    }

    /// 设置 Memory 命名空间的分数偏置倍数。
    pub fn with_memory_bias(mut self, bias: f32) -> Self {
        self.memory_bias = bias;
        self
    }

    /// 设置使用统计追踪器。
    pub fn with_usage_stats(mut self, stats: Arc<UsageStats>) -> Self {
        self.usage_stats = Some(stats);
        self
    }

    /// 检索查询的内容。
    #[instrument(skip(self), fields(query = %query))]
    pub async fn retrieve(&self, query: &str, top_k: usize) -> Result<Vec<RetrievalResult>> {
        let mut trace_builder = RetrievalTraceBuilder::new(query);

        // 步骤 1：意图分析。
        let intent = self.analyze_intent(query).await?;
        trace_builder.add_intent_analysis(intent.token_count);

        // 步骤 2：融合搜索（VFS 内部处理嵌入）。
        let results = self
            .fused_search(&intent.original_query, top_k, &intent)
            .await?;
        debug!("Fused search returned {} results", results.len());

        // Record doc hits and search query in usage stats
        if let Some(ref stats) = self.usage_stats {
            for result in &results {
                stats.record_doc_hit(result.uri.as_str(), result.score);
            }
            let ns = results.first().map(|r| r.uri.namespace().to_string());
            stats.record_search_query(query, results.len(), ns.as_deref());
        }

        for result in &results {
            trace_builder.add_l1_search(result.uri.clone(), result.score, 0);
        }

        // 步骤 3：加载内容。
        let results = self.load_content_for_results(results).await?;
        for result in &results {
            if result.has_content() {
                trace_builder.add_content_load(result.uri.clone(), result.token_count);
            }
            // 记录最终命中的结果 URI（轨迹黑匣子的结果清单）。
            trace_builder.add_result(result.uri.clone());
        }

        // Record doc loads in usage stats
        if let Some(ref stats) = self.usage_stats {
            for result in &results {
                if result.has_content() {
                    stats.record_doc_load(result.uri.as_str());
                }
            }
        }

        // 构建最终追踪记录。
        let trace = trace_builder.build();

        // 持久化轨迹（完整过程快照，供调试界面回溯"为什么检索成这样"）。
        if let Some(ref stats) = self.usage_stats {
            stats.record_retrieval_trace(&trace);
        }

        info!(
            query = %query,
            total_time_ms = trace.total_time_ms,
            total_tokens = trace.total_tokens,
            result_count = results.len(),
            "Retrieval completed"
        );

        Ok(results)
    }

    /// 分析查询意图，失败时返回默认意图（无 namespace 过滤的 Search 类型）。
    async fn analyze_intent(&self, query: &str) -> Result<Intent> {
        match self.intent_analyzer.analyze(query).await {
            Ok(intent) => Ok(intent),
            Err(e) => {
                tracing::warn!(error = %e, query = %query, "意图分析失败，使用默认意图回退");
                Ok(Intent::new(query))
            }
        }
    }

    /// 执行融合搜索。
    ///
    /// VFS 内部负责文本嵌入和 RRF 融合。
    async fn fused_search(
        &self,
        query: &str,
        top_k: usize,
        intent: &Intent,
    ) -> Result<Vec<RetrievalResult>> {
        let namespace = intent.target_scope.as_ref().map(|s| s.category_enum());
        let results = self.vfs.search(query, top_k, namespace).await?;

        let mut results: Vec<RetrievalResult> = results
            .into_iter()
            .map(|sr| RetrievalResult::new(sr.uri, sr.score))
            .collect();

        // Memory namespace 结果获得分数偏置，确保记忆优先于通用知识
        for result in &mut results {
            if result.uri.namespace().to_string() == "memory" {
                result.score *= self.memory_bias;
            }
        }

        // 将新鲜度评分融入最终分数（VFS 搜索结果不含时间戳，默认为 1.0）
        for result in &mut results {
            result.score = compute_combined_score(result.score);
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(top_k);
        Ok(results)
    }

    /// 为结果加载内容。
    async fn load_content_for_results(
        &self,
        results: Vec<RetrievalResult>,
    ) -> Result<Vec<RetrievalResult>> {
        let mut loaded_results = Vec::with_capacity(results.len());

        for mut result in results {
            let strategy = ContentLoadStrategy::from_score(result.score);
            let level = strategy.to_content_level();

            match self.vfs.read(&result.uri, level).await {
                Ok(content) => {
                    result.content = Some(content.clone());
                    result.content_level = level;
                    result.token_count = estimate_tokens(&content);
                }
                Err(e) => {
                    tracing::warn!(uri = %result.uri, error = %e, "加载内容失败");
                    if level != ContentLevel::Abstract {
                        if let Ok(content) =
                            self.vfs.read(&result.uri, ContentLevel::Abstract).await
                        {
                            result.content = Some(content.clone());
                            result.content_level = ContentLevel::Abstract;
                            result.token_count = estimate_tokens(&content);
                        }
                    }
                }
            }

            loaded_results.push(result);
        }

        Ok(loaded_results)
    }

    /// 使用命名空间过滤器检索内容。
    ///
    /// 先执行标准检索，然后过滤结果只保留满足条件的命名空间。
    ///
    /// - `query` - 查询内容
    /// - `top_k` - 返回数量限制
    /// - `filter` - 命名空间过滤函数
    pub async fn retrieve_by_namespace(
        &self,
        query: &str,
        top_k: usize,
        namespace: crate::common::types::ContextNamespace,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieve_with_namespace_filter(query, top_k, |ns| ns == namespace)
            .await
    }

    /// 使用命名空间过滤器检索内容。
    pub async fn retrieve_with_namespace_filter<F>(
        &self,
        query: &str,
        top_k: usize,
        filter: F,
    ) -> Result<Vec<RetrievalResult>>
    where
        F: Fn(crate::common::types::ContextNamespace) -> bool,
    {
        let results = self.retrieve(query, top_k * 2).await?;
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| filter(r.uri.namespace()))
            .take(top_k)
            .collect();
        Ok(filtered)
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "retriever_tests.rs"]
mod tests;
