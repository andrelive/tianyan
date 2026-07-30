//! 使用统计模块 —— 技能调用频率、文档访问热度的持久化追踪。
//!
//! 设计原则：
//! - **热路径零 I/O**：每次工具调用/文档检索只写内存 DashMap，不碰 SQLite
//! - **定时刷盘**：`flush()` 将内存计数器批量写入 SQLite
//! - **共享 SQLite**：与 SqliteSessionStore 共用同一个 SqliteDb

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;

use super::sqlite_db::SqliteDb;

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
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, rusqlite::Error> {
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
            Err(_) => return,
        };
        let _ = conn.execute(
            "INSERT INTO daily_search_queries (query_text, result_count, top_namespace) VALUES (?1,?2,?3)",
            rusqlite::params![query_text, result_count as i64, top_namespace],
        );
    }

    // ── 持久化 ─────────────────────────────────────────────────────

    /// 将内存计数器批量刷入 SQLite。
    pub async fn flush(&self) -> Result<(), rusqlite::Error> {
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
        let tx = conn.unchecked_transaction()?;
        let now = Utc::now().to_rfc3339();
        for (skill_id, calls, _succ, time) in &skill_batch {
            tx.execute(
                "INSERT INTO skill_calls (skill_id, success, time_us, recorded_at) VALUES (?1,1,?2,?3)",
                rusqlite::params![skill_id, if *calls > 0 { time / calls } else { 0 }, now],
            )?;
        }
        for (uri, _hits, avg_score) in &doc_batch {
            tx.execute(
                "INSERT INTO doc_access (uri, event_type, score, recorded_at) VALUES (?1,'search_hit',?2,?3)",
                rusqlite::params![uri, avg_score, now],
            )?;
        }
        tx.commit()?;
        self.pending_writes.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// 关闭统计模块，先刷盘再执行 PRAGMA optimize。
    pub async fn shutdown(&self) -> Result<(), rusqlite::Error> {
        self.flush().await?;
        self.db.lock().await.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }

    // ── 查询 API ─────────────────────────────────────────────────

    /// 查询调用次数最多的技能列表。
    pub async fn query_top_skills(&self, limit: usize) -> Vec<SkillStats> {
        let conn = self.db.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT skill_id, COUNT(*), SUM(success),
                CAST(SUM(success) AS REAL) / MAX(COUNT(*),1),
                AVG(time_us)/1000.0, MAX(recorded_at)
         FROM skill_calls GROUP BY skill_id ORDER BY 2 DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
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
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
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
            Err(_) => return Vec::new(),
        };
        stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok(DocStats {
                uri: row.get(0)?,
                search_hits: row.get::<_, i64>(1)? as u64,
                detail_loads: row.get::<_, i64>(2)? as u64,
                avg_score: row.get(3)?,
                last_hit_at: row.get(4)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
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
            Err(_) => return serde_json::json!({"period":"7d","namespaces":[]}),
        };
        let rows: Vec<_> = stmt
            .query_map([], |row| {
                Ok(serde_json::json!({"namespace": row.get::<_, String>(0)?, "count": row.get::<_, i64>(1)?}))
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        serde_json::json!({"period":"7d","namespaces":rows})
    }

    /// 查询全局统计概览（技能数、调用数、文档数、搜索数）。
    pub async fn query_summary(&self) -> serde_json::Value {
        let conn = self.db.lock().await;
        let skills: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT skill_id) FROM skill_calls",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let calls: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_calls", [], |r| r.get(0))
            .unwrap_or(0);
        let docs: i64 = conn
            .query_row("SELECT COUNT(DISTINCT uri) FROM doc_access", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let searches: i64 = conn
            .query_row("SELECT COUNT(*) FROM daily_search_queries", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        serde_json::json!({"total_skills_tracked":skills,"total_skill_calls":calls,"total_docs_tracked":docs,"total_searches":searches})
    }
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
        stats.record_skill_call("read_file", true, 5000);
        stats.record_skill_call("read_file", false, 10000);
        stats.record_skill_call("write_file", true, 3000);
        stats.flush().await.unwrap();
        let top = stats.query_top_skills(10).await;
        assert_eq!(top.len(), 2);
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
}
