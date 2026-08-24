//! 结构化 Trace（G6）：span 树持久化与回放。
//!
//! 记录主循环轮次（turn）、工具调用、后台任务的执行 span，落 SQLite
//! 支持事后回放与坏例分析。设计原则（与 UsageStats 一致）：
//! - **热路径零 I/O**：记录只写内存缓冲，不碰 SQLite
//! - **定时/查询前刷盘**：`flush()` 批量写入，查询前自动落库
//! - **共享 SQLite**：与 VFS 元数据共用同一个 SqliteDb
//!
//! 回放模型：span 按 `(session_id, task_id, turn_index)` 分组还原调用树——
//! 主循环轮次（kind=turn）下挂工具调用（kind=tool），后台任务独立成树
//! （kind=task，task_id 关联）。不依赖严格 parent 链，第一版足够还原
//! "哪个会话/任务在什么顺序上做了什么"。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::common::error::TianyanError;
use crate::vfs::backend::sqlite_db::SqliteDb;

/// span 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// 主循环轮次（一次 LLM 调用 + 工具执行）。
    Turn,
    /// 单次工具调用。
    Tool,
    /// 后台任务（含 delegate 子树根）。
    Task,
}

impl SpanKind {
    fn as_str(&self) -> &'static str {
        match self {
            SpanKind::Turn => "turn",
            SpanKind::Tool => "tool",
            SpanKind::Task => "task",
        }
    }
}

/// 一条执行 span（可持久化）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceSpan {
    /// 自增 ID（回放排序键）。
    pub id: i64,
    /// 归属会话。
    pub session_id: String,
    /// 后台任务 ID（task 类型必填；turn/tool 为 None）。
    pub task_id: Option<String>,
    /// 主循环轮次（turn/tool 使用；后台任务为 None）。
    pub turn_index: Option<i64>,
    /// span 类型（turn | tool | task）。
    pub kind: String,
    /// 名称（turn=模型名 / tool=工具名 / task=任务描述）。
    pub name: String,
    /// 详情（tool=参数摘要 / task=结果摘要；turn 为空）。
    pub detail: String,
    /// 耗时（毫秒）。
    pub duration_ms: i64,
    /// token 消耗（turn 使用；tool/task 为 0）。
    pub tokens: i64,
    /// 是否成功。
    pub success: bool,
    /// 错误信息（失败时）。
    pub error: Option<String>,
    /// 记录时间（RFC3339）。
    pub recorded_at: String,
}

/// 内存缓冲上限（超过后丢弃最旧，等待 flush 落库）。
const MAX_BUFFER: usize = 5000;
/// SQLite 全局保留窗口（超过后旧记录被清理）。
const KEEP_RECORDS: i64 = 10_000;

/// 结构化 Trace 收集器：热路径内存缓冲 + 批量落库 + 回放查询。
pub struct TraceCollector {
    buffer: Mutex<Vec<TraceSpan>>,
    /// session → 当前轮次索引（主循环轮边界由 AgentLoop 设置；
    /// 工具 span 归属当前轮；跨会话并发按键隔离）。
    current_turns: Mutex<HashMap<String, i64>>,
    db: SqliteDb,
    pending_writes: AtomicU64,
}

