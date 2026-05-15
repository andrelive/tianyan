//! 检索追踪模块。
//!
//! 本模块实现用于调试和优化的检索追踪记录。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::common::types::TianyanUri;

// 从 types.rs 重新导出以保持向后兼容。
pub use crate::context::types::{RetrievalStep, RetrievalStepType, RetrievalTrace};

/// 用于创建检索追踪记录的构建器。
#[derive(Debug)]
pub struct RetrievalTraceBuilder {
    query: String,
    steps: Vec<RetrievalStep>,
    results: Vec<TianyanUri>,
    total_tokens: usize,
    start_time: Instant,
}

impl RetrievalTraceBuilder {
    /// 创建新的追踪构建器。
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            steps: Vec::new(),
            results: Vec::new(),
            total_tokens: 0,
            start_time: Instant::now(),
        }
    }

    /// 添加意图分析步骤。
    pub fn add_intent_analysis(&mut self, tokens: usize) {
        self.add_step(RetrievalStep {
            step_type: RetrievalStepType::IntentAnalysis,
            target_uri: TianyanUri::new(crate::common::types::ContextNamespace::User, vec![]),
            score: None,
            tokens_used: tokens,
            timestamp: Utc::now(),
        });
    }

    /// 添加 L0 搜索步骤。
    pub fn add_l0_search(&mut self, target_uri: TianyanUri, score: f32, tokens: usize) {
        self.add_step(RetrievalStep {
            step_type: RetrievalStepType::L0Search,
            target_uri,
            score: Some(score),
            tokens_used: tokens,
            timestamp: Utc::now(),
        });
    }

    /// 添加 L1 搜索步骤。
    pub fn add_l1_search(&mut self, target_uri: TianyanUri, score: f32, tokens: usize) {
        self.add_step(RetrievalStep {
            step_type: RetrievalStepType::L1Search,
            target_uri,
            score: Some(score),
            tokens_used: tokens,
            timestamp: Utc::now(),
        });
    }

    /// 添加内容加载步骤。
    pub fn add_content_load(&mut self, target_uri: TianyanUri, tokens: usize) {
        self.add_step(RetrievalStep {
            step_type: RetrievalStepType::ContentLoad,
            target_uri,
            score: None,
            tokens_used: tokens,
            timestamp: Utc::now(),
        });
    }

    /// 添加聚合步骤。
    pub fn add_aggregation(&mut self, tokens: usize) {
        self.add_step(RetrievalStep {
            step_type: RetrievalStepType::Aggregation,
            target_uri: TianyanUri::new(crate::common::types::ContextNamespace::User, vec![]),
            score: None,
            tokens_used: tokens,
            timestamp: Utc::now(),
        });
    }

    /// 添加自定义步骤。
    pub fn add_step(&mut self, step: RetrievalStep) {
        self.total_tokens += step.tokens_used;
        self.steps.push(step);
    }

    /// 添加结果 URI。
    pub fn add_result(&mut self, uri: TianyanUri) {
        self.results.push(uri);
    }

    /// 添加多个结果 URI。
    pub fn add_results(&mut self, uris: Vec<TianyanUri>) {
        self.results.extend(uris);
    }

    /// 构建最终追踪记录。
    pub fn build(self) -> RetrievalTrace {
        RetrievalTrace {
            query: self.query,
            steps: self.steps,
            results: self.results,
            total_tokens: self.total_tokens,
            total_time_ms: self.start_time.elapsed().as_millis() as u64,
            timestamp: Utc::now(),
        }
    }
}

impl RetrievalTrace {
    /// 为此追踪记录创建构建器。
    pub fn builder(query: impl Into<String>) -> RetrievalTraceBuilder {
        RetrievalTraceBuilder::new(query)
    }

    /// 获取步骤数量。
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// 获取结果数量。
    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// 按类型获取步骤。
    pub fn steps_by_type(&self, step_type: RetrievalStepType) -> Vec<&RetrievalStep> {
        self.steps
            .iter()
            .filter(|s| s.step_type == step_type)
            .collect()
    }

    /// 获取所有有分数步骤的平均分数。
    pub fn average_score(&self) -> Option<f32> {
        let scores: Vec<f32> = self.steps.iter().filter_map(|s| s.score).collect();
        if scores.is_empty() {
            return None;
        }
        Some(scores.iter().sum::<f32>() / scores.len() as f32)
    }

