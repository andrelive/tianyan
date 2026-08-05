//! 检索追踪模块。
//!
//! 本模块实现用于调试和优化的检索追踪记录。

use chrono::Utc;
use std::time::Instant;

use crate::common::types::TianyanUri;

use super::types::{RetrievalStep, RetrievalStepType, RetrievalTrace};

/// 用于创建检索追踪记录的构建器。
#[derive(Debug)]
pub(crate) struct RetrievalTraceBuilder {
    query: String,
    steps: Vec<RetrievalStep>,
    results: Vec<TianyanUri>,
    total_tokens: usize,
    start_time: Instant,
}

// 构建器由 DualLayerRetriever 生产使用（检索轨迹持久化链路）。
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

    /// 添加自定义步骤。
    pub fn add_step(&mut self, step: RetrievalStep) {
        self.total_tokens += step.tokens_used;
        self.steps.push(step);
    }

    /// 添加结果 URI。
    pub fn add_result(&mut self, uri: TianyanUri) {
        self.results.push(uri);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;

    #[test]
    fn test_retrieval_trace_builder() {
        let mut builder = RetrievalTraceBuilder::new("test query");
        builder.add_intent_analysis(10);
        builder.add_l1_search(
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
}