impl TraceCollector {
    /// 创建收集器（共享 SqliteDb；表结构由 `init_all_schemas` 统一创建）。
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self {
            buffer: Mutex::new(Vec::new()),
            current_turns: Mutex::new(HashMap::new()),
            db,
            pending_writes: AtomicU64::new(0),
        }))
    }

    // ── 热路径记录 ─────────────────────────────────────────────

    /// 设置会话当前轮次（AgentLoop 每轮开始时调用）。
    pub fn set_current_turn(&self, session_id: &str, turn: i64) {
        let _ = self
            .current_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string(), turn);
    }

    /// 记录主循环轮次 span（轮结束，含 token 消耗）。
    pub fn record_turn(
        &self,
        session_id: &str,
        model: &str,
        duration_ms: i64,
        tokens: i64,
        success: bool,
    ) {
        let turn = self
            .current_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .copied();
        let mut span = TraceSpan {
            id: 0,
            session_id: session_id.to_string(),
            task_id: None,
            turn_index: turn,
            kind: SpanKind::Turn.as_str().to_string(),
            name: model.to_string(),
            detail: String::new(),
            duration_ms,
            tokens,
            success,
            error: None,
            recorded_at: Utc::now().to_rfc3339(),
        };
        if !success {
            span.error = Some("轮次未正常完成".to_string());
        }
        self.record(span);
    }

    /// 记录工具调用 span（execute_single 结束时调用；归属当前轮）。
    pub fn record_tool(
        &self,
        session_id: &str,
        tool_name: &str,
        params_summary: &str,
        duration_ms: i64,
        success: bool,
        error: Option<String>,
    ) {
        let turn = self
            .current_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .copied();
        self.record(TraceSpan {
            id: 0,
            session_id: session_id.to_string(),
            task_id: None,
            turn_index: turn,
            kind: SpanKind::Tool.as_str().to_string(),
            name: tool_name.to_string(),
            detail: params_summary.to_string(),
            duration_ms,
            tokens: 0,
            success,
            error: error.map(|e| crate::common::truncate::truncate_utf8_boundary(&e, 300)),
            recorded_at: Utc::now().to_rfc3339(),
        });
    }

    /// 记录后台任务 span（finish 时调用；独立子树根）。
    ///
    /// # 参数
    /// - `session_id`：归属会话
    /// - `task_id`：后台任务 ID
    /// - `description`：任务描述（span 名称）
    /// - `result_summary`：结果摘要（span 详情）
    /// - `duration_ms`：任务耗时（毫秒）
    /// - `success`：是否成功
    /// - `error`：失败原因（失败时）
    #[allow(clippy::too_many_arguments)]
    pub fn record_task(
        &self,
        session_id: &str,
        task_id: &str,
        description: &str,
        result_summary: &str,
        duration_ms: i64,
        success: bool,
        error: Option<String>,
    ) {
        self.record(TraceSpan {
            id: 0,
            session_id: session_id.to_string(),
            task_id: Some(task_id.to_string()),
            turn_index: None,
            kind: SpanKind::Task.as_str().to_string(),
            name: description.to_string(),
            detail: crate::common::truncate::truncate_utf8_boundary(result_summary, 500),
            duration_ms,
            tokens: 0,
            success,
            error: error.map(|e| crate::common::truncate::truncate_utf8_boundary(&e, 300)),
            recorded_at: Utc::now().to_rfc3339(),
        });
    }

    /// 写入缓冲（热路径：无锁竞争最小化；超限丢弃最旧）。
    fn record(&self, span: TraceSpan) {
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buffer.push(span);
        if buffer.len() > MAX_BUFFER {
            let overflow = buffer.len() - MAX_BUFFER;
            buffer.drain(..overflow);
        }
        self.pending_writes.fetch_add(1, Ordering::Relaxed);
    }

    // ── 持久化与查询 ───────────────────────────────────────────

    /// 缓冲批量写入 SQLite（事务），并清理超出保留窗口的旧记录。
    pub async fn flush(&self) -> Result<(), TianyanError> {
        let spans: Vec<TraceSpan> = {
            let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buffer)
        };
        if spans.is_empty() {
            return Ok(());
        }

        let conn = self.db.lock().await;
        let result = conn.execute("BEGIN", []);
        let _ = result;
        for span in &spans {
            let res = conn.execute(
                "INSERT INTO trace_spans
                    (session_id, task_id, turn_index, kind, name, detail,
                     duration_ms, tokens, success, error, recorded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    span.session_id,
                    span.task_id,
                    span.turn_index,
                    span.kind,
                    span.name,
                    span.detail,
                    span.duration_ms,
                    span.tokens,
                    span.success as i32,
                    span.error,
                    span.recorded_at,
                ],
            );
            if let Err(e) = res {
                tracing::warn!(error = %e, "trace_spans 写入失败");
                let _ = conn.execute("ROLLBACK", []);
                return Err(TianyanError::Custom(format!("trace: 写入失败：{e}")));
            }
        }
        // 清理窗口：保留全局最近 KEEP_RECORDS 条
        if let Err(e) = conn.execute(
            "DELETE FROM trace_spans WHERE id NOT IN (
                SELECT id FROM trace_spans ORDER BY id DESC LIMIT ?1
            )",
            [KEEP_RECORDS],
        ) {
            tracing::warn!(error = %e, "trace_spans 清理失败");
        }
        if let Err(e) = conn.execute("COMMIT", []) {
            tracing::warn!(error = %e, "trace_spans 提交失败");
        }
        self.pending_writes.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// 回放查询（按 session / task 过滤，时间正序）。
    ///
    /// 查询前自动 flush，保证缓冲中的最近记录可见。
    pub async fn query(
        &self,
        session_id: Option<&str>,
        task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TraceSpan>, TianyanError> {
        self.flush().await?;

        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, task_id, turn_index, kind, name, detail,
                        duration_ms, tokens, success, error, recorded_at
                 FROM trace_spans
                 WHERE (?1 IS NULL OR session_id = ?1)
                 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| TianyanError::Custom(format!("trace: 查询失败：{e}")))?;

        let mut rows = stmt
            .query_map(rusqlite::params![session_id, limit as i64], row_to_span)
            .map_err(|e| TianyanError::Custom(format!("trace: 查询失败：{e}")))?;

        let mut spans: Vec<TraceSpan> = Vec::new();
        for row in rows.by_ref() {
            match row {
                Ok(span) => spans.push(span),
                Err(e) => tracing::warn!(error = %e, "trace_spans 行解析失败"),
            }
        }
        // 时间正序（回放顺序）
        spans.reverse();

        // 按 task_id 二次过滤（session + task 组合查询）
        if let Some(tid) = task_id {
            spans.retain(|s| s.task_id.as_deref() == Some(tid));
        }
        Ok(spans)
    }

    /// 待写入缓冲条数（观测用）。
    pub fn pending_count(&self) -> u64 {
        self.pending_writes.load(Ordering::Relaxed)
    }
}

