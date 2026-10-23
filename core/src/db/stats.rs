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
    /// 技能 ID（已剥离 `skill:` 前缀，与注册名对齐）。
    pub skill_id: String,
    /// 累计调用次数。
    pub total_calls: u64,
    /// 成功调用次数。
    pub success_calls: u64,
    /// 成功率（success_calls / total_calls）。
    pub success_rate: f64,
    /// 平均耗时（毫秒）。
    pub avg_time_ms: f64,
    /// 最后一次调用时间（RFC3339）。
    pub last_called_at: String,
}

/// 文档访问统计数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocStats {
    /// 文档 URI。
    pub uri: String,
    /// 检索命中次数。
    pub search_hits: u64,
    /// 详情加载次数。
    pub detail_loads: u64,
    /// 平均相关性得分。
    pub avg_score: f64,
    /// 最后一次命中时间（RFC3339）。
    pub last_hit_at: String,
}

/// 统计域仓储。
pub struct StatsRepo {
    db: Arc<Database>,
}

impl StatsRepo {
    /// 创建统计仓储（共享 `Database` 单连接）。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn sqlite_error(context: &str, e: rusqlite::Error) -> TianyanError {
        TianyanError::Custom(format!("observability: usage_stats: {context}：{e}"))
    }

    /// 落盘计数批量（技能/文档；单事务逐行展开保证统计真实）。
    ///
    /// 行数由调用次数决定（每次调用一行），故逐行插入是语义要求；但语句
    /// **只 prepare 一次**（复用 statement），避免每次 execute 重复解析 SQL。
    ///
    /// 文档批次为 `(uri, 命中次数, 详情加载次数, 平均分)`：两种事件各按次数
    /// 展开（`search_hit` 带平均分；`detail_load` 的 score 记 0——详情加载无
    /// 相关性得分）。**detail_load 此前从不落库**（读取端 `detail_loads` 恒 0，
    /// T1-13）。
    pub async fn flush_counts(
        &self,
        skill_batch: &[(String, u64, u64, u64)],
        doc_batch: &[(String, u64, u64, f64)],
    ) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| Self::sqlite_error("统计落盘", e))?;
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut stmt_success = tx
                .prepare(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,1,?2,?3)",
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            let mut stmt_failure = tx
                .prepare(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,0,?2,?3)",
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            for (skill_id, calls, successes, time) in skill_batch {
                let avg_time_us = if *calls > 0 { time / calls } else { 0 };
                for _ in 0..*successes {
                    stmt_success
                        .execute(rusqlite::params![skill_id, avg_time_us, &now])
                        .map_err(|e| Self::sqlite_error("统计落盘", e))?;
                }
                for _ in *successes..*calls {
                    stmt_failure
                        .execute(rusqlite::params![skill_id, avg_time_us, &now])
                        .map_err(|e| Self::sqlite_error("统计落盘", e))?;
                }
            }
            let mut stmt_doc_hit = tx
                .prepare(
                    "INSERT INTO doc_access (uri, event_type, score, recorded_at) VALUES (?1,'search_hit',?2,?3)",
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            let mut stmt_doc_load = tx
                .prepare(
                    "INSERT INTO doc_access (uri, event_type, score, recorded_at) VALUES (?1,'detail_load',0,?2)",
                )
                .map_err(|e| Self::sqlite_error("统计落盘", e))?;
            for (uri, hits, loads, avg_score) in doc_batch {
                for _ in 0..*hits {
                    stmt_doc_hit
                        .execute(rusqlite::params![uri, avg_score, &now])
                        .map_err(|e| Self::sqlite_error("统计落盘", e))?;
                }
                for _ in 0..*loads {
                    stmt_doc_load
                        .execute(rusqlite::params![uri, &now])
                        .map_err(|e| Self::sqlite_error("统计落盘", e))?;
                }
            }
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

    /// 查询调用次数最多的技能（按调用次数倒序；只统计 `skill:` 前缀键）。
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

    /// 按时间窗口查询技能调用统计（演化任务材料用）。
    ///
    /// 与 [`Self::query_top_skills`] 的差别**只有时间窗口**：后者是累计口径（前端技能面板用），
    /// 本方法只统计最近 `days` 天。`recorded_at` 历史上存在两种格式（写入走 RFC3339，
    /// 表默认值 `datetime('now')`），因此过滤用 SQLite 的 ` datetime(recorded_at)` **归一化后再比**
    /// ——直接字符串比较会在「同一天内的两种格式」上判错（' ' 与 'T' 的码位差）。
    pub async fn query_top_skills_since(&self, days: u32, limit: usize) -> Vec<SkillStats> {
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT substr(skill_id, 7), COUNT(*), SUM(success),
                CAST(SUM(success) AS REAL) / MAX(COUNT(*),1),
                AVG(time_us)/1000.0, MAX(recorded_at)
         FROM skill_calls WHERE skill_id LIKE 'skill:%'
           AND datetime(recorded_at) >= datetime('now', ?2)
         GROUP BY skill_id ORDER BY 2 DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "统计查询 prepare 失败");
                return Vec::new();
            }
        };
        let window = format!("-{days} days");
        Self::log_query_err(
            stmt.query_map(rusqlite::params![limit as i64, window], |row| {
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
            "query_top_skills_since",
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
                    // 只有 detail_load（无 search_hit）的 URI：AVG 为 NULL——
                    // 直接 `get::<f64>` 会报错并被 filter_map 静默丢弃该行（T1-13）
                    avg_score: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存库 + 已初始化 schema 的仓储（与 `usage_stats::tests::setup` 同款）。
    async fn setup() -> (Arc<Database>, StatsRepo) {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let repo = StatsRepo::new(db.clone());
        (db, repo)
    }

    /// 时间窗口：`query_top_skills_since` 只统计窗口内的调用，累计口径
    /// （`query_top_skills`）不受影响。
    ///
    /// 判别力：窗口外的记录用 `datetime('now','-40 days')`（**表默认值格式**）落盘，
    /// 窗口内的用 `flush_counts` 的 RFC3339 写入路径——两种格式并存是生产实况
    /// （历史行是 `datetime('now')`、新行是 RFC3339）。过滤必须归一化后比较。
    #[tokio::test]
    async fn test_query_top_skills_since_filters_by_window() {
        let (db, repo) = setup().await;
        // 两条近期调用（RFC3339 写入路径）：1 成功 1 失败
        repo.flush_counts(&[("skill:code_review".to_string(), 2, 1, 12_000)], &[])
            .await
            .unwrap();
        // 一条 40 天前的调用（改成表默认值的 `datetime('now')` 格式）
        repo.flush_counts(&[("skill:legacy".to_string(), 1, 1, 3_000)], &[])
            .await
            .unwrap();
        db.lock()
            .await
            .execute(
                "UPDATE skill_calls SET recorded_at = datetime('now','-40 days')
                 WHERE skill_id = 'skill:legacy'",
                [],
            )
            .unwrap();

        let recent = repo.query_top_skills_since(30, 10).await;
        assert_eq!(recent.len(), 1, "40 天前的调用应落在窗口外: {recent:?}");
        assert_eq!(recent[0].skill_id, "code_review", "应剥离 skill: 前缀");
        assert_eq!(recent[0].total_calls, 2);
        assert_eq!(recent[0].success_calls, 1);
        assert!(
            (recent[0].avg_time_ms - 6.0).abs() < 1e-6,
            "均耗时应为 12000us/2/1000 = 6ms，实际 {}",
            recent[0].avg_time_ms
        );

        // 累计口径（前端技能面板）两种都含——`query_top_skills` 语义未变
        let all = repo.query_top_skills(10).await;
        assert_eq!(all.len(), 2, "累计口径应含窗口外的历史调用: {all:?}");
        assert!(all.iter().any(|s| s.skill_id == "legacy"));
    }

    /// 窗口宽度是参数：40 天前的记录在 90 天窗口内可见（不是恒为「近 30 天」）。
    #[tokio::test]
    async fn test_query_top_skills_since_window_is_parameter() {
        let (db, repo) = setup().await;
        repo.flush_counts(&[("skill:old".to_string(), 1, 1, 1_000)], &[])
            .await
            .unwrap();
        db.lock()
            .await
            .execute(
                "UPDATE skill_calls SET recorded_at = datetime('now','-40 days')",
                [],
            )
            .unwrap();
        assert!(
            repo.query_top_skills_since(30, 10).await.is_empty(),
            "30 天窗口应排除 40 天前的调用"
        );
        assert_eq!(
            repo.query_top_skills_since(90, 10).await.len(),
            1,
            "90 天窗口应包含 40 天前的调用"
        );
    }
}
