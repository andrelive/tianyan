//! 双层检索器实现。
//!
//! 本模块实现基于多向量融合的检索系统：
//! - 使用 RRF (Reciprocal Rank Fusion) 算法融合 abstract 和 overview 向量搜索结果

use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, instrument};

use crate::observability::usage_stats::UsageStats;

use super::types::RetrievalResult;
use crate::common::error::Result;
use crate::common::types::{system_paths, ContentLevel, TianyanUri};
use crate::context::compression::estimate_tokens;
use crate::vfs::VirtualFileSystem;

use super::intent::{Intent, IntentAnalyzer};
use super::loader::ContentLoadStrategy;

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
        let started = Instant::now();

        // 步骤 1：意图分析。
        let intent = self.analyze_intent(query).await?;

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
            stats
                .record_search_query(query, results.len(), ns.as_deref())
                .await;
        }

        // 步骤 3：加载内容。
        let results = self.load_content_for_results(results).await?;

        // Record doc loads in usage stats
        if let Some(ref stats) = self.usage_stats {
            for result in &results {
                if result.has_content() {
                    stats.record_doc_load(result.uri.as_str());
                }
            }
        }

        info!(
            query = %query,
            elapsed_ms = started.elapsed().as_millis() as u64,
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
        self.fused_search_in_namespace(query, top_k, namespace)
            .await
    }

    /// 执行融合搜索（可显式指定 namespace）。
    ///
    /// `namespace = None` 时不施加过滤（VFS 全量检索）；显式 `Some(ns)` 时
    /// **直接下推**到 VFS 搜索——显式 namespace 检索不经过意图推断（T1-4）。
    async fn fused_search_in_namespace(
        &self,
        query: &str,
        top_k: usize,
        namespace: Option<crate::common::types::ContextNamespace>,
    ) -> Result<Vec<RetrievalResult>> {
        let results = self.vfs.search(query, top_k, namespace).await?;

        // 系统/运维路径不作为检索结果：归档/评审/运维子域不得复活注入上下文
        // （存量向量兜底过滤——新增由摘要与索引采集侧拦截）
        let mut results: Vec<RetrievalResult> = results
            .into_iter()
            .filter(|sr| !system_paths::is_system_path(&sr.uri))
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
    ///
    /// ADR-001 修订（2026-09-22）：命中即"简介（+目录）"——`Overview` 策略加载
    /// L1，若其为结构化目录（`parse_doc_index`）则前置 L0 简介并渲染目录
    /// （模型可据此定位章节、按需取段）；非目录（全文直用/旧概览）原样返回。
    /// **L2 不自动加载**（见 `ContentLoadStrategy`）。
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
                    let content = if level == ContentLevel::Overview {
                        self.compose_overview(&result.uri, &content).await
                    } else {
                        content
                    };
                    result.token_count = estimate_tokens(&content);
                    result.content = Some(content);
                    result.content_level = level;
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
    /// 组装 L1 的消费形态：结构化目录 → "L0 简介 + 渲染目录"；非目录（全文
    /// 直用/旧概览）→ 原样返回（不加 L0，避免前缀膨胀——全文已含内容）。
    async fn compose_overview(&self, uri: &TianyanUri, l1: &str) -> String {
        // 非目录：直接返回（不读 L0——避免无谓读取）
        if crate::vfs::parse_doc_index(l1).is_none() {
            return l1.to_string();
        }
        let abstract_text = self
            .vfs
            .read(uri, ContentLevel::Abstract)
            .await
            .unwrap_or_default();
        super::loader::compose_overview_parts(&abstract_text, l1)
    }

    /// 按**显式** namespace 检索内容（不受意图推断影响）。
    ///
    /// 与 [`Self::retrieve`] 的区别：namespace 直接**下推**到 VFS 搜索——
    /// 显式指定检索范围时不得先经过意图分析再在结果上过滤（T1-4：意图可能
    /// 把搜索范围限制到其它 namespace，随后过滤 → rules/memories 静默为空）。
    ///
    /// - `query` - 查询内容
    /// - `top_k` - 返回数量限制
    /// - `namespace` - 目标命名空间（下推 VFS category 过滤）
    pub async fn retrieve_by_namespace(
        &self,
        query: &str,
        top_k: usize,
        namespace: crate::common::types::ContextNamespace,
    ) -> Result<Vec<RetrievalResult>> {
        let started = Instant::now();
        // 直接按 namespace 搜索（不经意图分析）
        let results = self
            .fused_search_in_namespace(query, top_k, Some(namespace))
            .await?;
        // 使用统计（与 retrieve 路径同口径：doc 命中 / 查询 / 内容加载）
        if let Some(ref stats) = self.usage_stats {
            for result in &results {
                stats.record_doc_hit(result.uri.as_str(), result.score);
            }
            let ns = results.first().map(|r| r.uri.namespace().to_string());
            stats
                .record_search_query(query, results.len(), ns.as_deref())
                .await;
        }
        let results = self.load_content_for_results(results).await?;
        if let Some(ref stats) = self.usage_stats {
            for result in &results {
                if result.has_content() {
                    stats.record_doc_load(result.uri.as_str());
                }
            }
        }
        info!(
            query = %query,
            namespace = %namespace,
            elapsed_ms = started.elapsed().as_millis() as u64,
            result_count = results.len(),
            "Namespace retrieval completed"
        );
        Ok(results)
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "retriever_tests.rs"]
mod tests;
