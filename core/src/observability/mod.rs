//! 智能体可查询的可观测性模块。
//!
//! 提供 Agent 运行状态的记录和查询接口，
//! 让智能体可以反思自己的执行历史，实现 Harness Engineering 原则 2
//! 的"环境可读性"。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

/// 单次执行的 token 消耗记录。
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub total_tokens: usize,
    pub system_prompt_tokens: usize,
    pub retrieved_tokens: usize,
    pub success: bool,
}

/// 步骤失败统计。
#[derive(Debug, Clone)]
pub struct FailureStats {
    pub step_description: String,
    pub failure_count: usize,
    pub last_failure: DateTime<Utc>,
    pub last_error: String,
}

/// 规则命中记录。
#[derive(Debug, Clone)]
pub struct RuleHitRecord {
    pub rule_id: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub was_relevant: bool,
}

/// 智能体可查询的可观测性存储。
pub struct AgentMetrics {
    token_history: RwLock<Vec<TokenRecord>>,
    failure_history: RwLock<Vec<FailureStats>>,
    execution_count: RwLock<usize>,
    success_count: RwLock<usize>,
    rule_hit_count: RwLock<usize>,
    rule_miss_count: RwLock<usize>,
    pipeline_failures: RwLock<usize>,
}

impl AgentMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            token_history: RwLock::new(Vec::new()),
            failure_history: RwLock::new(Vec::new()),
            execution_count: RwLock::new(0),
            success_count: RwLock::new(0),
            rule_hit_count: RwLock::new(0),
            rule_miss_count: RwLock::new(0),
            pipeline_failures: RwLock::new(0),
        })
    }

    /// 记录一次 token 消耗。
    pub async fn record_token_usage(&self, record: TokenRecord) {
        let mut history = self.token_history.write().await;
        history.push(record);
        if history.len() > 1000 {
            let keep_from = history.len() - 1000;
            history.drain(..keep_from);
        }
    }

    /// 记录一次步骤失败。
    pub async fn record_failure(&self, step_description: &str, error: &str) {
        let mut failures = self.failure_history.write().await;
        if let Some(existing) = failures
            .iter_mut()
            .find(|f| f.step_description == step_description)
        {
            existing.failure_count += 1;
            existing.last_failure = Utc::now();
            existing.last_error = error.to_string();
        } else {
            failures.push(FailureStats {
                step_description: step_description.to_string(),
                failure_count: 1,
                last_failure: Utc::now(),
                last_error: error.to_string(),
            });
        }
    }

    /// 记录一次执行完成。
    pub async fn record_execution(&self, success: bool) {
        *self.execution_count.write().await += 1;
        if success {
            *self.success_count.write().await += 1;
        }
    }

    /// 记录一次规则命中（learned rule 与当前 query 相关并被注入）。
    pub async fn record_rule_hit(&self, rule_count: usize) {
        *self.rule_hit_count.write().await += rule_count;
    }

    /// 记录一次 Pipeline 失败。
    pub async fn record_pipeline_failure(&self) {
        *self.pipeline_failures.write().await += 1;
    }

    /// 查询 token 消耗历史摘要。
    pub async fn query_token_summary(&self) -> serde_json::Value {
        let history = self.token_history.read().await;
        let total: usize = history.iter().map(|r| r.total_tokens).sum();
        let count = history.len();
        let avg = if count > 0 { total / count } else { 0 };

        serde_json::json!({
            "total_executions": count,
            "total_tokens": total,
            "average_tokens_per_execution": avg,
        })
    }

    /// 查询成功率。
    pub async fn query_success_rate(&self) -> serde_json::Value {
        let exec = *self.execution_count.read().await;
        let success = *self.success_count.read().await;
        let rate = if exec > 0 {
            (success as f32 / exec as f32) * 100.0
        } else {
            0.0
        };

        serde_json::json!({
            "total_executions": exec,
            "successful": success,
            "success_rate_percent": rate,
        })
    }

    /// 查询规则有效性统计。
    pub async fn query_rule_effectiveness(&self) -> serde_json::Value {
        let hits = *self.rule_hit_count.read().await;
        let total = hits + *self.rule_miss_count.read().await;
        let hit_rate = if total > 0 {
            (hits as f32 / total as f32) * 100.0
        } else {
            0.0
        };

        serde_json::json!({
            "total_rule_injections": total,
            "relevant_hits": hits,
            "irrelevant_misses": *self.rule_miss_count.read().await,
            "rule_relevance_percent": hit_rate,
        })
    }

    /// 查询常见失败。
    pub async fn query_common_failures(&self) -> serde_json::Value {
        let failures = self.failure_history.read().await;
        let mut sorted = failures.clone();
        sorted.sort_by(|a, b| b.failure_count.cmp(&a.failure_count));

        let top: Vec<serde_json::Value> = sorted
            .iter()
            .take(10)
            .map(|f| {
                serde_json::json!({
                    "description": f.step_description,
                    "count": f.failure_count,
                    "last_error": f.last_error,
                })
            })
            .collect();

        serde_json::json!({ "common_failures": top })
    }

    /// 查询 Harness 健康摘要（供 Agent 自省）。
    pub async fn query_harness_health(&self) -> serde_json::Value {
        let exec = *self.execution_count.read().await;
        let success = *self.success_count.read().await;
        let pipeline_fail = *self.pipeline_failures.read().await;
        let rule_hits = *self.rule_hit_count.read().await;

        let history = self.token_history.read().await;
        let total_tokens: usize = history.iter().map(|r| r.total_tokens).sum();
        let avg_tokens = if history.len() > 0 {
            total_tokens / history.len()
        } else {
            0
        };

        let success_rate = if exec > 0 {
            (success as f32 / exec as f32) * 100.0
        } else {
            0.0
        };

        serde_json::json!({
            "harness_version": 1,
            "executions": exec,
            "success_rate_percent": success_rate,
            "pipeline_failures": pipeline_fail,
            "rules_injected": rule_hits,
            "avg_tokens_per_execution": avg_tokens,
            "total_tokens_consumed": total_tokens,
            "common_failures_count": self.failure_history.read().await.len(),
        })
    }
}

impl Default for AgentMetrics {
    fn default() -> Self {
        Self {
            token_history: RwLock::new(Vec::new()),
            failure_history: RwLock::new(Vec::new()),
            execution_count: RwLock::new(0),
            success_count: RwLock::new(0),
            rule_hit_count: RwLock::new(0),
            rule_miss_count: RwLock::new(0),
            pipeline_failures: RwLock::new(0),
        }
    }
}
