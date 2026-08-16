//! 使用统计模块 —— 技能调用频率、文档访问热度的持久化追踪。
//!
//! 设计原则：
//! - **热路径零 I/O**：每次工具调用/文档检索只写内存 DashMap，不碰 SQLite
//! - **定时刷盘**：`flush()` 将内存计数器批量写入 SQLite
//! - **共享 SQLite**：与 VFS 元数据共用同一个 SqliteDb

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;

use crate::common::error::TianyanError;
use crate::common::types::retrieval_trace::RetrievalTrace;
use crate::vfs::backend::sqlite_db::SqliteDb;

#[derive(Default)]
struct SkillCounter {
    calls: AtomicU64,
    successes: AtomicU64,
    total_time_us: AtomicU64,
}

#[derive(Default)]
struct DocCounter {
    search_hits: AtomicU64,
    detail_loads: AtomicU64,
    total_score: AtomicU64,
}

/// 技能调用统计数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillStats {
    /// 技能唯一标识。
    pub skill_id: String,
    /// 调用总次数。
    pub total_calls: u64,
    /// 成功调用次数。
    pub success_calls: u64,
    /// 成功率（0.0 ~ 1.0）。
    pub success_rate: f64,
    /// 平均耗时（毫秒）。
    pub avg_time_ms: f64,
    /// 最后调用时间（RFC3339）。
    pub last_called_at: String,
}

/// 文档访问统计数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocStats {
    /// 文档 URI。
    pub uri: String,
    /// 搜索命中的次数。
    pub search_hits: u64,
    /// 详情加载的次数。
    pub detail_loads: u64,
    /// 搜索命中时的平均相关性得分。
    pub avg_score: f64,
    /// 最后命中时间（RFC3339）。
    pub last_hit_at: String,
}

/// 使用统计追踪器。
pub struct UsageStats {
    skill_counters: DashMap<String, Arc<SkillCounter>>,
    doc_counters: DashMap<String, Arc<DocCounter>>,
    db: SqliteDb,
    pending_writes: AtomicU64,
}

