//! LLM 用量日志（token 统计：缓存未命中输入 / 缓存命中输入 / 输出 / 推理）。
//!
//! 每次 LLM 调用一行（`usage_logs` 表），覆盖聊天、子代理委托、演化任务等
//! 所有走 AgentLoop 的路径；统计面板按 provider / model / 时段聚合。
//!
//! 口径：
//! - `uncached_input` = prompt_tokens - cache_read（未命中输入，真金白银）
//! - `cached_input` = cache_read（缓存命中输入）
//! - `completion_tokens` = 输出
//! - 缓存命中率 = cached_input / (uncached_input + cached_input)

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::common::types::TokenUsage;
use crate::db::Database;
use crate::db::usage::{UsageRepo, UsageStat};

/// LLM 用量日志（共享 SqliteDb 连接，与 VFS 同库）。
#[derive(Clone)]
pub struct UsageLog {
    /// 用量日志域仓储（SQL 收敛：写入/统计经此）。
    repo: UsageRepo,
}


impl UsageLog {
    /// 创建用量日志（共享 SqliteDb）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { repo: UsageRepo::new(db) }))
    }

    /// 记录一次 LLM 调用用量（非热路径：每轮一次，直接写 SQLite）。
    ///
    /// provider 由 model 名推导（约定 `provider/model` 前缀命名，
    /// 如 `opencode/deepseek-v4-flash`）。写失败仅告警，不影响主流程。
    pub async fn record(&self, session_id: &str, provider: &str, model: &str, usage: &TokenUsage) {
        // 落盘经 UsageRepo（SQL 收敛于 db 层）
        self.repo
            .record(
                session_id,
                provider,
                model,
                usage.prompt_tokens.saturating_sub(usage.cache_read) as u64,
                usage.cache_read as u64,
                usage.completion_tokens as u64,
                usage.total_tokens as u64,
            )
            .await;
    }

    /// 聚合统计。
    ///
    /// - `since_ts`：None 表示全部；Some 为 epoch 秒下限。
    /// - `provider` / `model`：非空时按精确值过滤。
    /// - `group_by`：`provider` | `model` | `none`（总计）。
    pub async fn stats(
        &self,
        since_ts: Option<i64>,
        until_ts: Option<i64>,
        provider: Option<&str>,
        model: Option<&str>,
        group_by: &str,
    ) -> Vec<UsageStat> {
        self.repo.stats(since_ts, until_ts, provider, model, group_by).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Arc<UsageLog> {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        UsageLog::new(db).unwrap()
    }

    fn usage(prompt: usize, cached: usize, output: usize) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: output,
            total_tokens: prompt + output,
            cache_read: cached,
            cache_write: 0,
        }
    }

    #[tokio::test]
    async fn test_record_and_stats_total() {
        let log = setup().await;
        log.record(
            "s1",
            "opencode",
            "deepseek-v4-flash",
            &usage(1000, 800, 200),
        )
        .await;
        log.record("s1", "opencode", "deepseek-v4-flash", &usage(500, 100, 300))
            .await;
        log.record(
            "evolution-1",
            "bailian",
            "qwen3.7-plus",
            &usage(2000, 0, 400),
        )
        .await;

        let stats = log.stats(None, None, None, None, "none").await;
        assert_eq!(stats.len(), 1);
        let s = &stats[0];
        assert_eq!(s.calls, 3);
        // uncached = (1000-800) + (500-100) + (2000-0) = 200+400+2000 = 2600
        assert_eq!(s.uncached_input, 2600);
        assert_eq!(s.cached_input, 900);
        assert_eq!(s.completion_tokens, 900);
        assert_eq!(s.total_tokens, 4400);
        // 命中率 = 900 / (2600+900) = 0.257
        assert!((s.cache_hit_rate - 900.0 / 3500.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_stats_group_by_model() {
        let log = setup().await;
        log.record(
            "s1",
            "opencode",
            "deepseek-v4-flash",
            &usage(1000, 800, 200),
        )
        .await;
        log.record("s1", "bailian", "qwen3.7-plus", &usage(2000, 0, 400))
            .await;

        let stats = log.stats(None, None, None, None, "model").await;
        assert_eq!(stats.len(), 2);
        // 按 total 降序：bailian 2400 > opencode 1200
        assert_eq!(stats[0].group, "bailian/qwen3.7-plus");
        assert_eq!(stats[0].uncached_input, 2000);
        assert_eq!(stats[1].group, "opencode/deepseek-v4-flash");
        assert_eq!(stats[1].cached_input, 800);
    }

    #[tokio::test]
    async fn test_stats_filter_provider_and_since() {
        let log = setup().await;
        log.record(
            "s1",
            "opencode",
            "deepseek-v4-flash",
            &usage(1000, 800, 200),
        )
        .await;
        log.record("s1", "bailian", "qwen3.7-plus", &usage(2000, 0, 400))
            .await;

        // 按 provider 过滤
        let stats = log.stats(None, None, Some("opencode"), None, "model").await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].group, "opencode/deepseek-v4-flash");

        // since = 未来时间 → 无记录
        let stats = log
            .stats(
                Some(chrono::Utc::now().timestamp() + 100000),
                None,
                None,
                None,
                "none",
            )
            .await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].calls, 0);
    }

    #[test]
    fn test_new_error_type_is_tianyan_error() {
        let db = Database::open_in_memory().unwrap();
        let result: Result<Arc<UsageLog>, TianyanError> = UsageLog::new(db);
        assert!(result.is_ok());
    }
}
