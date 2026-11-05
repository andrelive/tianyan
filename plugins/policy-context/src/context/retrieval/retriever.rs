//! 双层检索器实现。
//!
//! 本模块实现基于多向量融合的检索系统：
//! - 使用 RRF (Reciprocal Rank Fusion) 算法融合 abstract 和 overview 向量搜索结果

use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, instrument};

use tianyan::observability::usage_stats::UsageStats;

use super::types::RetrievalResult;
use crate::context::compression::estimate_tokens;
use tianyan::common::error::{Result, TianyanError};
use tianyan::common::types::{system_paths, ContentLevel, TianyanUri};
use tianyan::vfs::VirtualFileSystem;

use super::intent::{Intent, IntentAnalyzer};
use super::loader::ContentLoadStrategy;

/// 组合评分（新鲜度恒为 1.0——`source_updated_at` 无数据源，字段已移除，
/// 常数项内联，与旧行为逐位一致）。
const DEFAULT_FRESHNESS_WEIGHT: f32 = 0.2;

fn compute_combined_score(semantic_score: f32) -> f32 {
    semantic_score * (1.0 - DEFAULT_FRESHNESS_WEIGHT) + DEFAULT_FRESHNESS_WEIGHT
}

/// 语义相似度下限：低于此值的候选**不注入**。
///
/// 依据（2026-11 实测 `doc_access` 的 207 批注入记录）：注入批次固定是「每池满
/// 5 条」，与相关性无关——73% 的批次最低分 < 0.40，其中 4 批的**最高分**也只有
/// 0.10~0.27（纯噪音照样进前缀，既占 token 也污染前缀缓存）。0.35 是「弱相关」
/// 的经验分界：近期正常批次的最低分折算后是 0.45+，历史噪音批次是 0.04~0.09。
pub const MIN_SEMANTIC_SCORE: f32 = 0.35;

/// 语义下限换算成**组合分**（与 [`compute_combined_score`] 同口径）。
///
/// 门槛必须作用在组合分上：`doc_access` 记录的、排序与 `ContentLoadStrategy`
/// 判级用的都是组合分，混用两套口径会让门槛值不可解释。
pub fn min_combined_score() -> f32 {
    compute_combined_score(MIN_SEMANTIC_SCORE)
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
        namespace: Option<tianyan::common::types::ContextNamespace>,
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
        if tianyan::vfs::parse_doc_index(l1).is_none() {
            return l1.to_string();
        }
        let abstract_text = self
            .vfs
            .read(uri, ContentLevel::Abstract)
            .await
            .unwrap_or_default();
        super::loader::compose_overview_parts(&abstract_text, l1)
    }

    /// 跨多个命名空间检索，**合并后统一排序与截断**。
    ///
    /// 与 [`Self::retrieve_by_namespace`] 的两点区别：
    ///
    /// 1. **两个池子的分数从此可比**：`memory_bias` 与组合分在合并后统一施加，
    ///    排序只看分数——不再是「每池各取 top_k，谁也看不见谁」（旧路径每池必然
    ///    满 `top_k` 条，无论那个池最高分是 0.7 还是 0.1）；
    /// 2. **不凑数**：低于 `min_score` 的候选直接丢弃（依据见 [`MIN_SEMANTIC_SCORE`]）。
    ///
    /// 每个命名空间仍然**下推**到 VFS 检索（ADR-001 铁律②：取回全局 top-k 再在
    /// 内存里按命名空间筛，会让目标命名空间被别的池子挤空）。
    ///
    /// 使用统计只记**一条** query：旧路径每池各记一条，`daily_search_queries` 里
    /// 同一句话同秒出现两条（实测一次输入对应 `id=574/575` 两条记录）。
    pub async fn retrieve_merged(
        &self,
        query: &str,
        namespaces: &[tianyan::common::types::ContextNamespace],
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<RetrievalResult>> {
        let started = Instant::now();

        // 逐命名空间下推检索，候选汇总到一个池子里（单个池失败只跳过该池）
        let mut merged: Vec<RetrievalResult> = Vec::new();
        let mut succeeded = 0usize;
        let mut last_error: Option<TianyanError> = None;
        for ns in namespaces {
            match self.vfs.search(query, top_k, Some(*ns)).await {
                Ok(hits) => {
                    succeeded += 1;
                    for sr in hits {
                        if system_paths::is_system_path(&sr.uri) {
                            continue;
                        }
                        let mut result = RetrievalResult::new(sr.uri, sr.score);
                        // Memory 偏置与组合分在**合并后**统一施加，保证两池同口径
                        if result.uri.namespace().to_string() == "memory" {
                            result.score *= self.memory_bias;
                        }
                        result.score = compute_combined_score(result.score);
                        merged.push(result);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, namespace = %ns, "命名空间检索失败（跳过该池）");
                    last_error = Some(e);
                }
            }
        }

        // 全部命名空间都失败 -> 上抛，让调用方走既有回退路径（全量规则列表）。
        // 只要有一个池成功就继续——部分池失败不该让整段上下文缺失。
        if succeeded == 0 && !namespaces.is_empty() {
            return Err(last_error.unwrap_or_else(|| {
                TianyanError::Custom("retrieval: 所有命名空间检索均失败".to_string())
            }));
        }

        let candidates = merged.len();
        merged.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // 同一 uri 可能被多个池子返回（底层实现若未真正下推 namespace 过滤就会这样）——
        // 降序排序后相邻去重，保留分数最高那份，避免同一内容占两个名额。
        merged.dedup_by(|a, b| a.uri.as_str() == b.uri.as_str());
        merged.retain(|r| r.score >= min_score);
        merged.truncate(top_k);

        if let Some(ref stats) = self.usage_stats {
            for result in &merged {
                stats.record_doc_hit(result.uri.as_str(), result.score);
            }
            let ns = merged.first().map(|r| r.uri.namespace().to_string());
            stats
                .record_search_query(query, merged.len(), ns.as_deref())
                .await;
        }

        let merged = self.load_content_for_results(merged).await?;
        if let Some(ref stats) = self.usage_stats {
            for result in &merged {
                if result.has_content() {
                    stats.record_doc_load(result.uri.as_str());
                }
            }
        }

        info!(
            query = %query,
            candidates,
            kept = merged.len(),
            min_score,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Merged retrieval completed"
        );
        Ok(merged)
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "retriever_tests.rs"]
mod tests;
