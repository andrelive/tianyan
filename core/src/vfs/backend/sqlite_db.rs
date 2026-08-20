//! 共享 SQLite 数据库连接，供 UsageStats 统计与 VFS 元数据共用。
//!
//! 设计：单个 SQLite 文件，多张表，统一管理 Schema 迁移。

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::{Mutex, MutexGuard, TryLockError};

/// 共享的 SQLite 数据库句柄。
///
/// 内部包装 `Arc<Mutex<Connection>>`，线程安全，支持多消费者。
#[derive(Clone)]
pub struct SqliteDb {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SqliteDb {
    /// 打开（或创建）指定路径的 SQLite 数据库。
    pub fn open(path: PathBuf) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    path = %parent.display(),
                    error = %e,
                    "创建数据库目录失败（数据库打开可能失败）"
                );
            }
        }
        let conn = Connection::open(&path)?;
        // foreign_keys=ON：启用 schema 中声明的 FK 约束（ON DELETE CASCADE 等）
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// 创建内存数据库（测试用）。
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        })
    }

    /// 获取数据库文件路径。
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// 获取内部连接的引用（通过 Mutex 保护）。
    pub async fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().await
    }

    /// 非阻塞获取连接引用（不等待）。
    pub fn try_lock(&self) -> Result<MutexGuard<'_, Connection>, TryLockError> {
        self.conn.try_lock()
    }

    /// 初始化所有表的 Schema。
    pub async fn init_all_schemas(&self) -> Result<(), rusqlite::Error> {
        let conn = self.lock().await;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }
}

