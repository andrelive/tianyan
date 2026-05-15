//! 上下文类型定义。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::types::{ContentLevel, TianyanUri};

/// 默认新鲜度半衰期（天）。每过此天数，新鲜度减半。
pub const DEFAULT_FRESHNESS_HALF_LIFE_DAYS: f64 = 30.0;

/// 半衰期新鲜度衰减。
/// 每过半衰期天数，新鲜度减半。
///
/// - `age_days`: 距今天数
/// - `half_life_days`: 半衰期（默认 30 天）
/// - `returns`: 0.0~1.0 的新鲜度评分
pub fn compute_freshness(age_days: i64, half_life_days: f64) -> f32 {
    if age_days <= 0 {
        return 1.0;
    }
    (0.5_f64).powf(age_days as f64 / half_life_days) as f32
}

/// 带有内容的检索结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    /// 结果 URI。
    pub uri: TianyanUri,
    /// 相关性分数。
    pub score: f32,
    /// 已加载的内容（如果可用）。
    pub content: Option<String>,
    /// 加载的内容层级。
    pub content_level: ContentLevel,
    /// 内容的 Token 数量。
    pub token_count: usize,
    /// 结果的类别。
    pub category: String,
    /// 结果关联的标签。
    pub tags: Vec<String>,
    /// 源内容最后更新时间（用于新鲜度计算）。
    pub source_updated_at: Option<DateTime<Utc>>,
    /// 新鲜度评分（0.0~1.0，1.0 为最新）。
    pub freshness_score: f32,
}

impl RetrievalResult {
    /// 创建新的检索结果。
    pub fn new(uri: TianyanUri, score: f32) -> Self {
        Self {
            uri,
            score,
            content: None,
            content_level: ContentLevel::Abstract,
            token_count: 0,
            category: String::new(),
            tags: Vec::new(),
            source_updated_at: None,
            freshness_score: 1.0,
        }
    }

    /// 检查是否已加载内容。
    pub fn has_content(&self) -> bool {
        self.content.is_some()
    }
}

/// 检索追踪步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalStep {
    /// 步骤类型。
    pub step_type: RetrievalStepType,
    /// 目标 URI。
    pub target_uri: TianyanUri,
    /// 相关性分数。
    pub score: Option<f32>,
    /// Token 消耗量。
    pub tokens_used: usize,
    /// 时间戳。
    pub timestamp: DateTime<Utc>,
}

/// 检索步骤类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStepType {
    /// 意图分析。
    IntentAnalysis,
    /// L0 向量搜索。
    L0Search,
    /// L1 向量搜索。
    L1Search,
    /// 内容加载。
    ContentLoad,
    /// 结果聚合。
    Aggregation,
}

/// 完整的检索追踪记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalTrace {
    /// 原始查询。
    pub query: String,
    /// 检索步骤列表。
    pub steps: Vec<RetrievalStep>,
    /// 最终结果 URI 列表。
    pub results: Vec<TianyanUri>,
    /// 总 Token 消耗量。
    pub total_tokens: usize,
    /// 总检索时间（毫秒）。
    pub total_time_ms: u64,
    /// 时间戳。
    pub timestamp: DateTime<Utc>,
}

impl RetrievalTrace {
    /// 创建新的检索追踪记录。
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            steps: Vec::new(),
            results: Vec::new(),
            total_tokens: 0,
            total_time_ms: 0,
            timestamp: Utc::now(),
        }
    }

    /// 添加一个步骤到追踪记录。
    pub fn add_step(&mut self, step: RetrievalStep) {
        self.total_tokens += step.tokens_used;
        self.steps.push(step);
    }

    /// 添加结果 URI。
    pub fn add_result(&mut self, uri: TianyanUri) {
        self.results.push(uri);
    }
}

/// 统一上下文窗口，聚合所有注入 prompt 的上下文。
#[derive(Debug, Clone)]
pub struct ContextWindow {
    /// 系统提示词（从 VFS Agent 命名空间加载）。
    pub system_prompt: String,
    /// 压缩后的对话摘要。
    pub summary: Option<String>,
    /// 检索结果（含 URI、内容、分数、层级、类别）。
    pub retrieved: Vec<RetrievalResult>,
    /// Token 使用统计。
    pub token_usage: ContextTokenUsage,
}

impl ContextWindow {
    /// 创建空的上下文窗口。
    pub fn new(system_prompt: String) -> Self {
        Self {
            system_prompt,
            summary: None,
            retrieved: Vec::new(),
            token_usage: ContextTokenUsage::default(),
        }
    }

    /// 计算总 token 数。
    pub fn total_tokens(&self) -> usize {
        self.token_usage.total()
    }
}

/// 上下文窗口的 token 使用统计。
#[derive(Debug, Clone, Default)]
pub struct ContextTokenUsage {
    pub system_prompt_tokens: usize,
    pub summary_tokens: usize,
    pub retrieved_tokens: usize,
}

impl ContextTokenUsage {
    pub fn total(&self) -> usize {
        self.system_prompt_tokens + self.summary_tokens + self.retrieved_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retrieval_trace() {
        let mut trace = RetrievalTrace::new("test query");
        let step = RetrievalStep {
            step_type: RetrievalStepType::L0Search,
            target_uri: TianyanUri::parse("tianyan://user/profile").unwrap(),
            score: Some(0.9),
            tokens_used: 100,
            timestamp: Utc::now(),
        };
        trace.add_step(step);
        assert_eq!(trace.total_tokens, 100);
    }
}