impl UsageStats {
    /// 创建新的 UsageStats 实例，使用共享的 SqliteDb 连接。
    ///
    /// # Errors
    /// * 返回 `TianyanError`（本方法当前不失败，签名保持与全库统一错误类型）。
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self {
            skill_counters: DashMap::new(),
            doc_counters: DashMap::new(),
            db,
            pending_writes: AtomicU64::new(0),
        }))
    }

    // ── 热路径记录 ─────────────────────────────────────────────────

    /// 记录一次技能调用（热路径，只写内存）。
    pub fn record_skill_call(&self, skill_id: &str, success: bool, time_us: u64) {
        let counter = self
            .skill_counters
            .entry(skill_id.to_string())
            .or_default()
            .clone();
        counter.calls.fetch_add(1, Ordering::Relaxed);
        if success {
            counter.successes.fetch_add(1, Ordering::Relaxed);
        }
        counter.total_time_us.fetch_add(time_us, Ordering::Relaxed);
        self.pending_writes.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次文档搜索命中（热路径，只写内存）。
    pub fn record_doc_hit(&self, uri: &str, score: f32) {
        let counter = self
            .doc_counters
            .entry(uri.to_string())
            .or_default()
            .clone();
        counter.search_hits.fetch_add(1, Ordering::Relaxed);
        let score_fixed = (score as f64 * 1_000_000.0) as u64;
        counter
            .total_score
            .fetch_add(score_fixed, Ordering::Relaxed);
        self.pending_writes.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次文档详情加载（热路径，只写内存）。
    pub fn record_doc_load(&self, uri: &str) {
        let counter = self
            .doc_counters
            .entry(uri.to_string())
            .or_default()
            .clone();
        counter.detail_loads.fetch_add(1, Ordering::Relaxed);
        self.pending_writes.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次搜索查询（直接写入 SQLite，非热路径）。
    pub fn record_search_query(
        &self,
        query_text: &str,
        result_count: usize,
        top_namespace: Option<&str>,
    ) {
        let conn = match self.db.try_lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "统计存储错误：搜索查询跳过（数据库锁不可用）");
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO daily_search_queries (query_text, result_count, top_namespace) VALUES (?1,?2,?3)",
            rusqlite::params![query_text, result_count as i64, top_namespace],
        ) {
            tracing::warn!(error = %e, "统计存储错误：搜索查询写入失败");
        }
    }

    /// 记录一次检索轨迹（完整过程快照，非热路径）。
    ///
    /// 轨迹保留最近 [`MAX_RETRIEVAL_TRACES`] 条，超出删除最旧（FIFO）。
    pub fn record_retrieval_trace(&self, trace: &RetrievalTrace) {
        let trace_json = match serde_json::to_string(trace) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "统计存储错误：轨迹序列化失败");
                return;
            }
        };
        let conn = match self.db.try_lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "统计存储错误：检索轨迹跳过（数据库锁不可用）");
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO retrieval_traces (query, trace_json, total_tokens, total_time_ms) VALUES (?1,?2,?3,?4)",
            rusqlite::params![trace.query, trace_json, trace.total_tokens as i64, trace.total_time_ms as i64],
        ) {
            tracing::warn!(error = %e, "统计存储错误：检索轨迹写入失败");
            return;
        }
        if let Err(e) = conn.execute(
            "DELETE FROM retrieval_traces WHERE id NOT IN (SELECT id FROM retrieval_traces ORDER BY id DESC LIMIT ?1)",
            rusqlite::params![MAX_RETRIEVAL_TRACES],
        ) {
            tracing::warn!(error = %e, "统计存储错误：检索轨迹清理失败");
        }
    }

    // ── 持久化 ─────────────────────────────────────────────────────

    /// 将内存计数器批量刷入 SQLite。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 `TianyanError::Custom`（带 `observability` 前缀）。
    pub async fn flush(&self) -> Result<(), TianyanError> {
        let skill_batch: Vec<_> = self
            .skill_counters
            .iter()
            .filter_map(|e| {
                let c = e.value();
                let calls = c.calls.swap(0, Ordering::Relaxed);
                let successes = c.successes.swap(0, Ordering::Relaxed);
                let time = c.total_time_us.swap(0, Ordering::Relaxed);
                (calls > 0).then(|| (e.key().clone(), calls, successes, time))
            })
            .collect();

        let doc_batch: Vec<_> = self
            .doc_counters
            .iter()
            .filter_map(|e| {
                let c = e.value();
                let hits = c.search_hits.swap(0, Ordering::Relaxed);
                let score = c.total_score.swap(0, Ordering::Relaxed);
                (hits > 0).then(|| {
                    (
                        e.key().clone(),
                        hits,
                        score as f64 / hits as f64 / 1_000_000.0,
                    )
                })
            })
            .collect();

        if skill_batch.is_empty() && doc_batch.is_empty() {
            self.pending_writes.store(0, Ordering::Relaxed);
            return Ok(());
        }

        let conn = self.db.lock().await;
        let tx = conn.unchecked_transaction().map_err(sqlite_error)?;
        let now = Utc::now().to_rfc3339();
        for (skill_id, calls, successes, time) in &skill_batch {
            // 成功计数真实落库：按调用次数逐行展开（此前每批聚合只插
            // 一行且 success 硬编码 1——失败调用被记为成功，成功率恒为
            // 100%、执行异常统计失真）。逐行展开保证 COUNT(*)/SUM(success)
            // 与真实调用一一对应；批量 flush 在单事务内，开销可接受。
            let avg_time_us = if *calls > 0 { time / calls } else { 0 };
            for _ in 0..*successes {
                tx.execute(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,1,?2,?3)",
                    rusqlite::params![skill_id, avg_time_us, now],
                )
                .map_err(sqlite_error)?;
            }
            for _ in *successes..*calls {
                tx.execute(
                    "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,0,?2,?3)",
                    rusqlite::params![skill_id, avg_time_us, now],
                )
                .map_err(sqlite_error)?;
            }
        }
        for (uri, _hits, avg_score) in &doc_batch {
            tx.execute(
                "INSERT INTO doc_access (uri, event_type, score, recorded_at) VALUES (?1,'search_hit',?2,?3)",
                rusqlite::params![uri, avg_score, now],
            )
            .map_err(sqlite_error)?;
        }
        tx.commit().map_err(sqlite_error)?;
        self.pending_writes.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// 关闭统计模块，先刷盘再执行 PRAGMA optimize。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 `TianyanError::Custom`（带 `observability` 前缀）。
    pub async fn shutdown(&self) -> Result<(), TianyanError> {
        self.flush().await?;
        self.db
            .lock()
            .await
            .execute_batch("PRAGMA optimize;")
            .map_err(sqlite_error)?;
        Ok(())
    }

    // ── 查询 API ─────────────────────────────────────────────────

    /// 查询前刷盘：把内存中的热路径计数先落库，保证查询看到最新数据
    /// （与 [`crate::observability::trace::TraceCollector`] 的"查询前自动落库"一致）。
    ///
    /// 刷盘失败不阻断查询——降级为返回已落库数据，错误仅告警。
    async fn flush_pending(&self) {
        if let Err(e) = self.flush().await {
            tracing::warn!(error = %e, "统计查询前刷盘失败，返回已落库数据");
        }
    }

    /// 记录 SQL 查询错误（统计查询失败时降级返回空数据，但错误必须可见）。
    fn log_query_err<T>(result: Result<T, rusqlite::Error>, query: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(error = %e, query, "统计查询失败，返回空数据");
                None
            }
        }
    }

    /// 查询调用次数最多的技能列表。
    ///
    /// 口径：只统计真技能（`skill:` 前缀键），普通工具调用不混入；
    /// 返回的 skill_id 剥离前缀（与技能注册名对齐，如 `planning`）。
    pub async fn query_top_skills(&self, limit: usize) -> Vec<SkillStats> {
        self.flush_pending().await;
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
        self.flush_pending().await;
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
        self.flush_pending().await;
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

    /// 查询最近的检索轨迹（完整过程快照，供调试界面使用）。
    ///
    /// 返回按时间倒序的最新 `limit` 条；轨迹 JSON 解析失败的行被跳过。
    pub async fn query_recent_traces(&self, limit: usize) -> Vec<RetrievalTrace> {
        let conn = self.db.lock().await;
        let mut stmt = match conn
            .prepare("SELECT trace_json FROM retrieval_traces ORDER BY id DESC LIMIT ?1")
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "统计查询失败：检索轨迹 prepare 失败");
                return Vec::new();
            }
        };
        let rows: Vec<String> = match stmt.query_map(rusqlite::params![limit as i64], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "统计查询失败：检索轨迹查询失败");
                return Vec::new();
            }
        };
        rows.iter()
            .filter_map(|j| serde_json::from_str::<RetrievalTrace>(j).ok())
            .collect()
    }
}

