//! 会话持久化仓储（Repository 层）。
//!
//! **SQL 收敛**：会话消息/元数据/FTS 与全文回忆的数据访问统一收口于此，
//! 业务组件（SessionStore/SessionRecall）不再直接写 SQL。
//!
//! 依赖方向：`db → session::types`（纯类型）+ `common`——不依赖 session 域实现。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::common::types::{Part, StructuredMessage};
use crate::db::Database;
use crate::session::types::{RecallHit, RecallMessage, SessionHeader, SessionMeta};

/// 会话数据访问仓储（单连接经 Database 统一访问）。
#[derive(Clone)]
pub struct SessionRepo {
    db: Arc<Database>,
}

/// 文本列截断上限（FTS 索引体积控制）。
const MAX_TEXT_CHARS: usize = 4000;
/// 工具文本截断上限（回忆/窗口展示保留要点）。
const MAX_TOOL_TEXT_CHARS: usize = 500;

/// 从消息 parts 提取纯文本与工具文本：文本进正文与 FTS；工具调用/结果
/// 截断进 tool_text（回忆窗口/记忆提取保留要点）。空消息（纯图片等）双空。
fn extract_parts(msg: &StructuredMessage) -> (String, String) {
    let mut text = String::new();
    let mut tool_text = String::new();
    for part in &msg.parts {
        match part {
            Part::Text { text: t, .. } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            Part::ToolCall {
                name, arguments, ..
            } => {
                if !tool_text.is_empty() {
                    tool_text.push('\n');
                }
                tool_text.push_str(&format!("[调用 {name}] {arguments}"));
            }
            Part::ToolResult { content, .. } => {
                if !tool_text.is_empty() {
                    tool_text.push('\n');
                }
                tool_text.push_str(&format!("[结果] {content}"));
            }
            Part::Reasoning { .. } | Part::Image { .. } => {}
        }
    }
    (
        crate::common::truncate::truncate_utf8_boundary(&text, MAX_TEXT_CHARS),
        crate::common::truncate::truncate_utf8_boundary(&tool_text, MAX_TOOL_TEXT_CHARS),
    )
}
impl SessionRepo {
    /// 创建仓储（共享 Database 单连接）。
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// 共享 Database（仓储用；测试/组合需要时访问）。
    pub fn database(&self) -> &Arc<Database> {
        &self.db
    }

