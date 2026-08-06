//! 检索追踪类型（跨模块共享的持久化契约）。
//!
//! 本模块定义单次检索的完整追踪记录，是 `retrieval_traces.trace_json`
//! 列与 `gui-vite/src/lib/types.ts` 镜像的**冻结契约**：
//! 字段名/序列化格式一经变更即破坏已持久化数据与前端展示。
//!
//! 依赖方向：本模块只依赖 `common::types`，供 `context`（生产）
//! 与 `observability`（持久化）共同使用，避免 `context ↔ observability` 依赖环。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::types::TianyanUri;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_trace() -> RetrievalTrace {
        RetrievalTrace {
            query: "为什么没找到".to_string(),
            steps: vec![RetrievalStep {
                step_type: RetrievalStepType::L0Search,
                target_uri: TianyanUri::parse("tianyan://knowledge/rust.md").unwrap(),
                score: Some(0.87),
                tokens_used: 120,
                timestamp: Utc::now(),
            }],
            results: vec![TianyanUri::parse("tianyan://knowledge/rust.md").unwrap()],
            total_tokens: 120,
            total_time_ms: 42,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_serde_roundtrip_byte_stable() {
        // 锁定持久化契约：序列化 → 反序列化 → 再序列化，字节必须一致
        let trace = sample_trace();
        let json = serde_json::to_string(&trace).unwrap();
        let decoded: RetrievalTrace = serde_json::from_str(&json).unwrap();
        let json_again = serde_json::to_string(&decoded).unwrap();
        assert_eq!(json, json_again, "round-trip 后序列化字节应一致");

        // 字段契约（与 gui-vite/src/lib/types.ts 镜像）原样保留
        assert_eq!(decoded.query, trace.query);
        assert_eq!(decoded.steps.len(), 1);
        assert_eq!(decoded.steps[0].step_type, RetrievalStepType::L0Search);
        assert_eq!(decoded.steps[0].score, Some(0.87));
        assert_eq!(decoded.steps[0].tokens_used, 120);
        assert_eq!(decoded.results.len(), 1);
        assert_eq!(decoded.total_tokens, 120);
        assert_eq!(decoded.total_time_ms, 42);
    }

    #[test]
    fn test_step_type_snake_case_contract() {
        // 持久化契约：步骤类型序列化为 snake_case（前端 RetrievalStepType 镜像）
        assert_eq!(
            serde_json::to_string(&RetrievalStepType::IntentAnalysis).unwrap(),
            "\"intent_analysis\""
        );
        assert_eq!(
            serde_json::to_string(&RetrievalStepType::L0Search).unwrap(),
            "\"l0_search\""
        );
        assert_eq!(
            serde_json::to_string(&RetrievalStepType::L1Search).unwrap(),
            "\"l1_search\""
        );
        assert_eq!(
            serde_json::to_string(&RetrievalStepType::ContentLoad).unwrap(),
            "\"content_load\""
        );
        assert_eq!(
            serde_json::to_string(&RetrievalStepType::Aggregation).unwrap(),
            "\"aggregation\""
        );
    }
}
