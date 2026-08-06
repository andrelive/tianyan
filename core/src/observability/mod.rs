//! 智能体可查询的可观测性模块。
//!
//! 提供 Agent 运行状态的记录和查询接口，
//! 让智能体可以反思自己的执行历史，实现 Harness Engineering 原则 2
//! 的"环境可读性"。

pub mod usage_stats;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

/// 单次执行的 token 消耗记录。
#[derive(Debug, Clone)]
pub struct TokenRecord {
    /// 会话 ID。
    pub session_id: String,
    /// 记录时间戳。
    pub timestamp: DateTime<Utc>,
    /// 总 token 数。
    pub total_tokens: usize,
    /// 系统提示 token 数。
    pub system_prompt_tokens: usize,
    /// 检索上下文 token 数。
    pub retrieved_tokens: usize,
    /// 是否成功。
    pub success: bool,
}

/// 步骤失败统计。
#[derive(Debug, Clone)]
pub struct FailureStats {
    /// 步骤描述。
    pub step_description: String,
    /// 失败次数。
    pub failure_count: usize,
    /// 最后一次失败时间。
    pub last_failure: DateTime<Utc>,
    /// 最后一次错误信息。
    pub last_error: String,
}

/// 规则命中记录。
#[derive(Debug, Clone)]
pub struct RuleHitRecord {
    /// 规则 ID。
    pub rule_id: String,
    /// 会话 ID。
    pub session_id: String,
    /// 记录时间戳。
    pub timestamp: DateTime<Utc>,
    /// 是否相关。
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
    /// 创建新的 AgentMetrics 实例。
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
        let avg = total.checked_div(count).unwrap_or(0);

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
        sorted.sort_by_key(|f| std::cmp::Reverse(f.failure_count));

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
        let avg_tokens = if !history.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token_record(session: &str, tokens: usize, success: bool) -> TokenRecord {
        TokenRecord {
            session_id: session.to_string(),
            timestamp: Utc::now(),
            total_tokens: tokens,
            system_prompt_tokens: 100,
            retrieved_tokens: 200,
            success,
        }
    }

    #[tokio::test]
    async fn test_query_success_rate_empty() {
        let metrics = AgentMetrics::new();
        let result = metrics.query_success_rate().await;
        assert_eq!(result["total_executions"], 0);
        assert_eq!(result["successful"], 0);
        assert_eq!(result["success_rate_percent"], 0.0);
    }

    #[tokio::test]
    async fn test_record_execution_and_query_success_rate() {
        let metrics = AgentMetrics::new();
        metrics.record_execution(true).await;
        metrics.record_execution(true).await;
        metrics.record_execution(false).await;
        metrics.record_execution(true).await;

        let result = metrics.query_success_rate().await;
        assert_eq!(result["total_executions"], 4);
        assert_eq!(result["successful"], 3);
        let rate = result["success_rate_percent"].as_f64().unwrap();
        assert!((rate - 75.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_record_failure_and_query_common_failures() {
        let metrics = AgentMetrics::new();
        metrics
            .record_failure("read_file failed", "not found")
            .await;
        metrics
            .record_failure("read_file failed", "not found again")
            .await;
        metrics
            .record_failure("write failed", "permission denied")
            .await;

        let result = metrics.query_common_failures().await;
        let failures = result["common_failures"].as_array().unwrap();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0]["description"], "read_file failed");
        assert_eq!(failures[0]["count"], 2);
    }

    #[tokio::test]
    async fn test_record_token_usage_and_query_summary() {
        let metrics = AgentMetrics::new();
        metrics
            .record_token_usage(make_token_record("s1", 1000, true))
            .await;
        metrics
            .record_token_usage(make_token_record("s2", 2000, true))
            .await;
        metrics
            .record_token_usage(make_token_record("s3", 3000, false))
            .await;

        let result = metrics.query_token_summary().await;
        assert_eq!(result["total_executions"], 3);
        assert_eq!(result["total_tokens"], 6000);
        assert_eq!(result["average_tokens_per_execution"], 2000);
    }

    #[tokio::test]
    async fn test_token_history_truncation_at_1000() {
        let metrics = AgentMetrics::new();
        for _i in 0..1100 {
            metrics
                .record_token_usage(make_token_record("s", 1, true))
                .await;
        }
        let result = metrics.query_token_summary().await;
        assert_eq!(result["total_executions"], 1000);
    }

    #[tokio::test]
    async fn test_record_rule_hit_and_query_effectiveness() {
        let metrics = AgentMetrics::new();
        metrics.record_rule_hit(5).await;
        metrics.record_rule_hit(3).await;

        let result = metrics.query_rule_effectiveness().await;
        assert_eq!(result["relevant_hits"], 8);
        assert_eq!(result["irrelevant_misses"], 0);
    }

    #[tokio::test]
    async fn test_record_pipeline_failure() {
        let metrics = AgentMetrics::new();
        metrics.record_pipeline_failure().await;
        metrics.record_pipeline_failure().await;

        let health = metrics.query_harness_health().await;
        assert_eq!(health["pipeline_failures"], 2);
    }

    #[tokio::test]
    async fn test_query_harness_health_aggregate() {
        let metrics = AgentMetrics::new();
        metrics.record_execution(true).await;
        metrics.record_execution(true).await;
        metrics.record_execution(false).await;
        metrics.record_rule_hit(10).await;
        metrics
            .record_token_usage(make_token_record("s1", 5000, true))
            .await;
        metrics.record_pipeline_failure().await;

        let health = metrics.query_harness_health().await;
        assert_eq!(health["harness_version"], 1);
        assert_eq!(health["executions"], 3);
        let sr = health["success_rate_percent"].as_f64().unwrap();
        assert!((sr - 66.666).abs() < 1.0);
        assert_eq!(health["rules_injected"], 10);
        assert_eq!(health["pipeline_failures"], 1);
        assert_eq!(health["total_tokens_consumed"], 5000);
        assert_eq!(health["avg_tokens_per_execution"], 5000);
    }

    #[tokio::test]
    async fn test_query_common_failures_empty() {
        let metrics = AgentMetrics::new();
        let result = metrics.query_common_failures().await;
        let failures = result["common_failures"].as_array().unwrap();
        assert!(failures.is_empty());
    }
}
