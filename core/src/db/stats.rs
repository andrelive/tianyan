//! 统计域仓储（Repository 层）。
//!
//! **SQL 收敛**：使用统计的数据访问从 `UsageStats` 收口于此；业务组件保留
//! 内存热路径计数（DashMap），落盘批量与查询经本仓储。

use std::sync::Arc;

use crate::common::error::TianyanError;

use crate::db::Database;

/// 技能调用统计数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillStats {
    pub skill_id: String,
    pub total_calls: u64,
    pub success_calls: u64,
    pub success_rate: f64,
    pub avg_time_ms: f64,
    pub last_called_at: String,
}

/// 文档访问统计数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocStats {
    pub uri: String,
    pub search_hits: u64,
    pub detail_loads: u64,
    pub avg_score: f64,
    pub last_hit_at: String,
}

/// 统计域仓储。
pub struct StatsRepo {
    db: Arc<Database>,
}

impl StatsRepo {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn sqlite_error(context: &str, e: rusqlite::Error) -> TianyanError {
        TianyanError::Custom(format!("observability: usage_stats: {context}：{e}"))
    }

    /// 落盘计数批量（技能/文档；单事务逐行展开保证统计真实）。
    pub async fn flush_counts(
        &self,
        skill_batch: &[(String, u64, u64, u64)],
        doc_batch: &[(String, u64, f64)],
    ) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| Self::sqlite_error("统计落盘", e))?;
        let now = chrono::Utc::now().to_rfc3339();
        for (skill_id, calls, successes, time) in skill_batch {
            let avg_time_us = if *calls > 0 { time / calls } else { 0 };
            for _ in 0..*successes {
                tx.execute(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,1,?2,?3)",
                    rusqlite::params![skill_id, avg_time_us, now],
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            }
            for _ in *successes..*calls {
                tx.execute(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,0,?2,?3)",
                    rusqlite::params![skill_id, avg_time_us, now],
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            }
        }
        for (uri, _hits, avg_score) in doc_batch {
            tx.execute(
                "INSERT INTO doc_access (uri, event_type, score, recorded_at) VALUES (?1,'search_hit',?2,?3)",
                rusqlite::params![uri, avg_score, now],
            )
            .map_err(|e| Self::sqlite_error("统计落盘", e))?;
        }
        tx.commit().map_err(|e| Self::sqlite_error("统计落盘", e))?;
        Ok(())
    }

    /// 关闭时优化（PRAGMA optimize；关闭刷盘由组件 flush 负责）。
    pub async fn optimize(&self) -> Result<(), TianyanError> {
        self.db
            .lock()
            .await
            .execute_batch("PRAGMA optimize;")
            .map_err(|e| Self::sqlite_error("统计优化", e))?;
        Ok(())
    }

    fn log_query_err<T>(result: Result<T, rusqlite::Error>, query: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(error = %e, query, "统计查询失败，返回空数据");
                None
            }
        }
    }

    pub async fn query_top_skills(&self, limit: usize) -> Vec<SkillStats> {
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT substr(skill_id, 7), COUNT(*), SUM(success),
                CAST(SUM(success) AS REAL) / MAX(COUNT(*),1),
                AVG(time_us)/1000.0, MAX(recorded_at)
         FROM skill_calls WHERE skill_id LIKE 'skill:%'
         GROUP BY skill_id ORDER BY 2 DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "统计查询 prepare 失败");
                return Vec::new();
            }
        };
        Self::log_query_err(
            stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(SkillStats {
                    skill_id: row.get(0)?,
                    total_calls: row.get::<_, i64>(1)? as u64,
                    success_calls: row.get::<_, i64>(2)? as u64,
                    success_rate: row.get(3)?,
                    avg_time_ms: row.get(4)?,
                    last_called_at: row.get(5)?,
                })
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect()),
            "query_top_skills",
        )
        .unwrap_or_default()
    }

    /// 查询访问最少的冷门文档列表。
    pub async fn query_cold_documents(&self, limit: usize) -> Vec<DocStats> {
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT uri, SUM(CASE WHEN event_type='search_hit' THEN 1 ELSE 0 END),
                SUM(CASE WHEN event_type='detail_load' THEN 1 ELSE 0 END),
                AVG(CASE WHEN event_type='search_hit' THEN score END), MAX(recorded_at)
         FROM doc_access GROUP BY uri ORDER BY 2 ASC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "统计查询 prepare 失败");
                return Vec::new();
            }
        };
        Self::log_query_err(
            stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(DocStats {
                    uri: row.get(0)?,
                    search_hits: row.get::<_, i64>(1)? as u64,
                    detail_loads: row.get::<_, i64>(2)? as u64,
                    avg_score: row.get(3)?,
                    last_hit_at: row.get(4)?,
                })
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect()),
            "query_cold_documents",
        )
        .unwrap_or_default()
    }

    /// 查询近 7 天的搜索命名空间热度分布。
    pub async fn query_search_heatmap(&self) -> serde_json::Value {
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT COALESCE(top_namespace,'unknown'), COUNT(*)
         FROM daily_search_queries WHERE recorded_at >= datetime('now','-7 days')
         GROUP BY 1 ORDER BY 2 DESC",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "统计查询 prepare 失败");
                return serde_json::json!({"period":"7d","namespaces":[]});
            }
        };
        let rows: Vec<_> = Self::log_query_err(
            stmt.query_map([], |row| {
                Ok(serde_json::json!({"namespace": row.get::<_, String>(0)?, "count": row.get::<_, i64>(1)?}))
            })
            .map(|r| r.filter_map(|r| r.ok()).collect()),
            "query_search_heatmap",
        )
        .unwrap_or_default();
        serde_json::json!({"period":"7d","namespaces":rows})
    }

    /// 查询全局统计概览（技能数、调用数、工具数、文档数、搜索数）。
    ///
    /// 口径：`skill_calls` 表同时记录真技能（`skill:<id>` 键）与普通工具
    /// （工具名键）。技能指标只统计 `skill:` 前缀，工具指标统计其余——
    /// 工具是工具，技能是技能。
    pub async fn query_summary(&self) -> serde_json::Value {
        let conn = self.db.lock().await;
        let skills: i64 = Self::log_query_err(
            conn.query_row(
                "SELECT COUNT(DISTINCT skill_id) FROM skill_calls WHERE skill_id LIKE 'skill:%'",
                [],
                |r| r.get(0),
            ),
            "query_summary.skills",
        )
        .unwrap_or(0);
        let calls: i64 = Self::log_query_err(
            conn.query_row(
                "SELECT COUNT(*) FROM skill_calls WHERE skill_id LIKE 'skill:%'",
                [],
                |r| r.get(0),
            ),
            "query_summary.calls",
        )
        .unwrap_or(0);
        let tools: i64 = Self::log_query_err(
            conn.query_row(
                "SELECT COUNT(DISTINCT skill_id) FROM skill_calls WHERE skill_id NOT LIKE 'skill:%'",
                [],
                |r| r.get(0),
            ),
            "query_summary.tools",
        )
        .unwrap_or(0);
        let tool_calls: i64 = Self::log_query_err(
            conn.query_row(
                "SELECT COUNT(*) FROM skill_calls WHERE skill_id NOT LIKE 'skill:%'",
                [],
                |r| r.get(0),
            ),
            "query_summary.tool_calls",
        )
        .unwrap_or(0);
        let docs: i64 = Self::log_query_err(
            conn.query_row("SELECT COUNT(DISTINCT uri) FROM doc_access", [], |r| {
                r.get(0)
            }),
            "query_summary.docs",
        )
        .unwrap_or(0);
        let searches: i64 = Self::log_query_err(
            conn.query_row("SELECT COUNT(*) FROM daily_search_queries", [], |r| {
                r.get(0)
            }),
            "query_summary.searches",
        )
        .unwrap_or(0);
        serde_json::json!({"total_skills_tracked":skills,"total_skill_calls":calls,"total_tools_tracked":tools,"total_tool_calls":tool_calls,"total_docs_tracked":docs,"total_searches":searches})
    }
}