/// 统一的数据库 Schema。
const SCHEMA_SQL: &str = "
    -- 技能调用统计
    CREATE TABLE IF NOT EXISTS skill_calls (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        skill_id    TEXT    NOT NULL,
        success     INTEGER NOT NULL,
        time_us     INTEGER NOT NULL,
        recorded_at TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_skill_calls_skill ON skill_calls(skill_id, recorded_at);

    -- 文档访问统计
    CREATE TABLE IF NOT EXISTS doc_access (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        uri         TEXT    NOT NULL,
        event_type  TEXT    NOT NULL,
        score       REAL,
        recorded_at TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_doc_access_uri ON doc_access(uri, event_type);

    -- 搜索查询日志
    CREATE TABLE IF NOT EXISTS daily_search_queries (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        query_text    TEXT    NOT NULL,
        result_count  INTEGER NOT NULL,
        top_namespace TEXT,
        recorded_at   TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_daily_queries_date ON daily_search_queries(recorded_at);

    -- 检索轨迹（单次检索完整过程，观测数据；写入即保留最近 N 条，由 UsageStats 控制）
    CREATE TABLE IF NOT EXISTS retrieval_traces (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        query         TEXT    NOT NULL,
        trace_json    TEXT    NOT NULL,
        total_tokens  INTEGER NOT NULL,
        total_time_ms INTEGER NOT NULL,
        recorded_at   TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_retrieval_traces_date ON retrieval_traces(recorded_at);

    -- VFS 条目内容
    CREATE TABLE IF NOT EXISTS vfs_entries (
        uri              TEXT PRIMARY KEY,
        is_directory     INTEGER DEFAULT 0,
        abstract_content TEXT,
        overview_content TEXT,
        detail_content   TEXT,
        created_at       TEXT DEFAULT (datetime('now')),
        updated_at       TEXT DEFAULT (datetime('now'))
    );

    -- 后台任务状态（ADR-013：任务实体化——状态脱离调用栈持久化，
    -- 重启可查询可恢复；Running/Pending 任务重启后标记为 Failed）
    CREATE TABLE IF NOT EXISTS background_tasks (
        id                 TEXT PRIMARY KEY,
        description        TEXT    NOT NULL,
        status             TEXT    NOT NULL,
        parent_session_id  TEXT    NOT NULL,
        result             TEXT,
        error              TEXT,
        created_at         INTEGER NOT NULL,
        completed_at       INTEGER,
        seq                INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_background_tasks_session ON background_tasks(parent_session_id, seq);

    -- 结构化 Trace（G6）：会话/任务执行 span，支持事后回放与坏例分析。
    -- 按 (session_id, task_id, turn_index) 分组还原调用树；由 TraceCollector
    -- 缓冲批量写入并清理超出保留窗口的旧记录。
    CREATE TABLE IF NOT EXISTS trace_spans (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id    TEXT    NOT NULL,
        task_id       TEXT,
        turn_index    INTEGER,
        kind          TEXT    NOT NULL,
        name          TEXT    NOT NULL,
        detail        TEXT    NOT NULL DEFAULT '',
        duration_ms   INTEGER NOT NULL DEFAULT 0,
        tokens        INTEGER NOT NULL DEFAULT 0,
        success       INTEGER NOT NULL DEFAULT 1,
        error         TEXT,
        recorded_at   TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_trace_spans_session ON trace_spans(session_id, id);
    CREATE INDEX IF NOT EXISTS idx_trace_spans_task ON trace_spans(task_id, id);

    -- GEPA 执行记录（ADR-017：GEPA 数据层；派生统计数据，可重建。
    -- 由 ToolObservabilityListener 每次工具执行后写入，供演化智能体查询）
    CREATE TABLE IF NOT EXISTS executions (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id        TEXT    NOT NULL,
        tool_name         TEXT    NOT NULL,
        category          TEXT    NOT NULL,
        task_description  TEXT    NOT NULL,
        success           INTEGER NOT NULL,
        execution_time_ms INTEGER NOT NULL,
        skills_used       TEXT    NOT NULL DEFAULT '[]',
        steps_json        TEXT    NOT NULL DEFAULT '[]',
        result            TEXT    NOT NULL DEFAULT '',
        ts                INTEGER NOT NULL,
        recorded_at       TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_executions_cat_ts ON executions(category, ts);
    CREATE INDEX IF NOT EXISTS idx_executions_tool_ts ON executions(tool_name, ts);
    CREATE INDEX IF NOT EXISTS idx_executions_session_ts ON executions(session_id, ts);

    -- 会话消息索引（ADR-017 决策 6：FTS5 会话回忆，方案 A；派生数据，可重建。
    -- JSONL 保持权威；text 仅 user/assistant 文本，工具调用/结果截断存 tool_text）
    CREATE TABLE IF NOT EXISTS session_messages (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id  TEXT    NOT NULL,
        seq         INTEGER NOT NULL,
        message_id  TEXT    NOT NULL,
        role        TEXT    NOT NULL,
        text        TEXT    NOT NULL DEFAULT '',
        tool_text   TEXT    NOT NULL DEFAULT '',
        tokens      INTEGER NOT NULL DEFAULT 0,
        ts          INTEGER NOT NULL,
        recorded_at TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_uniq ON session_messages(session_id, seq);
    CREATE INDEX IF NOT EXISTS idx_session_messages_session_ts ON session_messages(session_id, ts);

    -- LLM 用量日志（token 统计：每次 LLM 调用一行，覆盖聊天/子代理/演化任务；
    -- 由 AgentLoop 每轮拿到 turn_usage 后写入，供统计面板按 provider/model/时段聚合）
    CREATE TABLE IF NOT EXISTS usage_logs (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id        TEXT    NOT NULL,
        provider          TEXT    NOT NULL DEFAULT '',
        model             TEXT    NOT NULL DEFAULT '',
        uncached_input    INTEGER NOT NULL DEFAULT 0,
        cached_input      INTEGER NOT NULL DEFAULT 0,
        completion_tokens INTEGER NOT NULL DEFAULT 0,
        total_tokens      INTEGER NOT NULL DEFAULT 0,
        ts                INTEGER NOT NULL,
        recorded_at       TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_usage_logs_ts ON usage_logs(ts);
    CREATE INDEX IF NOT EXISTS idx_usage_logs_model ON usage_logs(provider, model);

    -- FTS5 倒排索引（trigram tokenizer：中文子串匹配；rowid 对应 session_messages.id）
    CREATE VIRTUAL TABLE IF NOT EXISTS session_messages_fts USING fts5(
        text,
        tokenize = 'trigram'
    );
";
