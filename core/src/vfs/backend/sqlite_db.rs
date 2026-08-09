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
";
