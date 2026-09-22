//! 会话权威存储（ADR-018：SQLite 表为唯一真相）。
//!
//! **SQL 收敛于本模块**（会话专属存储，ADR-018 例外）：消息/元数据/FTS 的
//! 数据访问直接实现于此（经共享的 Database 单连接）；通用 SQLite 设施
//! （门面/连接/schema）在 `db` 模块——`session → db` 单向，避免环。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::common::types::{Part, StructuredMessage};
use crate::db::Database;
use crate::session::types::{SessionHeader, SessionMeta};
use rusqlite::OptionalExtension;

/// 会话权威存储（经 Database 单连接直接实现 SQL；ADR-018）。
#[derive(Clone)]
pub struct SessionStore {
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

/// SQLite 错误包装（带 session 前缀）。
/// 递归后代集合 CTE（T0-13）：从 `?1`（被删/被清会话）出发沿
/// `parent_session_id` 链收集**全部**后代（子/孙/…）。`UNION` 去重兼防环。
const DESCENDANT_CTE: &str = "WITH RECURSIVE descendants(session_id) AS (SELECT session_id FROM session_meta WHERE parent_session_id = ?1 UNION SELECT m.session_id FROM session_meta m JOIN descendants d ON m.parent_session_id = d.session_id) ";

fn sqlite_error(context: &str, e: rusqlite::Error) -> TianyanError {
    TianyanError::Custom(format!("session: session_store: {context}：{e}"))
}

/// 解析单行 `content_parts` JSON；损坏时记 warn 并返回 `None`（T1-17：
/// 单行损坏不废整个会话——调用方据返回值决定占位或跳过）。
fn parse_message_row(json: &str, session_id: &str, seq: i64) -> Option<StructuredMessage> {
    match serde_json::from_str::<StructuredMessage>(json) {
        Ok(msg) => Some(msg),
        Err(e) => {
            tracing::warn!(
                session_id,
                seq,
                error = %e,
                "session: session_store: 消息行损坏，已跳过"
            );
            None
        }
    }
}

impl SessionStore {
    /// 创建会话存储（共享 Database 单连接）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self { db }))
    }

    /// 创建会话（写 `session_meta`；重复创建返回 conflict，ADR-014）。
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
            "INSERT INTO session_meta (session_id, header_json, created_at, parent_session_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, header_json, created_at, header.parent_session_id],
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

    /// 追加一条消息：单事务内原子取号（MAX(seq)+1）并写入完整消息，
    /// text 非空时同步写 FTS（空正文/纯图片消息占 seq 但不进 FTS）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError（显式上抛，不吞错）。
    pub async fn append_message(
        &self,
        session_id: &str,
        msg: &StructuredMessage,
    ) -> Result<(), TianyanError> {
        self.append_message_inner(session_id, msg, true).await
    }

    /// 追加消息但不写 FTS（ADR-026：子智能体会话——无"人"提供的信息，
    /// 不进回忆检索；FTS 索引不膨胀，session_recall 天然搜不到）。
    pub async fn append_message_no_fts(
        &self,
        session_id: &str,
        msg: &StructuredMessage,
    ) -> Result<(), TianyanError> {
        self.append_message_inner(session_id, msg, false).await
    }

    /// 追加消息核心实现（index_fts 控制是否写 FTS 索引）。
    async fn append_message_inner(
        &self,
        session_id: &str,
        msg: &StructuredMessage,
        index_fts: bool,
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
        if index_fts && !text.is_empty() {
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO session_messages_fts (rowid, text) VALUES (?1, ?2)",
                rusqlite::params![rowid, text],
            )
            .map_err(|e| sqlite_error("FTS 索引写入失败", e))?;
        }
        // ADR-026：刷新会话活跃时间（清理策略 B 的"最不活跃"判定依据）
        tx.execute(
            "UPDATE session_meta SET updated_at = datetime('now') WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("会话活跃时间刷新失败", e))?;
        tx.commit()
            .map_err(|e| sqlite_error("追加事务提交失败", e))?;
        Ok(())
    }

