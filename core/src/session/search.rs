//! 会话回忆服务（ADR-017 决策 6：FTS5 消息索引）。
//!
//! **SQL 收敛于本模块**（会话检索专属，读 session_messages/session_messages_fts）：
//! 经共享的 Database 单连接直接实现全文检索/窗口/增量查询；不依赖 db 层通用仓储。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::db::Database;
use crate::session::types::{RecallHit, RecallMessage};

/// 会话回忆服务（经 Database 单连接直接实现 FTS 查询）。
pub struct SessionRecall {
    db: Arc<Database>,
}

impl SessionRecall {
    /// 创建会话回忆服务（共享 Database 单连接）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
    }

    /// FTS 回忆检索（关键词匹配历史消息；返回命中及其附近窗口）。
    ///
    /// `since_days`：只看最近 N 天（None/0 = 全历史）。
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        since_days: Option<u32>,
    ) -> Result<Vec<RecallHit>, TianyanError> {
        let q = sanitize_fts_query(query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        // 时间过滤（可选）：只看最近 N 天（epoch 毫秒水位）
        let since_ms: Option<i64> = since_days.filter(|d| *d > 0).map(|d| {
            (chrono::Utc::now() - chrono::Duration::days(i64::from(d))).timestamp_millis()
        });
        let conn = self.db.lock().await;
        let sql = "SELECT m.session_id, m.seq, m.message_id, m.role, m.text,                    bm25(session_messages_fts) AS score                    FROM session_messages_fts                    JOIN session_messages m ON m.id = session_messages_fts.rowid                    WHERE session_messages_fts MATCH ?1                    AND (?3 IS NULL OR m.ts > ?3)                    ORDER BY score LIMIT ?2";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| sqlite_error("回忆查询准备失败", e))?;
        let rows = stmt
            .query_map(rusqlite::params![q, limit as i64, since_ms], |row| {
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

    /// 跨会话增量：`since_ms`（epoch 毫秒）之后有正文的主会话消息（最近优先）。
    ///
    /// - 只含 user/assistant 文本（纯工具/空正文消息不进材料）；
    /// - 排除子智能体会话（`session_meta.parent_session_id` 非空，ADR-026）；
    /// - 供演化综述"近期会话增量"采集（ADR-017：防止跨运行重复通读旧会话）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError::Custom（带 session 前缀）。
    pub async fn recent_since(
        &self,
        since_ms: i64,
        limit: usize,
    ) -> Result<Vec<RecallMessage>, TianyanError> {
        let conn = self.db.lock().await;
        let sql = "SELECT m.session_id, m.seq, m.message_id, m.role, m.text, m.tool_text, m.ts                    FROM session_messages m                    JOIN session_meta meta ON meta.session_id = m.session_id                    WHERE m.ts > ?1 AND m.text != '' AND m.role IN ('user','assistant')                      AND meta.parent_session_id IS NULL                    ORDER BY m.ts DESC LIMIT ?2";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| sqlite_error("增量查询准备失败", e))?;
        let rows = stmt
            .query_map(
                rusqlite::params![since_ms, limit as i64],
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
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let store = SessionStore::new(db.clone()).unwrap();
        // 会话权威存储要求显式 create（append 不再隐式重建幽灵会话）
        store
            .create("s1", &crate::session::SessionHeader::default())
            .await
            .unwrap();
        (store, SessionRecall::new(db).unwrap())
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
        let hits = recall.search("部署方案", 5, None).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m1");
        assert!(hits[0].score > 0.0);

        // 英文子串命中
        let hits = recall.search("docker", 5, None).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m2");

        // 无命中
        let hits = recall.search("不存在的词", 5, None).await.unwrap();
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
        let hits = recall.search("read_file", 5, None).await.unwrap();
        assert!(hits.is_empty());
        let hits = recall.search("我来查一下", 5, None).await.unwrap();
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

    /// 指定时间戳的消息（增量/水位线测试用）。
    fn msg_at(id: &str, role: MessageRole, text: &str, ts: i64) -> StructuredMessage {
        let mut m = msg(id, role, text, None);
        m.time.created = ts;
        m
    }

    #[tokio::test]
    async fn test_recent_since_watermark_no_repeat_across_days() {
        // "跨 3 天"会话：Day1/Day2/Day3 各一条。水位线推进 → 每次只取增量，
        // 旧片段不复现（防止综述连续多天重复通读同一批旧消息）。
        let (store, recall) = make_pair().await;
        let day1 = 1_800_000_000_000i64;
        let day = 86_400_000i64;
        store
            .append_message("s1", &msg_at("d1", MessageRole::User, "Day1 内容", day1))
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg_at("d2", MessageRole::Assistant, "Day2 内容", day1 + day),
            )
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg_at("d3", MessageRole::User, "Day3 内容", day1 + 2 * day),
            )
            .await
            .unwrap();

        // 下一次运行：水位线 = Day1 消息之后 → 只看到 Day2+Day3（Day1 不重复）
        let got = recall.recent_since(day1, 100).await.unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|m| m.ts > day1));
        assert!(!got.iter().any(|m| m.text.contains("Day1")));
        assert!(got[0].ts >= got[1].ts, "最近优先（ts DESC）");
        // 再下一次：水位线 = Day2 消息之后 → 只剩 Day3（Day1/Day2 均不复现）
        let got2 = recall.recent_since(day1 + day, 100).await.unwrap();
        assert_eq!(got2.len(), 1);
        assert!(got2[0].text.contains("Day3"));
    }

    #[tokio::test]
    async fn test_recent_since_excludes_child_empty_and_tool_only() {
        let (store, recall) = make_pair().await;
        let base = 1_800_000_000_000i64;
        // 主会话：空正文 / 纯工具 / 正常正文各一条
        store
            .append_message("s1", &msg_at("empty", MessageRole::User, "", base + 1))
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg(
                    "tool",
                    MessageRole::Assistant,
                    "",
                    Some(("read_file", "{}")),
                ),
            )
            .await
            .unwrap();
        store
            .append_message("s1", &msg_at("ok", MessageRole::User, "正常内容", base + 4))
            .await
            .unwrap();
        // 子智能体会话（带 parent）：不进增量材料（ADR-026）
        store
            .create(
                "child",
                &crate::session::SessionHeader {
                    parent_session_id: Some("s1".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        store
            .append_message(
                "child",
                &msg_at("c1", MessageRole::User, "子会话内容", base + 5),
            )
            .await
            .unwrap();

        let got = recall.recent_since(base, 100).await.unwrap();
        assert_eq!(got.len(), 1, "只应包含主会话正常正文消息: {got:?}");
        assert_eq!(got[0].message_id, "ok");
    }

    #[tokio::test]
    async fn test_search_respects_since_days() {
        let (store, recall) = make_pair().await;
        let now = chrono::Utc::now().timestamp_millis();
        let day = 86_400_000i64;
        store
            .append_message(
                "s1",
                &msg_at("old", MessageRole::User, "旧部署方案记录", now - 30 * day),
            )
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg_at("new", MessageRole::User, "新部署方案记录", now - day / 2),
            )
            .await
            .unwrap();
        // 不限时间：两条都命中
        let all = recall.search("部署方案记录", 10, None).await.unwrap();
        assert_eq!(all.len(), 2);
        // 最近 3 天：只剩新记录
        let recent = recall.search("部署方案记录", 10, Some(3)).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].message_id, "new");
        // since_days=0 等同不限
        let zero = recall.search("部署方案记录", 10, Some(0)).await.unwrap();
        assert_eq!(zero.len(), 2);
    }
}
