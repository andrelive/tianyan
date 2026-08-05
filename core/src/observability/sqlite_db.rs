//! 共享 SQLite 数据库连接，供 UsageStats 和 SqliteSessionStore 共用。
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
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

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

    -- 会话
    CREATE TABLE IF NOT EXISTS sessions (
        session_id   TEXT PRIMARY KEY,
        title        TEXT,
        token_count  INTEGER DEFAULT 0,
        created_at   TEXT NOT NULL,
        ended_at     TEXT
    );

    -- 会话消息
    CREATE TABLE IF NOT EXISTS session_messages (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id      TEXT    NOT NULL,
        msg_id          TEXT    NOT NULL,
        parent_id       TEXT,
        role            TEXT    NOT NULL,
        parts_json      TEXT    NOT NULL,   -- JSON: Vec<Part>
        tokens_json     TEXT,               -- JSON: DetailedTokenUsage
        cost            REAL    DEFAULT 0.0,
        model_id        TEXT,
        time_created    INTEGER NOT NULL,
        time_completed  INTEGER NOT NULL,
        finish          TEXT,
        compression_marker INTEGER DEFAULT 0,
        recorded_at     TEXT    NOT NULL DEFAULT (datetime('now')),
        FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_msg_session ON session_messages(session_id);
    CREATE INDEX IF NOT EXISTS idx_msg_marker ON session_messages(session_id, compression_marker);

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
";
