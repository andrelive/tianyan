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
use crate::vfs::backend::sqlite_db::SqliteDb;

/// LLM 用量日志（共享 SqliteDb 连接，与 VFS 同库）。
#[derive(Clone)]
pub struct UsageLog {
    db: SqliteDb,
}

/// 聚合统计结果（总计或按 provider/model 分组）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageStat {
    /// 分组键（provider 或 `provider/model`；总计时为 ""）。
    pub group: String,
    /// LLM 调用次数。
    pub calls: u64,
    /// 缓存未命中输入 token。
    pub uncached_input: u64,
    /// 缓存命中输入 token。
    pub cached_input: u64,
    /// 输出 token。
    pub completion_tokens: u64,
    /// 总 token（含输入输出）。
    pub total_tokens: u64,
    /// 缓存命中率（0.0 ~ 1.0；无输入时为 0）。
    pub cache_hit_rate: f64,
}

impl UsageLog {
    /// 创建用量日志（共享 SqliteDb）。
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
    }

    /// 记录一次 LLM 调用用量（非热路径：每轮一次，直接写 SQLite）。
    ///
    /// provider 由 model 名推导（约定 `provider/model` 前缀命名，
    /// 如 `opencode/deepseek-v4-flash`）。写失败仅告警，不影响主流程。
    pub async fn record(&self, session_id: &str, provider: &str, model: &str, usage: &TokenUsage) {
        let uncached = usage.prompt_tokens.saturating_sub(usage.cache_read);
        let conn = match self.db.try_lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "usage_log: 跳过（数据库锁不可用）");
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO usage_logs
                (session_id, provider, model, uncached_input, cached_input,
                 completion_tokens, total_tokens, ts)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                session_id,
                provider,
                model,
                uncached as i64,
                usage.cache_read as i64,
                usage.completion_tokens as i64,
                usage.total_tokens as i64,
                chrono::Utc::now().timestamp()
            ],
        ) {
            tracing::warn!(error = %e, "usage_log 写入失败");
        }
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
        let mut sql = String::from(
            "SELECT provider, model, COUNT(*),
                    SUM(uncached_input), SUM(cached_input),
                    SUM(completion_tokens), SUM(total_tokens)
             FROM usage_logs",
        );
        let mut conds = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql + Send + Sync>> = Vec::new();
        if let Some(ts) = since_ts {
            conds.push("ts >= ?");
            params.push(Box::new(ts));
        }
        if let Some(ts) = until_ts {
            conds.push("ts <= ?");
            params.push(Box::new(ts));
        }
        if let Some(p) = provider {
            conds.push("provider = ?");
            params.push(Box::new(p.to_string()));
        }
        if let Some(m) = model {
            conds.push("model = ?");
            params.push(Box::new(m.to_string()));
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" GROUP BY ");
        match group_by {
            "provider" => sql.push_str("provider ORDER BY SUM(total_tokens) DESC"),
            "model" => {
                sql.push_str("provider, model ORDER BY SUM(total_tokens) DESC");
            }
            _ => {
                // 总计：去掉 GROUP BY 再查
                sql.truncate(sql.len() - " GROUP BY ".len());
                return self.total_stat(&sql, &params).await;
            }
        }

        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "usage_log 统计 prepare 失败");
                return Vec::new();
            }
        };
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = match stmt.query_map(params_ref.as_slice(), |row| {
            let provider: String = row.get(0)?;
            let model: String = row.get(1)?;
            let group = if group_by == "provider" {
                provider
            } else {
                format!("{}/{}", provider, model)
            };
            Ok((
                group,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
                row.get::<_, i64>(5)? as u64,
                row.get::<_, i64>(6)? as u64,
            ))
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(error = %e, "usage_log 统计查询失败");
                return Vec::new();
            }
        };
        rows.into_iter().map(Self::stat_from).collect()
    }

    /// 总计统计（group_by = none）：单行聚合，无条件时退化为全表 SUM。
    async fn total_stat(
        &self,
        _sql: &str,
        conds: &[Box<dyn rusqlite::types::ToSql + Send + Sync>],
    ) -> Vec<UsageStat> {
        let mut agg = String::from(
            "SELECT COUNT(*), COALESCE(SUM(uncached_input),0), COALESCE(SUM(cached_input),0),
                    COALESCE(SUM(completion_tokens),0), COALESCE(SUM(total_tokens),0)
             FROM usage_logs",
        );
        if !conds.is_empty() {
            agg.push_str(" WHERE ");
            let mut parts = Vec::new();
            if conds.len() >= 1 {
                parts.push("ts >= ?");
            }
            if conds.len() >= 2 {
                parts.push("ts <= ?");
            }
            if conds.len() >= 3 {
                parts.push("provider = ?");
            }
            if conds.len() >= 4 {
                parts.push("model = ?");
            }
            agg.push_str(&parts.join(" AND "));
        }
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(&agg) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, sql = %agg, "usage_log 总计 prepare 失败");
                return Vec::new();
            }
        };
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = conds
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::types::ToSql)
            .collect();
        match stmt.query_row(params_ref.as_slice(), |row| {
            Ok(Self::stat_from((
                String::new(),
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, i64>(4)? as u64,
            )))
        }) {
            Ok(s) => vec![s],
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                vec![Self::stat_from((String::new(), 0, 0, 0, 0, 0))]
            }
            Err(e) => {
                tracing::warn!(error = %e, "usage_log 总计查询失败");
                Vec::new()
            }
        }
    }

    /// 行元组 → 统计结构（含命中率计算）。
    fn stat_from(t: (String, u64, u64, u64, u64, u64)) -> UsageStat {
        let (group, calls, uncached, cached, completion, total) = t;
        let input = uncached + cached;
        let hit = if input > 0 {
            cached as f64 / input as f64
        } else {
            0.0
        };
        UsageStat {
            group,
            calls,
            uncached_input: uncached,
            cached_input: cached,
            completion_tokens: completion,
            total_tokens: total,
            cache_hit_rate: hit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Arc<UsageLog> {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
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
        let db = SqliteDb::open_in_memory().unwrap();
        let result: Result<Arc<UsageLog>, TianyanError> = UsageLog::new(db);
        assert!(result.is_ok());
    }
}