    /// 格式化为可读字符串。
    pub fn format_summary(&self) -> String {
        let mut summary = format!("Retrieval Trace for: '{}'\n", self.query);
        summary.push_str(&format!("  Total time: {}ms\n", self.total_time_ms));
        summary.push_str(&format!("  Total tokens: {}\n", self.total_tokens));
        summary.push_str(&format!("  Steps: {}\n", self.steps.len()));
        summary.push_str(&format!("  Results: {}\n", self.results.len()));

        if let Some(avg_score) = self.average_score() {
            summary.push_str(&format!("  Average score: {:.3}\n", avg_score));
        }

        summary.push_str("\nSteps:\n");
        for (i, step) in self.steps.iter().enumerate() {
            summary.push_str(&format!(
                "  {}. {:?} -> {} (tokens: {}",
                i + 1,
                step.step_type,
                step.target_uri,
                step.tokens_used
            ));
            if let Some(score) = step.score {
                summary.push_str(&format!(", score: {:.3}", score));
            }
            summary.push_str(")\n");
        }

        summary
    }
}

impl RetrievalStep {
    /// 创建新的检索步骤。
    pub fn new(
        step_type: RetrievalStepType,
        target_uri: TianyanUri,
        score: Option<f32>,
        tokens_used: usize,
    ) -> Self {
        Self {
            step_type,
            target_uri,
            score,
            tokens_used,
            timestamp: Utc::now(),
        }
    }

    /// 创建意图分析步骤。
    pub fn intent_analysis(tokens: usize) -> Self {
        Self::new(
            RetrievalStepType::IntentAnalysis,
            TianyanUri::new(crate::common::types::ContextNamespace::User, vec![]),
            None,
            tokens,
        )
    }

    /// 创建 L0 搜索步骤。
    pub fn l0_search(uri: TianyanUri, score: f32, tokens: usize) -> Self {
        Self::new(RetrievalStepType::L0Search, uri, Some(score), tokens)
    }

    /// 创建 L1 搜索步骤。
    pub fn l1_search(uri: TianyanUri, score: f32, tokens: usize) -> Self {
        Self::new(RetrievalStepType::L1Search, uri, Some(score), tokens)
    }

    /// 创建内容加载步骤。
    pub fn content_load(uri: TianyanUri, tokens: usize) -> Self {
        Self::new(RetrievalStepType::ContentLoad, uri, None, tokens)
    }

    /// 创建聚合步骤。
    pub fn aggregation(tokens: usize) -> Self {
        Self::new(
            RetrievalStepType::Aggregation,
            TianyanUri::new(crate::common::types::ContextNamespace::User, vec![]),
            None,
            tokens,
        )
    }
}

/// Token 消耗统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStats {
    /// 意图分析使用的 Token。
    pub intent_analysis_tokens: usize,
    /// L0 搜索使用的 Token。
    pub l0_search_tokens: usize,
    /// L1 搜索使用的 Token。
    pub l1_search_tokens: usize,
    /// 内容加载使用的 Token。
    pub content_load_tokens: usize,
    /// 聚合使用的 Token。
    pub aggregation_tokens: usize,
    /// 总 Token 数量。
    pub total_tokens: usize,
}

impl TokenStats {
    /// 创建新的 Token 统计。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从追踪记录更新统计。
    pub fn from_trace(trace: &RetrievalTrace) -> Self {
        let mut stats = Self::new();
        for step in &trace.steps {
            match step.step_type {
                RetrievalStepType::IntentAnalysis => {
                    stats.intent_analysis_tokens += step.tokens_used;
                }
                RetrievalStepType::L0Search => {
                    stats.l0_search_tokens += step.tokens_used;
                }
                RetrievalStepType::L1Search => {
                    stats.l1_search_tokens += step.tokens_used;
                }
                RetrievalStepType::ContentLoad => {
                    stats.content_load_tokens += step.tokens_used;
                }
                RetrievalStepType::Aggregation => {
                    stats.aggregation_tokens += step.tokens_used;
                }
            }
        }
        stats.total_tokens = trace.total_tokens;
        stats
    }

    /// 获取百分比分布。
    pub fn percentages(&self) -> TokenPercentages {
        let total = self.total_tokens as f32;
        if total == 0.0 {
            return TokenPercentages::default();
        }

        TokenPercentages {
            intent_analysis: (self.intent_analysis_tokens as f32 / total) * 100.0,
            l0_search: (self.l0_search_tokens as f32 / total) * 100.0,
            l1_search: (self.l1_search_tokens as f32 / total) * 100.0,
            content_load: (self.content_load_tokens as f32 / total) * 100.0,
            aggregation: (self.aggregation_tokens as f32 / total) * 100.0,
        }
    }
}

