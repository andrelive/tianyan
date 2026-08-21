//! 会话回忆检索（ADR-017 决策 6：FTS5 会话回忆，方案 A）。
//!
//! 会话权威存储已迁入 SQLite（ADR-018，`crate::session::store::SessionStore`）；
//! 本模块是回忆检索服务，读同一张 `session_messages` 表：
//! - FTS5 倒排索引（trigram tokenizer：中文子串匹配，BM25 排序；rowid 对应消息行）；
//! - 回忆流程：关键词 → BM25 命中 → 附近窗口（user/assistant 文本，忽略工具）；
//! - 消息写入（含 FTS 维护）由 SessionStore 负责，本模块只读。
//!
//! 架构落位：session 模块能力（共享 SqliteDb，权威数据），不新增平行检索抽象。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::vfs::backend::sqlite_db::SqliteDb;

/// 回忆窗口默认半径（命中前后各 N 条）。
pub const DEFAULT_WINDOW_RADIUS: i64 = 5;

/// 回忆命中。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallHit {
    /// 来源会话。
    pub session_id: String,
    /// 消息序号（会话内 0 起始）。
    pub seq: i64,
    /// 消息 ID。
    pub message_id: String,
    /// 消息角色。
    pub role: String,
    /// 文本内容（user/assistant）。
    pub text: String,
    /// 相关度（BM25 分数取负，越大越相关）。
    pub score: f64,
}

/// 回忆窗口消息。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallMessage {
    /// 来源会话。
    pub session_id: String,
    /// 消息序号。
    pub seq: i64,
    /// 消息 ID。
    pub message_id: String,
    /// 消息角色。
    pub role: String,
    /// 文本内容（user/assistant）。
    pub text: String,
    /// 工具调用/结果（截断；回忆窗口展示用）。
    pub tool_text: String,
    /// 消息创建时间（epoch 毫秒）。
    pub ts: i64,
}

/// 会话回忆检索服务。
#[derive(Clone)]
pub struct SessionRecall {
    db: SqliteDb,
}

impl SessionRecall {
    /// 创建会话回忆服务（共享 SqliteDb 连接）。
    ///
    /// # Errors
    /// * 返回 TianyanError（本方法当前不失败，签名保持与全库统一错误类型）。
    pub fn new(db: SqliteDb) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
    }

    /// 关键词回忆：FTS5 BM25 命中（trigram 子串匹配，中文无需分词）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 session 前缀）。
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
fn sanitize_fts_query(query: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{
        DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
    };
    use crate::session::store::SessionStore;

    fn msg(
        id: &str,
        role: MessageRole,
        text: &str,
        tool: Option<(&str, &str)>,
    ) -> StructuredMessage {
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            });
        }
        if let Some((name, args)) = tool {
            parts.push(Part::ToolCall {
                id: format!("call-{id}"),
                name: name.to_string(),
                arguments: args.to_string(),
                time: PartTime::default(),
            });
            parts.push(Part::ToolResult {
                tool_call_id: format!("call-{id}"),
                content: format!("结果-{id}"),
                error: None,
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

    /// 建内存库：store（写数据）+ recall（查数据）共享同一 SqliteDb。
    async fn make_pair() -> (Arc<SessionStore>, Arc<SessionRecall>) {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        (
            SessionStore::new(db.clone()).unwrap(),
            SessionRecall::new(db).unwrap(),
        )
    }

    #[tokio::test]
    async fn test_index_and_search_chinese_substring() {
        let (store, recall) = make_pair().await;
        store
            .append_message(
                "s1",
                &msg("m1", MessageRole::User, "我们讨论部署方案", None),
            )
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg("m2", MessageRole::Assistant, "建议用 docker 部署", None),
            )
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m3", MessageRole::User, "今天天气不错", None))
            .await
            .unwrap();

        // 中文子串命中（trigram）
        let hits = recall.search("部署方案", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m1");
        assert!(hits[0].score > 0.0);

        // 英文子串命中
        let hits = recall.search("docker", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m2");

        // 无命中
        let hits = recall.search("不存在的词", 5).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_tool_parts_not_in_fts() {
        let (store, recall) = make_pair().await;
        store
            .append_message(
                "s1",
                &msg(
                    "m1",
                    MessageRole::Assistant,
                    "我来查一下",
                    Some(("read_file", "{\"path\": \"a.rs\"}")),
                ),
            )
            .await
            .unwrap();

        // 工具名/参数不进 FTS（text 列只有 user/assistant 文本）
        let hits = recall.search("read_file", 5).await.unwrap();
        assert!(hits.is_empty());
        let hits = recall.search("我来查一下", 5).await.unwrap();
        assert_eq!(hits.len(), 1);

        // tool_text 保留（回忆窗口展示）
        let since = recall.messages_since("s1", -1, 10).await.unwrap();
        assert_eq!(since.len(), 1);
        assert!(since[0].tool_text.contains("read_file"));
        assert!(since[0].tool_text.contains("结果-m1"));
    }

    #[tokio::test]
    async fn test_window_around_hit() {
        let (store, recall) = make_pair().await;
        for i in 0..10 {
            store
                .append_message(
                    "s1",
                    &msg(
                        &format!("m{i}"),
                        MessageRole::User,
                        &format!("消息 {i}"),
                        None,
                    ),
                )
                .await
                .unwrap();
        }
        let win = recall.window("s1", 5, 2).await.unwrap();
        assert_eq!(win.len(), 5);
        assert_eq!(win[0].seq, 3);
        assert_eq!(win[4].seq, 7);
    }

    #[tokio::test]
    async fn test_messages_since_watermark() {
        let (store, recall) = make_pair().await;
        for i in 0..5 {
            store
                .append_message(
                    "s1",
                    &msg(
                        &format!("m{i}"),
                        MessageRole::User,
                        &format!("消息 {i}"),
                        None,
                    ),
                )
                .await
                .unwrap();
        }
        let since = recall.messages_since("s1", 2, 10).await.unwrap();
        assert_eq!(since.len(), 2);
        assert_eq!(since[0].seq, 3);
        assert_eq!(since[1].seq, 4);
    }

    #[tokio::test]
    async fn test_empty_message_excluded_from_recall_queries() {
        let (store, recall) = make_pair().await;
        // 空正文消息占 seq 但不参与窗口/增量查询
        store
            .append_message("s1", &msg("m-empty", MessageRole::User, "", None))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::User, "有正文", None))
            .await
            .unwrap();
        let since = recall.messages_since("s1", -1, 10).await.unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].message_id, "m2");
        let win = recall.window("s1", 1, 1).await.unwrap();
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].message_id, "m2");
    }

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(sanitize_fts_query("部署方案"), "\"部署方案\"");
        assert_eq!(sanitize_fts_query("a b c"), "\"abc\"");
        assert_eq!(sanitize_fts_query("ab"), "");
        assert_eq!(sanitize_fts_query("带\"引号\"的词"), "\"带引号的词\"");
    }
}