    /// 加载会话：元数据 + 全量消息（按 seq 升序）。
    ///
    /// **逐行容错（T1-17）**：单行 `content_parts` 损坏不废整个会话——损坏行
    /// 以占位 system 消息顶替（保持「位置即 seq」不变式，ADR-027），并记 warn。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError（行级 JSON 损坏不返回错误）。
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
                "SELECT seq, content_parts FROM session_messages WHERE session_id = ?1 ORDER BY seq",
            )
            .map_err(|e| sqlite_error("消息查询准备失败", e))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| sqlite_error("消息查询执行失败", e))?;
        let mut messages = Vec::new();
        for row in rows {
            let (seq, json) = row.map_err(|e| sqlite_error("消息行读取失败", e))?;
            match parse_message_row(&json, session_id, seq) {
                Some(msg) => messages.push(msg),
                // 占位：保持「位置即 seq」不变式（ADR-027），损坏行不使链错位
                None => messages.push(StructuredMessage::system(
                    session_id,
                    format!("[会话存储：第 {seq} 条消息损坏，已跳过]"),
                )),
            }
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

    /// 轻量列出所有主会话（仅元数据，不加载消息；按最后消息时间倒序）。
    ///
    /// ADR-026：子智能体会话（parent_session_id 非空）不进会话列表。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn list_meta(&self) -> Result<Vec<SessionMeta>, TianyanError> {
        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT sm.session_id, sm.header_json, sm.created_at,                    (SELECT COUNT(*) FROM session_messages m WHERE m.session_id = sm.session_id),                    (SELECT MAX(m.ts) FROM session_messages m WHERE m.session_id = sm.session_id)                    FROM session_meta sm WHERE sm.parent_session_id IS NULL                    AND sm.session_id NOT LIKE 'evolution-%'                    ORDER BY COALESCE((SELECT MAX(m.ts) FROM session_messages m WHERE m.session_id = sm.session_id), sm.created_at) DESC",
            )
            .map_err(|e| sqlite_error("会话列表查询准备失败", e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    // created_at 可空（append 隐式创建的会话缺省 NULL）：
                    // 读取为 Option，NULL 归 0（排序靠后），避免整列表 500
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(|e| sqlite_error("会话列表查询执行失败", e))?;
        let mut out = Vec::new();
        for row in rows {
            let (session_id, header_json, created_at, message_count, last_message_at) =
                row.map_err(|e| sqlite_error("会话列表行读取失败", e))?;
            out.push(SessionMeta {
                session_id,
                header: serde_json::from_str(&header_json).unwrap_or_default(),
                created_at,
                last_message_at,
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

    /// 会话最新消息序号（无消息时返回 -1）。
    ///
    /// ADR-028：事件推送的 seq 锚点（落库后取号广播；断点对齐的游标）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn last_seq(&self, session_id: &str) -> Result<i64, TianyanError> {
        let conn = self.db.lock().await;
        conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) FROM session_messages WHERE session_id = ?1",
            rusqlite::params![session_id],
            |r| r.get(0),
        )
        .map_err(|e| sqlite_error("会话最新序号查询失败", e))
    }

    /// 加载自 since_seq 之后的消息（含 seq，ADR-028 断点对齐增量）。
    ///
    /// **逐行容错（T1-17）**：损坏行跳过并记 warn（带真实 seq，空洞无害）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError（行级 JSON 损坏不返回错误）。
    pub async fn load_after(
        &self,
        session_id: &str,
        since_seq: i64,
    ) -> Result<Vec<(i64, StructuredMessage)>, TianyanError> {
        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT seq, content_parts FROM session_messages WHERE session_id = ?1 AND seq > ?2 ORDER BY seq",
            )
            .map_err(|e| sqlite_error("增量查询准备失败", e))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, since_seq], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| sqlite_error("增量查询执行失败", e))?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, json) = row.map_err(|e| sqlite_error("增量查询行读取失败", e))?;
            if let Some(msg) = parse_message_row(&json, session_id, seq) {
                out.push((seq, msg));
            }
        }
        Ok(out)
    }

    /// 取 seq 严格小于 `before_seq` 的**最近** `limit` 条（升序返回）。
    ///
    /// 供前端分段加载（上滚取更早历史，ADR-035 §8）：「最近一页」由调用方传
    /// `before_seq = last_seq + 1` 表达。SQL 用 `ORDER BY seq DESC LIMIT n`
    /// 只扫目标区间（不读全链），返回前反转为升序（与链序一致）。
    ///
    /// **逐行容错（T1-17）**：损坏行跳过并记 warn（带真实 seq，空洞无害）。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError（行级 JSON 损坏不返回错误）。
    pub async fn load_before(
        &self,
        session_id: &str,
        before_seq: i64,
        limit: usize,
    ) -> Result<Vec<(i64, StructuredMessage)>, TianyanError> {
        let conn = self.db.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT seq, content_parts FROM session_messages WHERE session_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
            )
            .map_err(|e| sqlite_error("分页查询准备失败", e))?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id, before_seq, limit as i64],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .map_err(|e| sqlite_error("分页查询执行失败", e))?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, json) = row.map_err(|e| sqlite_error("分页查询行读取失败", e))?;
            if let Some(msg) = parse_message_row(&json, session_id, seq) {
                out.push((seq, msg));
            }
        }
        out.reverse(); // DESC 取页 → 反转为升序（与链序一致）
        Ok(out)
    }

    /// 取 seq 严格小于 `before_seq` 的**最近一条** `role=user` 消息的 seq。
    ///
    /// 供分段加载的页边界对齐（user 对齐，ADR-035 §8 修订）：页从用户消息
    /// 起——工具调用与其结果不会被分页切开（消除跨页"孤立结果卡/缺结果
    /// 调用"渲染）。无更早用户消息返回 `None`。
    ///
    /// # Errors
    /// * SQLite 查询失败时返回 TianyanError。
    pub async fn last_user_seq_before(
        &self,
        session_id: &str,
        before_seq: i64,
    ) -> Result<Option<i64>, TianyanError> {
        let conn = self.db.lock().await;
        conn.query_row(
            "SELECT MAX(seq) FROM session_messages WHERE session_id = ?1 AND role = 'user' AND seq < ?2",
            rusqlite::params![session_id, before_seq],
            |r| r.get::<_, Option<i64>>(0),
        )
        .map_err(|e| sqlite_error("最近用户消息序号查询失败", e))
    }

    /// 删除会话（单事务：清 FTS + 消息 + 元数据）。
    ///
    /// ADR-026 A：级联删除——主会话删除时连带删除其所有子智能体会话
    /// （子会话是主会话的附属，主会话没了子会话记录无独立价值）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError。
    pub async fn delete(&self, session_id: &str) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| sqlite_error("删除事务开启失败", e))?;
        // 级联：**递归**删除全部后代会话（子/孙/…）三表清理——嵌套委托会
        // 产生多层子会话，旧实现只删一层（parent = 本会话），孙会话成为孤儿：
        // 不进列表、消息与 FTS 永久留存、SessionRecall 仍命中不可访问会话（T0-13）。
        let cte = DESCENDANT_CTE;
        tx.execute(
            &format!("{cte} DELETE FROM session_messages_fts WHERE rowid IN (SELECT id FROM session_messages WHERE session_id IN (SELECT session_id FROM descendants))"),
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("级联删除子会话 FTS 清理失败", e))?;
        tx.execute(
            &format!("{cte} DELETE FROM session_messages WHERE session_id IN (SELECT session_id FROM descendants)"),
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("级联删除子会话消息失败", e))?;
        tx.execute(
            &format!("{cte} DELETE FROM session_meta WHERE session_id IN (SELECT session_id FROM descendants)"),
            rusqlite::params![session_id],
        )
        .map_err(|e| sqlite_error("级联删除子会话元数据失败", e))?;
        // 本会话三表清理
        tx.execute(
            "DELETE FROM session_messages_fts WHERE rowid IN (SELECT id FROM session_messages WHERE session_id = ?1)",
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

    /// ADR-026 B：子智能体会话上限保护——总数超 max_children 时，
    /// 删"最不活跃主会话"（其子会话 updated_at 最旧）的**全部后代**，
    /// 循环至 ≤ 上限。惰性触发（创建子会话时调用）。
    ///
    /// # Errors
    /// * SQLite 写入失败时返回 TianyanError。
    pub async fn enforce_child_session_limit(
        &self,
        max_children: usize,
    ) -> Result<(), TianyanError> {
        let conn = self.db.lock().await;
        loop {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_meta WHERE parent_session_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| sqlite_error("子会话计数查询失败", e))?;
            if count <= max_children as i64 {
                break;
            }
            // 候选限定为**根会话**（自身无父、且有后代）：只有从根递归删除才能
            // 一次清掉整棵树——旧实现按 parent_session_id 分组，会把嵌套中间
            // 节点也当候选（删掉中间节点后其祖先仍在 → 孙会话成孤儿，T0-13）。
            // 排序：其子会话最新活跃时间升序（最旧者先删）。
            let parent: Option<String> = conn
                .query_row(
                    "SELECT m.session_id FROM session_meta m
                     WHERE m.parent_session_id IS NULL
                       AND EXISTS (SELECT 1 FROM session_meta c WHERE c.parent_session_id = m.session_id)
                     ORDER BY (SELECT MAX(updated_at) FROM session_meta c WHERE c.parent_session_id = m.session_id) ASC
                     LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| sqlite_error("最不活跃主会话查询失败", e))?;
            let Some(parent) = parent else {
                break;
            };
            // 单事务删除该根会话的全部后代（旧实现三条 execute 非事务：中途
            // 失败会留下"消息已删、元数据还在"的半删状态，T0-13）。
            let cte = DESCENDANT_CTE;
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| sqlite_error("上限清理事务开启失败", e))?;
            tx.execute(
                &format!("{cte} DELETE FROM session_messages_fts WHERE rowid IN (SELECT id FROM session_messages WHERE session_id IN (SELECT session_id FROM descendants))"),
                rusqlite::params![parent],
            )
            .map_err(|e| sqlite_error("上限清理子会话 FTS 失败", e))?;
            tx.execute(
                &format!("{cte} DELETE FROM session_messages WHERE session_id IN (SELECT session_id FROM descendants)"),
                rusqlite::params![parent],
            )
            .map_err(|e| sqlite_error("上限清理子会话消息失败", e))?;
            tx.execute(
                &format!("{cte} DELETE FROM session_meta WHERE session_id IN (SELECT session_id FROM descendants)"),
                rusqlite::params![parent],
            )
            .map_err(|e| sqlite_error("上限清理子会话元数据失败", e))?;
            tx.commit()
                .map_err(|e| sqlite_error("上限清理事务提交失败", e))?;
        }
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
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
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
    async fn test_load_tolerates_corrupt_row() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        for (id, text) in [("m0", "第一"), ("m1", "第二"), ("m2", "第三")] {
            store
                .append_message("s1", &msg(id, MessageRole::User, text))
                .await
                .unwrap();
        }
        // 模拟落盘损坏：中间一行 content_parts 变成非法 JSON
        {
            let conn = store.db.lock().await;
            conn.execute(
                "UPDATE session_messages SET content_parts = '{oops' WHERE session_id = 's1' AND seq = 1",
                [],
            )
            .unwrap();
        }

        // 全量加载：不失败；损坏行以占位顶替，位置仍与 seq 对齐
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].id, "m0");
        assert!(matches!(messages[1].role, MessageRole::System));
        assert_eq!(messages[2].id, "m2");

        // 区间查询：损坏行跳过（带真实 seq，空洞无害）
        let after = store.load_after("s1", -1).await.unwrap();
        assert_eq!(
            after.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![0, 2]
        );
        let before = store.load_before("s1", 3, 10).await.unwrap();
        assert_eq!(
            before.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[tokio::test]
    async fn test_last_user_seq_before() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        // u0, a1, a2, u3, a4（user 位于 seq 0、3）
        for (id, role) in [
            ("m0", MessageRole::User),
            ("m1", MessageRole::Assistant),
            ("m2", MessageRole::Assistant),
            ("m3", MessageRole::User),
            ("m4", MessageRole::Assistant),
        ] {
            store
                .append_message("s1", &msg(id, role, "x"))
                .await
                .unwrap();
        }
        assert_eq!(store.last_user_seq_before("s1", 5).await.unwrap(), Some(3));
        assert_eq!(store.last_user_seq_before("s1", 4).await.unwrap(), Some(3));
        assert_eq!(store.last_user_seq_before("s1", 3).await.unwrap(), Some(0));
        assert_eq!(store.last_user_seq_before("s1", 1).await.unwrap(), Some(0));
        assert_eq!(store.last_user_seq_before("s1", 0).await.unwrap(), None);
        // 无 user 的会话
        store.create("s2", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s2", &msg("n0", MessageRole::Assistant, "x"))
            .await
            .unwrap();
        assert_eq!(store.last_user_seq_before("s2", 10).await.unwrap(), None);
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
        let hits = recall.search("正文内容", 10, None).await.unwrap();
        assert_eq!(hits.len(), 1);
        let hits = recall.search("不存在的词", 10, None).await.unwrap();
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
        assert!(recall.search("x", 10, None).await.unwrap().is_empty());
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

    /// 构造子会话 header（parent = 指定会话）。
    fn child_header(parent: &str) -> SessionHeader {
        SessionHeader {
            parent_session_id: Some(parent.to_string()),
            kind: Some("delegate".to_string()),
            ..SessionHeader::default()
        }
    }

    /// T0-13（主回归）：删除主会话必须级联到**全部后代**（子/孙）——
    /// 嵌套委托会产生多层子会话，旧实现只删一层，孙会话成为孤儿。
    #[tokio::test]
    async fn test_delete_cascades_recursively_to_grandchildren() {
        let store = make_store().await;
        store
            .create("main", &SessionHeader::default())
            .await
            .unwrap();
        store.create("child", &child_header("main")).await.unwrap();
        store.create("grand", &child_header("child")).await.unwrap();
        store
            .append_message("child", &msg("c1", MessageRole::Assistant, "儿童消息"))
            .await
            .unwrap();
        store
            .append_message("grand", &msg("g1", MessageRole::Assistant, "孙消息"))
            .await
            .unwrap();

        store.delete("main").await.unwrap();

        assert!(store.load("main").await.unwrap().is_none());
        assert!(
            store.load("child").await.unwrap().is_none(),
            "子会话必须删除"
        );
        assert!(
            store.load("grand").await.unwrap().is_none(),
            "孙会话必须删除（T0-13：旧实现只删一层）"
        );
        // 三表无残留（孙会话的消息与 FTS 索引也不得留下）
        let conn = store.db.lock().await;
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "消息表不得残留被删会话的消息");
        let fts: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_messages_fts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fts, 0, "FTS 索引不得残留被删会话的消息");
    }

    /// T0-13：上限清理从**根会话**递归删整棵树（不留孤儿）。
    #[tokio::test]
    async fn test_enforce_child_session_limit_removes_whole_tree() {
        let store = make_store().await;
        store
            .create("main", &SessionHeader::default())
            .await
            .unwrap();
        for i in 0..3 {
            let child = format!("child-{i}");
            store.create(&child, &child_header("main")).await.unwrap();
            store
                .create(&format!("grand-{i}"), &child_header(&child))
                .await
                .unwrap();
        }
        // 6 个子会话（含孙）> 上限 2 → 清理至 ≤ 2
        store.enforce_child_session_limit(2).await.unwrap();

        let conn = store.db.lock().await;
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_meta WHERE parent_session_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(left <= 2, "清理后子会话数应 ≤ 上限，实际 {left}");
        // 无孤儿：剩余子会话的父必须存在（旧实现删中间节点，孙会话成孤儿）
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_meta m WHERE m.parent_session_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM session_meta p WHERE p.session_id = m.parent_session_id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "清理不得留下孤儿会话（T0-13）");
    }
}