/// 检索轨迹保留上限（FIFO 淘汰）。
const MAX_RETRIEVAL_TRACES: i64 = 500;

/// 将 rusqlite 错误映射为 `TianyanError::Custom`（统一 observability 模块前缀）。
fn sqlite_error(e: rusqlite::Error) -> TianyanError {
    TianyanError::Custom(format!("observability: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Arc<UsageStats> {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        UsageStats::new(db).unwrap()
    }

    #[tokio::test]
    async fn test_record_and_flush_skill() {
        let stats = setup().await;
        stats.record_skill_call("skill:planning", true, 5000);
        stats.record_skill_call("skill:planning", false, 10000);
        stats.record_skill_call("skill:file_read", true, 3000);
        stats.record_skill_call("read_file", true, 3000); // 工具调用，不计入技能
        stats.flush().await.unwrap();
        let top = stats.query_top_skills(10).await;
        assert_eq!(top.len(), 2, "只有 skill: 前缀计入技能");
        // 前缀剥离：返回 planning 而非 skill:planning
        assert!(top.iter().any(|s| s.skill_id == "planning"));
        assert!(!top.iter().any(|s| s.skill_id.contains(":")));
        // planning 1 成功 1 失败 → 成功率 0.5
        let p = top.iter().find(|s| s.skill_id == "planning").unwrap();
        assert_eq!(p.total_calls, 2);
        assert_eq!(p.success_calls, 1);
    }

    #[tokio::test]
    async fn test_summary_splits_skills_and_tools() {
        // 口径：工具是工具，技能是技能——summary 分别计数
        let stats = setup().await;
        stats.record_skill_call("skill:planning", true, 5000);
        stats.record_skill_call("skill:planning", true, 3000);
        stats.record_skill_call("skill:code_review", true, 2000);
        stats.record_skill_call("read_file", true, 4000);
        stats.record_skill_call("execute_command", false, 6000);
        let s = stats.query_summary().await;
        assert_eq!(s["total_skills_tracked"], 2, "2 个不同技能");
        assert_eq!(s["total_skill_calls"], 3, "技能调用 3 次");
        assert_eq!(s["total_tools_tracked"], 2, "2 个不同工具");
        assert_eq!(s["total_tool_calls"], 2, "工具调用 2 次");
    }

    #[tokio::test]
    async fn test_doc_hit_and_cold_query() {
        let stats = setup().await;
        stats.record_doc_hit("tianyan://knowledge/rust.md", 0.85);
        stats.record_doc_hit("tianyan://knowledge/python.md", 0.45);
        stats.flush().await.unwrap();
        assert!(!stats.query_cold_documents(10).await.is_empty());
    }

    #[tokio::test]
    async fn test_heatmap() {
        let stats = setup().await;
        stats.record_search_query("async rust", 5, Some("knowledge"));
        let hm = stats.query_search_heatmap().await;
        assert!(!hm["namespaces"].as_array().unwrap().is_empty());
    }

    fn sample_trace(query: &str) -> RetrievalTrace {
        use crate::common::types::retrieval_trace::{RetrievalStep, RetrievalStepType};
        use crate::common::types::TianyanUri;
        RetrievalTrace {
            query: query.to_string(),
            steps: vec![RetrievalStep {
                step_type: RetrievalStepType::IntentAnalysis,
                target_uri: TianyanUri::parse("tianyan://knowledge/x").unwrap(),
                score: None,
                tokens_used: 10,
                timestamp: Utc::now(),
            }],
            results: vec![],
            total_tokens: 10,
            total_time_ms: 5,
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_retrieval_trace_roundtrip() {
        let stats = setup().await;
        stats.record_retrieval_trace(&sample_trace("为什么没找到"));
        let traces = stats.query_recent_traces(10).await;
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].query, "为什么没找到");
        assert_eq!(traces[0].steps.len(), 1);
    }

    #[tokio::test]
    async fn test_retrieval_trace_fifo_cap() {
        let stats = setup().await;
        for i in 0..(MAX_RETRIEVAL_TRACES + 10) {
            stats.record_retrieval_trace(&sample_trace(&format!("query-{i}")));
        }
        let traces = stats.query_recent_traces(1000).await;
        assert_eq!(
            traces.len() as i64,
            MAX_RETRIEVAL_TRACES,
            "超出上限应 FIFO 淘汰"
        );
        // 最旧的 10 条（query-0..query-9）应被淘汰，query-10 起保留
        assert!(traces.iter().any(|t| t.query == "query-10"));
        assert!(!traces.iter().any(|t| t.query == "query-9"));
    }

    #[test]
    fn test_new_error_type_is_tianyan_error() {
        // 契约：UsageStats::new 的错误类型必须是 TianyanError（不再泄漏 rusqlite::Error）
        let db = SqliteDb::open_in_memory().unwrap();
        let result: Result<Arc<UsageStats>, TianyanError> = UsageStats::new(db);
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_bad_db_path_fails() {
        // 坏路径：父目录位置被普通文件占用 → SqliteDb::open 失败（rusqlite::Error，边界类型不变）
        let tmp = std::env::temp_dir().join(format!("tianyan_bad_db_{}", std::process::id()));
        std::fs::write(&tmp, b"x").unwrap();
        let bad_path = tmp.join("nested").join("tianyan.db");
        let result = SqliteDb::open(bad_path);
        let _ = std::fs::remove_file(&tmp);
        assert!(result.is_err(), "坏数据库路径应打开失败");
    }

    #[tokio::test]
    async fn test_flush_error_maps_to_tianyan_error() {
        // 未初始化 schema 的数据库 → 刷盘失败，错误必须映射为 TianyanError::Custom
        let db = SqliteDb::open_in_memory().unwrap(); // 故意不调用 init_all_schemas
        let stats = UsageStats::new(db).unwrap();
        stats.record_skill_call("read_file", true, 100);
        let err = stats.flush().await.unwrap_err();
        match err {
            TianyanError::Custom(msg) => assert!(
                msg.contains("observability"),
                "flush 失败错误消息应带 observability 前缀，实际: {msg}"
            ),
            other => panic!("flush 失败应映射为 TianyanError::Custom，实际: {other:?}"),
        }
    }
}