/// SQLite 行 → TraceSpan。
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
        success: row.get::<_, i32>(9)? != 0,
        error: row.get(10)?,
        recorded_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collector() -> Arc<TraceCollector> {
        let db = SqliteDb::open_in_memory().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            db.init_all_schemas().await.unwrap();
        });
        TraceCollector::new(db).unwrap()
    }

    #[test]
    fn test_record_and_flush_roundtrip() {
        let c = collector();
        c.set_current_turn("s1", 0);
        c.record_turn("s1", "gpt-test", 120, 1500, true);
        c.record_tool("s1", "read_file", r#"{"path":"a.rs"}"#, 10, true, None);
        c.record_tool(
            "s1",
            "write_file",
            r#"{"path":"b.rs"}"#,
            5,
            false,
            Some("权限拒绝".to_string()),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let spans = rt.block_on(async { c.query(Some("s1"), None, 100).await.unwrap() });
        assert_eq!(spans.len(), 3, "turn + 2 tools 应全部落库");
        assert_eq!(spans[0].kind, "turn");
        assert_eq!(spans[0].turn_index, Some(0));
        assert_eq!(spans[0].tokens, 1500);
        assert_eq!(spans[1].name, "read_file");
        assert_eq!(spans[2].name, "write_file");
        assert!(!spans[2].success);
        assert_eq!(spans[2].error.as_deref(), Some("权限拒绝"));
    }

    #[test]
    fn test_record_task_span() {
        let c = collector();
        c.record_task(
            "s1",
            "bt_abc",
            "整理日志",
            "已归档 12 个文件",
            3000,
            true,
            None,
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let spans = rt.block_on(async { c.query(None, Some("bt_abc"), 10).await.unwrap() });
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, "task");
        assert_eq!(spans[0].task_id.as_deref(), Some("bt_abc"));
        assert_eq!(spans[0].detail, "已归档 12 个文件");
    }

    #[test]
    fn test_tool_spans_attach_to_current_turn() {
        let c = collector();
        c.set_current_turn("s1", 2);
        c.record_tool("s1", "glob", "{}", 1, true, None);
        // 另一会话并发：独立轮次键
        c.set_current_turn("s2", 0);
        c.record_tool("s2", "lsp", "{}", 1, true, None);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let spans = rt.block_on(async { c.query(Some("s1"), None, 10).await.unwrap() });
        assert_eq!(spans[0].turn_index, Some(2), "工具 span 归属当前轮");
    }

    #[test]
    fn test_query_flushes_pending() {
        let c = collector();
        c.record_tool("s1", "glob", "{}", 1, true, None);
        assert_eq!(c.pending_count(), 1);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let spans = rt.block_on(async { c.query(Some("s1"), None, 10).await.unwrap() });
        assert_eq!(spans.len(), 1);
        assert_eq!(c.pending_count(), 0, "查询前应自动落库");
    }

    #[test]
    fn test_truncate_unicode_boundary() {
        let long = "汉".repeat(200);
        let t = crate::common::truncate::truncate_utf8_boundary(&long, 100);
        assert!(
            t.len() <= 100 + 3,
            "截断 + 省略号（3 字节）应在上限内: {}",
            t.len()
        );
        assert!(t.ends_with('…'));
        assert_eq!(
            (t.len() - 3) % 3,
            0,
            "省略号前不应有半个 UTF-8 字符: {}",
            t.len()
        );
    }
}
