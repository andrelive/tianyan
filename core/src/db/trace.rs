//! Trace 域仓储（Repository 层）。
//!
//! **SQL 收敛**：执行 span（turn/tool/task）的落盘与查询从 TraceCollector 收口，
//! 组件保留内存缓冲。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::db::Database;

/// 一条执行 span（可持久化）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceSpan {
    /// 自增主键。
    pub id: i64,
    /// 会话 ID。
    pub session_id: String,
    /// 任务 ID（子代理/后台任务链路；无则 None）。
    pub task_id: Option<String>,
    /// 会话内轮次序号（无则 None）。
    pub turn_index: Option<i64>,
    /// span 类型（turn / tool / task 等）。
    pub kind: String,
    /// span 名称（工具名 / 阶段名）。
    pub name: String,
    /// 详情（参数摘要 / 结果摘要）。
    pub detail: String,
    /// 耗时（毫秒）。
    pub duration_ms: i64,
    /// Token 用量（无则 0）。
    pub tokens: i64,
    /// 是否成功。
    pub success: bool,
    /// 错误信息（失败时）。
    pub error: Option<String>,
    /// 记录时间（RFC3339）。
    pub recorded_at: String,
}

/// Trace 域仓储。
pub struct TraceRepo {
    db: Arc<Database>,
}

impl TraceRepo {
    /// 创建 Trace 仓储（共享 `Database` 单连接）。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 落盘 span 批量（trace_spans 表；组件 flush 调用）。
    ///
    /// 事务 RAII（T0-5）：用 rusqlite `Transaction`——任一条 INSERT 失败时
    /// Drop 自动 ROLLBACK（此前手写 `BEGIN` + 循环 `?` 提前返回，失败路径
    /// 无 ROLLBACK → 事务悬挂在连接上，后续写操作被卷入未结束事务）；
    /// 开启/提交失败均上抛（此前 `let _ =` 吞掉——COMMIT 失败会静默丢数据）。
    pub async fn flush_spans(&self, spans: &[TraceSpan]) -> Result<(), TianyanError> {
        let mut conn = self.db.lock().await;
        let tx = conn
            .transaction()
            .map_err(|e| TianyanError::Custom(format!("trace: 事务开启失败：{e}")))?;
        for span in spans {
            tx.execute(
                "INSERT INTO trace_spans (session_id, task_id, turn_index, kind, name, detail, duration_ms, tokens, success, error, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![span.session_id, span.task_id, span.turn_index, span.kind, span.name, span.detail, span.duration_ms, span.tokens, span.success as i32, span.error, span.recorded_at],
            )
            .map_err(|e| TianyanError::Custom(format!("trace: 落盘失败：{e}")))?;
        }
        tx.commit()
            .map_err(|e| TianyanError::Custom(format!("trace: 事务提交失败：{e}")))?;
        Ok(())
    }

    /// 查询 span（session/task 过滤；倒序限条）。
    pub async fn query(
        &self,
        session_id: Option<&str>,
        task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TraceSpan>, TianyanError> {
        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, task_id, turn_index, kind, name, detail, duration_ms, tokens, success, error, recorded_at FROM trace_spans WHERE (?1 IS NULL OR session_id = ?1) AND (?2 IS NULL OR task_id = ?2) ORDER BY id DESC LIMIT ?3",
            )
            .map_err(|e| TianyanError::Custom(format!("trace: 查询失败：{e}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, task_id, limit as i64],
                row_to_span,
            )
            .map_err(|e| TianyanError::Custom(format!("trace: 查询执行失败：{e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| TianyanError::Custom(format!("trace: 行读取失败：{e}")))?);
        }
        // 时间正序（回放顺序；SQL DESC + reverse——与原先 TraceCollector 语义一致）
        out.reverse();
        Ok(out)
    }
}

/// 行映射：数据库行 → TraceSpan。
fn row_to_span(row: &rusqlite::Row<'_>) -> rusqlite::Result<TraceSpan> {
    Ok(TraceSpan {
        id: row.get(0)?,
        session_id: row.get(1)?,
        task_id: row.get(2)?,
        turn_index: row.get(3)?,
        kind: row.get(4)?,
        name: row.get(5)?,
        detail: row.get(6)?,
        duration_ms: row.get(7)?,
        tokens: row.get(8)?,
        success: row.get(9)?,
        error: row.get(10)?,
        recorded_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_span(name: &str) -> TraceSpan {
        TraceSpan {
            id: 0,
            session_id: "s1".to_string(),
            task_id: None,
            turn_index: None,
            kind: "turn".to_string(),
            name: name.to_string(),
            detail: String::new(),
            duration_ms: 1,
            tokens: 0,
            success: true,
            error: None,
            recorded_at: "2026-09-15T00:00:00Z".to_string(),
        }
    }

    /// T0-5 主回归：写入失败时事务必须回滚（RAII）——
    /// 修复前手写 `BEGIN` + 循环 `?` 提前返回：失败路径没有 ROLLBACK，
    /// 事务悬挂在连接上（`is_autocommit() == false`），后续操作会被卷入
    /// 这个未结束事务（锁持有 + 数据一起提交/回滚的静默不一致）。
    #[tokio::test]
    async fn test_flush_spans_failure_rolls_back_transaction() {
        // 不建 schema：首条 INSERT 即失败（no such table）
        let db = Database::open_in_memory().unwrap();
        let repo = TraceRepo::new(db.clone());

        let err = repo
            .flush_spans(&[sample_span("t1")])
            .await
            .expect_err("表不存在，落盘应失败");
        assert!(err.to_string().contains("trace:"), "错误带模块前缀: {err}");

        // 关键断言：连接上不得残留活动事务
        let conn = db.lock().await;
        assert!(
            conn.is_autocommit(),
            "失败后不得保留悬挂事务（修复前 BEGIN 未配 ROLLBACK）"
        );
    }

    /// 回归：正常路径落盘 + 查询可读（且提交后无活动事务）。
    #[tokio::test]
    async fn test_flush_spans_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let repo = TraceRepo::new(db.clone());

        repo.flush_spans(&[sample_span("turn-1"), sample_span("turn-2")])
            .await
            .unwrap();

        let spans = repo.query(Some("s1"), None, 10).await.unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "turn-1");
        assert_eq!(spans[1].name, "turn-2");

        let conn = db.lock().await;
        assert!(conn.is_autocommit(), "提交后不得留活动事务");
    }
}
