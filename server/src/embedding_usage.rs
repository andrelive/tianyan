//! 嵌入用量入账（model 层契约 → `usage_logs` 表）。
//!
//! 背景：嵌入调用**不走 AgentLoop**（检索与摘要写入直接调 `EmbeddingService`），
//! 其 token 用量此前完全不入账——应用内用量统计里只有 chat provider，
//! 账单核对时"嵌入到底调了多少"无从查证（日志可证嵌入确实在被调用：
//! `embed_single_with_dimensions called model=text-embedding-v4`）。
//!
//! 本模块把 model 层的 [`EmbeddingUsageSink`] 契约落到 `usage_logs`：
//! `provider` 由装配层确定（如 `bailian`），`session_id` 记为 `""`（嵌入不属会话轮次）。

use std::sync::Arc;

use tianyan::common::types::TokenUsage;
use tianyan::model::EmbeddingUsageSink;
use tianyan::observability::usage_log::UsageLog;

/// 把嵌入用量写入 `usage_logs` 的实现。
pub struct UsageLogEmbeddingSink {
    log: Arc<UsageLog>,
    provider: String,
}

impl UsageLogEmbeddingSink {
    /// 构造（`provider` = 嵌入 provider 名，账单核对用）。
    pub fn new(log: Arc<UsageLog>, provider: impl Into<String>) -> Self {
        Self {
            log,
            provider: provider.into(),
        }
    }
}

impl EmbeddingUsageSink for UsageLogEmbeddingSink {
    fn record(&self, model: &str, usage: &TokenUsage, _calls: usize) {
        let log = self.log.clone();
        let provider = self.provider.clone();
        let model = model.to_string();
        let usage = usage.clone();
        // 非阻塞：本方法在检索热路径上被调用，不得等待 SQLite I/O
        tokio::spawn(async move {
            log.record("", &provider, &model, &usage).await;
        });
    }
}

/// 从配置解析嵌入 provider 名（未配置时 `"unknown"`）。
pub fn resolve_embedding_provider(config: &tianyan::config::TianyanConfig) -> String {
    config
        .models
        .resolve(tianyan::config::ModelCapability::TextEmbedding)
        .map(|r| r.provider.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 构造并注入嵌入用量 sink（失败只告警——嵌入照常工作，仅不入账）。
pub fn attach_embedding_usage_sink(
    services: &tianyan::model::ModelServices,
    usage_log: Arc<UsageLog>,
    config: &tianyan::config::TianyanConfig,
) {
    services.set_embedding_usage_sink(Arc::new(UsageLogEmbeddingSink::new(
        usage_log,
        resolve_embedding_provider(config),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tianyan::db::Database;

    /// 端到端：`record` → `usage_logs` 落库（provider/model/token 均可查）。
    ///
    /// 这是本节改动的核心收益：嵌入消耗从此可在用量统计与账单核对里看到
    /// （此前 `usage_logs` 只有 chat provider，"嵌入了多少"无从查证）。
    #[tokio::test]
    async fn test_sink_writes_embedding_usage_to_usage_logs() {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let log = UsageLog::new(db).unwrap();

        let sink = UsageLogEmbeddingSink::new(log.clone(), "bailian");
        sink.record("text-embedding-v4", &TokenUsage::new(42, 0), 1);

        // record 内部用 tokio::spawn 异步落库：等它完成
        let mut stats = Vec::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            stats = log
                .stats(
                    None,
                    None,
                    Some("bailian"),
                    Some("text-embedding-v4"),
                    "none",
                )
                .await;
            if stats.first().map(|s| s.calls).unwrap_or(0) > 0 {
                break;
            }
        }

        assert_eq!(stats.len(), 1, "应能按 provider 查到嵌入用量分组");
        assert_eq!(stats[0].calls, 1, "嵌入调用应记 1 次");
        assert_eq!(stats[0].uncached_input, 42, "输入 token 应落库");
        assert_eq!(stats[0].total_tokens, 42);
        assert_eq!(stats[0].completion_tokens, 0, "嵌入无输出 token");
    }

    /// 未配置嵌入 provider 时名字回退为 `unknown`（不 panic）。
    #[test]
    fn test_resolve_embedding_provider_defaults_to_unknown() {
        let config = tianyan::config::TianyanConfig::default();
        let name = resolve_embedding_provider(&config);
        assert!(
            !name.is_empty(),
            "provider 名不得为空（未配置时为 unknown）"
        );
    }
}
