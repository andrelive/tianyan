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
    /// 创建用量仓储（共享 `Database` 单连接）。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 记录一次 LLM 调用用量（非热路径：直接写 SQLite；失败仅告警）。
    /// 组件已从 TokenUsage 提取原始计数。
    ///
    /// 注：字段较多（8 参数）；后续可收敛为 `UsageRecord` 参数对象（记入债务）。
    #[allow(clippy::too_many_arguments)]
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
        // 写路径异步等待锁：try_lock 静默丢弃会让用量统计与演化输入失真。
        let conn = self.db.lock().await;
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
        // WHERE 子句**单点构造**：分组查询与总计查询共用同一份。
        // （此前 `total_stat` 丢弃调用方传入的 SQL，改按 conds 个数/顺序硬编码
        // 重建 WHERE——过滤条件一改就静默失配，是数据口径分裂的隐患。）
        let mut conds: Vec<&str> = Vec::new();
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
        let where_clause = if conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conds.join(" AND "))
        };

        let mut sql = format!(
            "SELECT provider, model, COUNT(*), SUM(uncached_input), SUM(cached_input), SUM(completion_tokens), SUM(total_tokens) FROM usage_logs{where_clause}"
        );
        match group_by {
            "provider" => {
                sql.push_str(" GROUP BY provider ORDER BY SUM(total_tokens) DESC");
            }
            "model" => {
                sql.push_str(" GROUP BY provider, model ORDER BY SUM(total_tokens) DESC");
            }
            _ => {
                return self.total_stat(&where_clause, &params).await;
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
    ///
    /// `where_clause` 由调用方单点构造（含前导 `" WHERE "`，无过滤时为
    /// 空串）——本函数不再按参数顺序重建 WHERE。
    async fn total_stat(
        &self,
        where_clause: &str,
        conds: &[Box<dyn rusqlite::types::ToSql + Send + Sync>],
    ) -> Vec<UsageStat> {
        let agg = format!(
            "SELECT COUNT(*), COALESCE(SUM(uncached_input),0), COALESCE(SUM(cached_input),0), COALESCE(SUM(completion_tokens),0), COALESCE(SUM(total_tokens),0) FROM usage_logs{where_clause}"
        );
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

    async fn setup() -> UsageRepo {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        UsageRepo::new(db)
    }

    /// 回归测试：WHERE 子句**单点构造**——总计与分组查询共用同一过滤条件。
    ///
    /// 旧实现的 `total_stat` 丢弃调用方传入的 SQL，改按 `conds.len()` 个数/
    /// 顺序硬编码重建 WHERE（只认 `ts>= / ts<= / provider / model` 四种位置），
    /// 过滤组合一改就静默失配——本测试同时断言总计与分组口径一致。
    #[tokio::test]
    async fn test_stats_filters_shared_by_total_and_grouping() {
        let repo = setup().await;
        repo.record("s1", "deepseek", "v3", 100, 0, 10, 110).await;
        repo.record("s1", "deepseek", "v3", 50, 50, 10, 110).await;
        repo.record("s2", "ollama", "qwen", 200, 0, 20, 220).await;

        // 无过滤总计
        let total = repo.stats(None, None, None, None, "none").await;
        assert_eq!(total.len(), 1);
        assert_eq!(total[0].calls, 3);
        assert_eq!(total[0].uncached_input, 350);
        assert_eq!(total[0].cached_input, 50);
        assert_eq!(total[0].completion_tokens, 40);
        assert_eq!(total[0].total_tokens, 440);

        // 单条件：provider
        let ds = repo.stats(None, None, Some("deepseek"), None, "none").await;
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].calls, 2);
        assert_eq!(ds[0].total_tokens, 220);

        // 双条件：provider + model（旧实现最易在此失配）
        let grouped = repo
            .stats(None, None, Some("deepseek"), Some("v3"), "model")
            .await;
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].group, "deepseek/v3");
        assert_eq!(grouped[0].calls, 2);
        assert_eq!(grouped[0].total_tokens, 220);

        let single = repo
            .stats(None, None, Some("deepseek"), Some("v3"), "none")
            .await;
        assert_eq!(single.len(), 1);
        assert_eq!(
            single[0].calls, grouped[0].calls,
            "总计与分组的过滤口径必须一致"
        );
        assert_eq!(single[0].total_tokens, grouped[0].total_tokens);

        // 时间下界在未来：无匹配（总计返回一条零值行）
        let future = repo
            .stats(Some(i64::MAX - 1), None, None, None, "none")
            .await;
        assert_eq!(future.len(), 1);
        assert_eq!(future[0].calls, 0);
        assert_eq!(future[0].total_tokens, 0);
    }
}
