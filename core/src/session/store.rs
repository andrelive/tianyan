//! 会话权威存储（ADR-018：迁出 VFS，SQLite 表为唯一真相）。
//!
//! JSONL 不再保存；消息存 `session_messages`（完整 StructuredMessage 在
//! `content_parts` 列），会话级状态存 `session_meta`（替代原 JSONL 首行
//! SessionHeader）。seq 在单事务内原子分配（`MAX(seq)+1`），唯一索引
//! `(session_id, seq)` 防并发撞号；写入失败显式上抛（不做 best-effort 吞错）。
//!
//! 回忆检索（FTS5 search / window / messages_since）读同一张表，
//! 见 `crate::session::search::SessionRecall`。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::common::types::{Part, StructuredMessage};
use crate::session::SessionHeader;
use crate::vfs::backend::sqlite_db::SqliteDb;

/// 文本列截断上限（FTS 索引体积控制；与回忆模块一致）。
const MAX_TEXT_CHARS: usize = 4000;
/// 工具文本截断上限（回忆/窗口展示保留要点）。
const MAX_TOOL_TEXT_CHARS: usize = 500;

/// 会话元数据（轻量列出，不加载消息）。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// 会话 ID。
    pub session_id: String,
    /// 会话级头部（title/ended_at/injectable 快照）。
    pub header: SessionHeader,
    /// 会话创建时间（epoch 毫秒；缺省 0 = 未知，由调用方兜底）。
    pub created_at: i64,
    /// 消息数（走 (session_id, seq) 索引计数，O(1)）。
    pub message_count: usize,
}

/// 会话权威存储服务。
#[derive(Clone)]
pub struct SessionStore {
    db: SqliteDb,
}

impl SessionStore {
    /// 创建会话存储（共享 SqliteDb 连接）。
    ///
    /// # Errors
    /// * 当前不失败；签名保持与全库统一错误类型。
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
    }

    /// 创建会话元数据行（header 含 created_at/title/ended_at）。
    ///
    /// # Errors
    /// * 会话已存在（session_id 冲突）或 SQLite 写入失败时返回 TianyanError。
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
}

/// 从消息提取 (text, tool_text)：text 仅 user/assistant 文本；工具调用/结果
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

/// SQLite 错误包装（带 session 前缀）。
fn sqlite_error(context: &str, e: rusqlite::Error) -> TianyanError {
    TianyanError::Custom(format!("session: session_store: {context}：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime};

    fn msg(id: &str, role: MessageRole, text: &str) -> StructuredMessage {
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            });
        }
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts,
            tokens: DetailedTokenUsage {
                total: 10,
                ..Default::default()
            },
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: 1_700_000_000_000 + id.len() as i64,
                completed: 0,
            },
            session_id: "s1".to_string(),
            finish: None,
            compression_marker: false,
        }
    }

    async fn make_store() -> Arc<SessionStore> {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        SessionStore::new(db).unwrap()
    }

    #[tokio::test]
    async fn test_append_load_roundtrip() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "你好"))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::Assistant, "世界"))
            .await
            .unwrap();

        let (loaded_header, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(loaded_header.session_header, Some(SessionHeader::MARKER));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[1].id, "m2");
        assert!(store.load("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_seq_monotonic_per_session() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        for i in 0..5 {
            store
                .append_message("s1", &msg(&format!("m{i}"), MessageRole::User, "x"))
                .await
                .unwrap();
        }
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 5);
        // 消息行独立于 meta：追加到不存在的会话仍成功（meta 由 create 负责）
        store
            .append_message("ghost", &msg("g1", MessageRole::User, "y"))
            .await
            .unwrap();
        let (_, messages) = store.load("ghost").await.unwrap().unwrap();
        assert_eq!(messages.len(), 1);
        // 不同会话序号独立（ghost 有 1 条，s1 有 5 条）
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 5);
    }

    #[tokio::test]
    async fn test_empty_message_occupies_seq_but_skips_fts() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m-empty", MessageRole::User, ""))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::User, "正文内容"))
            .await
            .unwrap();
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].id, "m2");
        let recall = crate::session::search::SessionRecall::new(store.db.clone()).unwrap();
        let hits = recall.search("正文内容", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        let hits = recall.search("不存在的词", 10).await.unwrap();
        assert_eq!(hits.len(), 0);
    }

    #[tokio::test]
    async fn test_rewrite_preserves_order_and_header() {
        let store = make_store().await;
        let header = SessionHeader {
            title: Some("标题".to_string()),
            ..SessionHeader::default()
        };
        store.create("s1", &header).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "旧"))
            .await
            .unwrap();
        let mut new_header = header.clone();
        new_header.title = Some("新标题".to_string());
        store
            .rewrite("s1", &new_header, &[msg("m2", MessageRole::User, "新")])
            .await
            .unwrap();
        let (loaded_header, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(loaded_header.title.as_deref(), Some("新标题"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "m2");
    }

    #[tokio::test]
    async fn test_meta_delete_and_recall() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "x"))
            .await
            .unwrap();
        store.create("s2", &SessionHeader::default()).await.unwrap();
        let metas = store.list_meta().await.unwrap();
        assert_eq!(metas.len(), 2);
        assert!(store.exists("s1").await.unwrap());
        assert!(!store.exists("s3").await.unwrap());
        store.delete("s1").await.unwrap();
        assert!(!store.exists("s1").await.unwrap());
        assert_eq!(store.list_meta().await.unwrap().len(), 1);
        // 删除后回忆也查不到
        let recall = crate::session::search::SessionRecall::new(store.db.clone()).unwrap();
        assert!(recall.search("x", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_export_jsonl_roundtrip() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "你好"))
            .await
            .unwrap();
        let jsonl = store.export_jsonl("s1").await.unwrap().unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(SessionHeader::parse_line(lines[0]).is_some());
        let parsed = crate::session::parse_message_lines(&jsonl);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "m1");
        assert_eq!(store.export_jsonl("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_create_duplicate_is_conflict() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        let err = store
            .create("s1", &SessionHeader::default())
            .await
            .unwrap_err();
        assert!(err.is_conflict());
    }
}