    pub async fn create(
        &self,
        session_id: &str,
        header: &SessionHeader,
    ) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        let header_json = serde_json::to_string(header).map_err(|e| {
            TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
        })?;
        let created_at = header.created_at.map(|t| t.timestamp_millis()).unwrap_or(0);
        conn.execute(
            "INSERT INTO session_meta (session_id, header_json, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, header_json, created_at],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                TianyanError::conflict(format!("session: session_store: 会话已存在：{session_id}"))
            }
            e => sqlite_error("会话创建失败", e),
        })?;
        Ok(())
    }

    /// 追加一条消息：单事务内原子取号（`MAX(seq)+1`）并写入完整消息，
    /// text 非空时同步写 FTS（空正文/纯图片消息占 seq 但不进 FTS）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError（显式上抛，不吞错）。
    pub async fn append_message(
        &self,
        session_id: &str,
        msg: &StructuredMessage,
    ) -> Result<(), TianyanError> {
        let (text, tool_text) = extract_parts(msg);
        let content_parts = serde_json::to_string(msg).map_err(|e| {
            TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
        })?;
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| sqlite_error("追加事务开启失败", e))?;
        // 确保会话元数据存在（append 即隐式创建，与旧 JSONL append 语义一致；
        // created_at 缺省 0，由后续 update_header/create 固化）
        tx.execute(
            "INSERT OR IGNORE INTO session_meta (session_id, header_json) VALUES (?1, '{}')",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("会话元数据初始化失败", e))?;
        // 原子取号：同一事务/同一连接内串行，并发追加不会撞号
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM session_messages WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .map_err(|e| sqlite_error("消息序号分配失败", e))?;
        tx.execute(
            "INSERT INTO session_messages (session_id, seq, message_id, role, text, tool_text, tokens, ts, content_parts) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                session_id,
                seq,
                msg.id,
                msg.role.to_string(),
                text,
                tool_text,
                msg.tokens.total as i64,
                msg.time.created,
                content_parts,
            ],
        )
        .map_err(|e| sqlite_error("消息写入失败", e))?;
        if !text.is_empty() {
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO session_messages_fts (rowid, text) VALUES (?1, ?2)",
                rusqlite::params![rowid, text],
            )
            .map_err(|e| sqlite_error("FTS 索引写入失败", e))?;
        }
        tx.commit()
            .map_err(|e| sqlite_error("追加事务提交失败", e))?;
        Ok(())
    }

    /// 加载会话：元数据 + 全量消息（按 seq 升序）。
    ///
    /// # Errors
    /// * SQLite 查询失败或 content_parts 反序列化失败时返回 TianyanError。
    pub async fn load(
        &self,
        session_id: &str,
    ) -> Result<Option<(SessionHeader, Vec<StructuredMessage>)>, TianyanError> {
        let conn = self.db.lock().await;
        let header_json: Option<String> = match conn.query_row(
            "SELECT header_json FROM session_meta WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(sqlite_error("会话元数据查询失败", e)),
        };
        let Some(header_json) = header_json else {
            return Ok(None);
        };
        let header: SessionHeader = serde_json::from_str(&header_json).unwrap_or_default();
        let mut stmt = conn
            .prepare(
                "SELECT content_parts FROM session_messages WHERE session_id = ?1 ORDER BY seq",
            )
            .map_err(|e| sqlite_error("消息查询准备失败", e))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id], |r| r.get::<_, String>(0))
            .map_err(|e| sqlite_error("消息查询执行失败", e))?;
        let mut messages = Vec::new();
        for row in rows {
            let json = row.map_err(|e| sqlite_error("消息行读取失败", e))?;
            let msg: StructuredMessage = serde_json::from_str(&json).map_err(|e| {
                TianyanError::Custom(format!("session: session_store: 消息反序列化失败：{e}"))
            })?;
            messages.push(msg);
        }
        Ok(Some((header, messages)))
    }

    /// 全量重写会话（压缩/截断/编辑后调用）：单事务内清 FTS + 清消息 +
    /// 重写 meta + 逐条写入（seq = 行号）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError。
    pub async fn rewrite(
        &self,
        session_id: &str,
        header: &SessionHeader,
        messages: &[StructuredMessage],
    ) -> Result<(), TianyanError> {
        let header_json = serde_json::to_string(header).map_err(|e| {
            TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
        })?;
        let created_at = header.created_at.map(|t| t.timestamp_millis()).unwrap_or(0);
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| sqlite_error("重写事务开启失败", e))?;
        tx.execute(
            "DELETE FROM session_messages_fts WHERE rowid IN              (SELECT id FROM session_messages WHERE session_id = ?1)",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("重写 FTS 清理失败", e))?;
        tx.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("重写消息清理失败", e))?;
        tx.execute(
            "INSERT INTO session_meta (session_id, header_json, created_at, updated_at) VALUES (?1, ?2, ?3, datetime('now')) ON CONFLICT(session_id) DO UPDATE SET header_json = excluded.header_json, created_at = excluded.created_at, updated_at = excluded.updated_at",
            rusqlite::params![session_id, header_json, created_at],
        )
        .map_err(|e| sqlite_error("重写元数据失败", e))?;
        for (i, msg) in messages.iter().enumerate() {
            let (text, tool_text) = extract_parts(msg);
            let content_parts = serde_json::to_string(msg).map_err(|e| {
                TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
            })?;
            tx.execute(
                "INSERT INTO session_messages (session_id, seq, message_id, role, text, tool_text, tokens, ts, content_parts) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                rusqlite::params![
                    session_id,
                    i as i64,
                    msg.id,
                    msg.role.to_string(),
                    text,
                    tool_text,
                    msg.tokens.total as i64,
                    msg.time.created,
                    content_parts,
                ],
            )
            .map_err(|e| sqlite_error("重写消息写入失败", e))?;
            if !text.is_empty() {
                let rowid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO session_messages_fts (rowid, text) VALUES (?1, ?2)",
                    rusqlite::params![rowid, text],
                )
                .map_err(|e| sqlite_error("重写 FTS 写入失败", e))?;
            }
        }
        tx.commit()
            .map_err(|e| sqlite_error("重写事务提交失败", e))?;
        Ok(())
    }

    /// 读取会话头部（injectable 快照等会话级状态）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn header(&self, session_id: &str) -> Result<Option<SessionHeader>, TianyanError> {
        let conn = self.db.lock().await;
        let header_json: Option<String> = match conn.query_row(
            "SELECT header_json FROM session_meta WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(sqlite_error("会话头部查询失败", e)),
        };
        Ok(header_json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// 更新会话头部（title/ended_at/created_at 同步；injectable 快照固化）。
    ///
    /// # Errors
    /// * 会话不存在或 SQLite 写入失败时返回 TianyanError。
    pub async fn update_header(
        &self,
        session_id: &str,
        header: &SessionHeader,
    ) -> Result<(), TianyanError> {
        let header_json = serde_json::to_string(header).map_err(|e| {
            TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
        })?;
        let created_at = header.created_at.map(|t| t.timestamp_millis()).unwrap_or(0);
        let conn = self.db.lock().await;
        let changed = conn
            .execute(
                "UPDATE session_meta SET header_json = ?2, created_at = ?3, updated_at = datetime('now') WHERE session_id = ?1",
                rusqlite::params![session_id, header_json, created_at],
            )
            .map_err(|e| sqlite_error("会话头部更新失败", e))?;
        if changed == 0 {
            return Err(TianyanError::not_found(format!(
                "session: session_store: 会话未找到：{session_id}"
            )));
        }
        Ok(())
    }

    /// 轻量列出所有会话（仅元数据，不加载消息；按创建时间倒序）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn list_meta(&self) -> Result<Vec<SessionMeta>, TianyanError> {
        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT sm.session_id, sm.header_json, sm.created_at,                    (SELECT COUNT(*) FROM session_messages m WHERE m.session_id = sm.session_id)                    FROM session_meta sm ORDER BY sm.created_at DESC",
            )
            .map_err(|e| sqlite_error("会话列表查询准备失败", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| sqlite_error("会话列表查询执行失败", e))?;
        let mut out = Vec::new();
        for row in rows {
            let (session_id, header_json, created_at, message_count) =
                row.map_err(|e| sqlite_error("会话列表行读取失败", e))?;
            out.push(SessionMeta {
                session_id,
                header: serde_json::from_str(&header_json).unwrap_or_default(),
                created_at,
                message_count: message_count as usize,
            });
        }
        Ok(out)
    }

    /// 会话是否存在。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn exists(&self, session_id: &str) -> Result<bool, TianyanError> {
        let conn = self.db.lock().await;
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_meta WHERE session_id = ?1)",
            rusqlite::params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| sqlite_error("会话存在性查询失败", e))
    }

    /// 删除会话（单事务：清 FTS + 消息 + 元数据）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError。
    pub async fn delete(&self, session_id: &str) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| sqlite_error("删除事务开启失败", e))?;
        tx.execute(
            "DELETE FROM session_messages_fts WHERE rowid IN              (SELECT id FROM session_messages WHERE session_id = ?1)",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("删除 FTS 清理失败", e))?;
        tx.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("删除消息清理失败", e))?;
        tx.execute(
            "DELETE FROM session_meta WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("删除元数据失败", e))?;
        tx.commit()
            .map_err(|e| sqlite_error("删除事务提交失败", e))?;
        Ok(())
    }

    /// 导出会话为 JSONL 文本（首行 SessionHeader + 逐行消息；vfs_read 兼容层用）。
    ///
    /// # Errors
    /// * SQLite 查询失败或序列化失败时返回 TianyanError。
    pub async fn export_jsonl(&self, session_id: &str) -> Result<Option<String>, TianyanError> {
        let Some((header, messages)) = self.load(session_id).await? else {
            return Ok(None);
        };
        let mut content = serde_json::to_string(&header).map_err(|e| {
            TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
        })?;
        content.push('\n');
        for msg in &messages {
            let line = serde_json::to_string(msg).map_err(|e| {
                TianyanError::Custom(format!("session: session_store: 序列化错误：{e}"))
            })?;
            content.push_str(&line);
            content.push('\n');
        }
        Ok(Some(content))
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<RecallHit>, TianyanError> {
        let q = sanitize_fts_query(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.db.lock().await;
        let sql = "SELECT m.session_id, m.seq, m.message_id, m.role, m.text,                    bm25(session_messages_fts) AS score                    FROM session_messages_fts                    JOIN session_messages m ON m.id = session_messages_fts.rowid                    WHERE session_messages_fts MATCH ?1                    ORDER BY score LIMIT ?2";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| sqlite_error("回忆查询准备失败", e))?;
        let rows = stmt
            .query_map(rusqlite::params![q, limit as i64], |row| {
                Ok(RecallHit {
                    session_id: row.get(0)?,
                    seq: row.get(1)?,
                    message_id: row.get(2)?,
                    role: row.get(3)?,
                    text: row.get(4)?,
                    score: -row.get::<_, f64>(5)?,
                })
            })
            .map_err(|e| sqlite_error("回忆查询执行失败", e))?;
        let mut hits = Vec::new();
        for row in rows {
            hits.push(row.map_err(|e| sqlite_error("回忆查询行读取失败", e))?);
        }
        Ok(hits)
    }

    /// 命中附近窗口（user/assistant 文本 + tool_text 截断；忽略工具过滤由
    /// text 列天然保证——工具调用/结果只进 tool_text；空正文消息不参与）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 session 前缀）。
    pub async fn window(
        &self,
        session_id: &str,
        center_seq: i64,
        radius: i64,
    ) -> Result<Vec<RecallMessage>, TianyanError> {
        let conn = self.db.lock().await;
        let sql = "SELECT session_id, seq, message_id, role, text, tool_text, ts                    FROM session_messages                    WHERE session_id = ?1 AND seq BETWEEN ?2 AND ?3                      AND (text != '' OR tool_text != '')                    ORDER BY seq";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| sqlite_error("窗口查询准备失败", e))?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, center_seq - radius, center_seq + radius],
                map_recall_message,
            )
            .map_err(|e| sqlite_error("窗口查询执行失败", e))?;
        collect(rows, "窗口查询行读取失败")
    }

    /// 会话内自 since_seq 之后的消息（水位线增量；供记忆提取/演化任务）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 session 前缀）。
    pub async fn messages_since(
        &self,
        session_id: &str,
        since_seq: i64,
        limit: usize,
    ) -> Result<Vec<RecallMessage>, TianyanError> {
        let conn = self.db.lock().await;
        let sql = "SELECT session_id, seq, message_id, role, text, tool_text, ts                    FROM session_messages                    WHERE session_id = ?1 AND seq > ?2                      AND (text != '' OR tool_text != '')                    ORDER BY seq LIMIT ?3";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| sqlite_error("增量查询准备失败", e))?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, since_seq, limit as i64],
                map_recall_message,
            )
            .map_err(|e| sqlite_error("增量查询执行失败", e))?;
        collect(rows, "增量查询行读取失败")
    }
}

/// 行映射：RecallMessage。
fn map_recall_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecallMessage> {
    Ok(RecallMessage {
        session_id: row.get(0)?,
        seq: row.get(1)?,
        message_id: row.get(2)?,
        role: row.get(3)?,
        text: row.get(4)?,
        tool_text: row.get(5)?,
        ts: row.get(6)?,
    })
}

/// 收集查询行（统一错误包装）。
fn collect(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<RecallMessage>,
    >,
    err_msg: &str,
) -> Result<Vec<RecallMessage>, TianyanError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sqlite_error(err_msg, e))?);
    }
    Ok(out)
}

/// FTS5 查询净化：trigram 短语匹配（去引号防注入；过短查询返回空）。
pub(crate) fn sanitize_fts_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .filter(|c| !matches!(c, '"' | '*' | '^' | ':' | '(' | ')' | '-' | ' '))
        .collect();
    if cleaned.chars().count() < 3 {
        return String::new();
    }
    format!("\"{cleaned}\"")
}

/// SQLite 错误包装（带 session 前缀）。
fn sqlite_error(context: &str, e: rusqlite::Error) -> TianyanError {
    TianyanError::Custom(format!("session: session_recall: {context}：{e}"))
}

