//! 用量日志域仓储（Repository 层）。
//!
//! **SQL 收敛**：LLM 调用用量（usage_logs 表）的写入与聚合统计从
//! `observability::usage_log::UsageLog` 收口于此。

use std::sync::Arc;

use crate::db::Database;

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

/// 用量日志域仓储。
#[derive(Clone)]
pub struct UsageRepo {
    db: Arc<Database>,
}

impl UsageRepo {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 记录一次 LLM 调用用量（非热路径：直接写 SQLite；失败仅告警）。
    /// 组件已从 TokenUsage 提取原始计数。
    pub async fn record(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        uncached: u64,
        cached: u64,
        completion: u64,
        total: u64,
    ) {
        let conn = match self.db.try_lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "usage_log: 跳过（数据库锁不可用）");
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO usage_logs (session_id, provider, model, uncached_input, cached_input, completion_tokens, total_tokens, ts) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                session_id,
                provider,
                model,
                uncached as i64,
                cached as i64,
                completion as i64,
                total as i64,
                chrono::Utc::now().timestamp()
            ],
        ) {
            tracing::warn!(error = %e, "usage_log 写入失败");
        }
    }

    /// 聚合统计（since/until/provider/model 过滤；group_by：provider|model|none）。
    pub async fn stats(
        &self,
        since_ts: Option<i64>,
        until_ts: Option<i64>,
        provider: Option<&str>,
        model: Option<&str>,
        group_by: &str,
    ) -> Vec<UsageStat> {
        let mut sql = String::from(
            "SELECT provider, model, COUNT(*), SUM(uncached_input), SUM(cached_input), SUM(completion_tokens), SUM(total_tokens) FROM usage_logs",
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
            Ok(iter) => iter.filter_map(|r| r.ok()).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(error = %e, "usage_log 统计查询失败");
                return Vec::new();
            }
        };
        rows.into_iter().map(Self::stat_from).collect()
    }

    /// 总计统计（group_by = none）。
    async fn total_stat(
        &self,
        _sql: &str,
        conds: &[Box<dyn rusqlite::types::ToSql + Send + Sync>],
    ) -> Vec<UsageStat> {
        let mut agg = String::from(
            "SELECT COUNT(*), COALESCE(SUM(uncached_input),0), COALESCE(SUM(cached_input),0), COALESCE(SUM(completion_tokens),0), COALESCE(SUM(total_tokens),0) FROM usage_logs",
        );
        if !conds.is_empty() {
            agg.push_str(" WHERE ts >= ?");
            if conds.len() >= 2 {
                agg.push_str(" AND ts <= ?");
            }
            if conds.len() >= 3 {
                agg.push_str(" AND provider = ?");
            }
            if conds.len() >= 4 {
                agg.push_str(" AND model = ?");
            }
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