/// Token 消耗百分比。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenPercentages {
    pub intent_analysis: f32,
    pub l0_search: f32,
    pub l1_search: f32,
    pub content_load: f32,
    pub aggregation: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;

    #[test]
    fn test_retrieval_trace_builder() {
        let mut builder = RetrievalTraceBuilder::new("test query");
        builder.add_intent_analysis(10);
        builder.add_l0_search(
            TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]),
            0.9,
            50,
        );
        builder.add_result(TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["doc".to_string()],
        ));

        let trace = builder.build();
        assert_eq!(trace.steps.len(), 2);
        assert_eq!(trace.results.len(), 1);
        assert_eq!(trace.total_tokens, 60);
    }

    #[test]
    fn test_retrieval_step_creation() {
        let step = RetrievalStep::l0_search(
            TianyanUri::new(ContextNamespace::User, vec!["profile".to_string()]),
            0.85,
            100,
        );

        assert_eq!(step.step_type, RetrievalStepType::L0Search);
        assert_eq!(step.score, Some(0.85));
        assert_eq!(step.tokens_used, 100);
    }

    #[test]
    fn test_retrieval_trace_summary() {
        let trace = RetrievalTrace {
            query: "test".to_string(),
            steps: vec![
                RetrievalStep::intent_analysis(10),
                RetrievalStep::l0_search(
                    TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]),
                    0.9,
                    50,
                ),
            ],
            results: vec![TianyanUri::new(
                ContextNamespace::Knowledge,
                vec!["doc".to_string()],
            )],
            total_tokens: 60,
            total_time_ms: 100,
            timestamp: Utc::now(),
        };

        let summary = trace.format_summary();
        assert!(summary.contains("test"));
        assert!(summary.contains("100ms"));
        assert!(summary.contains("60"));
    }

    #[test]
    fn test_token_stats() {
        let trace = RetrievalTrace {
            query: "test".to_string(),
            steps: vec![
                RetrievalStep::intent_analysis(10),
                RetrievalStep::l0_search(
                    TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]),
                    0.9,
                    50,
                ),
                RetrievalStep::content_load(
                    TianyanUri::new(ContextNamespace::Knowledge, vec!["doc".to_string()]),
                    200,
                ),
            ],
            results: vec![],
            total_tokens: 260,
            total_time_ms: 50,
            timestamp: Utc::now(),
        };

        let stats = TokenStats::from_trace(&trace);
        assert_eq!(stats.intent_analysis_tokens, 10);
        assert_eq!(stats.l0_search_tokens, 50);
        assert_eq!(stats.content_load_tokens, 200);
        assert_eq!(stats.total_tokens, 260);
    }

    #[test]
    fn test_token_percentages() {
        let stats = TokenStats {
            intent_analysis_tokens: 10,
            l0_search_tokens: 20,
            l1_search_tokens: 30,
            content_load_tokens: 40,
            aggregation_tokens: 0,
            total_tokens: 100,
        };

        let percentages = stats.percentages();
        assert!((percentages.intent_analysis - 10.0).abs() < 0.01);
        assert!((percentages.l0_search - 20.0).abs() < 0.01);
        assert!((percentages.content_load - 40.0).abs() < 0.01);
    }

    #[test]
    fn test_steps_by_type() {
        let trace = RetrievalTrace {
            query: "test".to_string(),
            steps: vec![
                RetrievalStep::intent_analysis(10),
                RetrievalStep::l0_search(
                    TianyanUri::new(ContextNamespace::Knowledge, vec!["a".to_string()]),
                    0.9,
                    50,
                ),
                RetrievalStep::l0_search(
                    TianyanUri::new(ContextNamespace::Knowledge, vec!["b".to_string()]),
                    0.8,
                    50,
                ),
            ],
            results: vec![],
            total_tokens: 110,
            total_time_ms: 50,
            timestamp: Utc::now(),
        };

        let l0_steps = trace.steps_by_type(RetrievalStepType::L0Search);
        assert_eq!(l0_steps.len(), 2);

        let intent_steps = trace.steps_by_type(RetrievalStepType::IntentAnalysis);
        assert_eq!(intent_steps.len(), 1);
    }
}
