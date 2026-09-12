//! 共享 SQLite 数据库连接，供 UsageStats 统计与 VFS 元数据共用。
//!
//! 设计：单个 SQLite 文件，多张表，统一管理 Schema 迁移。

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;

use crate::common::error::TianyanError;
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
    ///
    /// 错误在模块边界收敛为 [`TianyanError`]：rusqlite 的错误类型不外泄到
    /// `db` 模块之外（抽象泄漏收敛，ADR-020）。
    pub fn open(path: PathBuf) -> Result<Self, TianyanError> {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    path = %parent.display(),
                    error = %e,
                    "创建数据库目录失败（数据库打开可能失败）"
                );
            }
        }
        let conn = Connection::open(&path).map_err(|e| Self::open_error(&path, e))?;
        // foreign_keys=ON：启用 schema 中声明的 FK 约束（ON DELETE CASCADE 等）
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| Self::open_error(&path, e))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// 创建内存数据库（测试用）。
    pub fn open_in_memory() -> Result<Self, TianyanError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| TianyanError::Custom(format!("db: 打开内存数据库失败：{e}")))?;
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

    /// 打开失败的错误映射（路径 + 原因）。
    fn open_error(path: &std::path::Path, e: rusqlite::Error) -> TianyanError {
        TianyanError::Custom(format!("db: 打开数据库失败（{}）：{e}", path.display()))
    }

    /// 初始化所有表的 Schema。
    ///
    /// 错误在模块边界收敛为 [`TianyanError`]（实现内部仍用 rusqlite 错误，
    /// 但不再出现在公开签名上）。
    pub async fn init_all_schemas(&self) -> Result<(), TianyanError> {
        self.init_all_schemas_inner()
            .await
            .map_err(|e| TianyanError::Custom(format!("db: 初始化 schema 失败：{e}")))
    }

    /// schema 初始化的实现（内部使用 rusqlite 错误；私有，不外泄）。
    async fn init_all_schemas_inner(&self) -> Result<(), rusqlite::Error> {
        let conn = self.lock().await;
        // ADR-018：会话权威存储迁出 VFS（pre-release 本地数据，直接重建）。
        // 1) 清理 VFS 中残留的会话条目（旧 JSONL 伪文件）；
        // 2) 旧结构 session_messages（无 content_parts 列）直接 DROP 重建。
        let has_vfs_entries: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='vfs_entries')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_vfs_entries {
            conn.execute(
                "DELETE FROM vfs_entries WHERE uri LIKE 'tianyan://session/%'",
                [],
            )?;
        }
        let has_content_parts: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(session_messages)")?;
            let cols = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            cols.iter().any(|c| c == "content_parts")
        };
        if !has_content_parts {
            conn.execute_batch(
                "DROP TABLE IF EXISTS session_messages_fts; DROP TABLE IF EXISTS session_messages;",
            )?;
        }
        // ADR-026 幂等迁移：旧 session_meta 表补 parent_session_id 列。
        // 必须在 SCHEMA_SQL 之前执行——SCHEMA_SQL 含
        // CREATE INDEX idx_session_meta_parent，旧表无该列时建索引直接报错
        // （0.2.6 首版 bug：迁移放在 execute_batch 之后，旧库启动即失败）。
        // 新库（表不存在）跳过 ALTER，由 SCHEMA_SQL 创建带列的表。
        let has_meta_table: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_meta')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_meta_table {
            let has_parent_col: bool = {
                let mut stmt = conn.prepare("PRAGMA table_info(session_meta)")?;
                let cols = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                cols.iter().any(|c| c == "parent_session_id")
            };
            if !has_parent_col {
                conn.execute(
                    "ALTER TABLE session_meta ADD COLUMN parent_session_id TEXT",
                    [],
                )?;
            }
        }
        // 幂等迁移：旧 background_tasks 表补 kind 列（8817eb7 加列时未迁移，
        // 旧库 SELECT/INSERT 含 kind 报 no such column——后台任务持久化失效）。
        // 同样必须在 SCHEMA_SQL 之前（SCHEMA_SQL 的 CREATE TABLE 对已存在表不生效）。
        let has_bt_table: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='background_tasks')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_bt_table {
            let has_kind_col: bool = {
                let mut stmt = conn.prepare("PRAGMA table_info(background_tasks)")?;
                let cols = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                cols.iter().any(|c| c == "kind")
            };
            if !has_kind_col {
                conn.execute(
                    "ALTER TABLE background_tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'delegate'",
                    [],
                )?;
            }
        }
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
        kind               TEXT    NOT NULL DEFAULT 'delegate',
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

    -- 会话消息权威存储（ADR-018：迁出 VFS，JSONL 不再保存）。
    -- seq 为会话内消息序号（追加顺序；唯一索引防并发撞号）；
    -- content_parts 为完整 StructuredMessage JSON（唯一真相）；
    -- text/tool_text 为派生列（FTS 回忆/窗口查询用；空消息占 seq 但不进 FTS）。
    CREATE TABLE IF NOT EXISTS session_messages (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id    TEXT    NOT NULL,
        seq           INTEGER NOT NULL,
        message_id    TEXT    NOT NULL,
        role          TEXT    NOT NULL,
        text          TEXT    NOT NULL DEFAULT '',
        tool_text     TEXT    NOT NULL DEFAULT '',
        tokens        INTEGER NOT NULL DEFAULT 0,
        ts            INTEGER NOT NULL,
        content_parts TEXT    NOT NULL DEFAULT '',
        recorded_at   TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_uniq ON session_messages(session_id, seq);
    CREATE INDEX IF NOT EXISTS idx_session_messages_session_ts ON session_messages(session_id, ts);

    -- 会话级状态（ADR-018：替代 JSONL 首行 SessionHeader；
    -- injectable 快照 + title/ended_at/created_at 唯一持久化 home）
    CREATE TABLE IF NOT EXISTS session_meta (
        session_id  TEXT PRIMARY KEY,
        header_json TEXT NOT NULL DEFAULT '{}',
        created_at  INTEGER,
        updated_at  TEXT DEFAULT (datetime('now')),
        -- ADR-026：子智能体会话关联主会话（NULL = 主会话；查询/级联/过滤直接走 SQL）
        parent_session_id TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_session_meta_parent ON session_meta(parent_session_id);

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

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_close_releases_file_lock() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db");
        {
            let db = SqliteDb::open(db_path.clone()).unwrap();
            {
                let conn = db.lock().await;
                conn.execute_batch("CREATE TABLE t(x);").unwrap();
            }
        }
        // SqliteDb dropped → Connection should be closed → rename should work
        let probe = dir.path().join("t2.db");
        std::fs::rename(&db_path, &probe).unwrap();
        std::fs::rename(&probe, &db_path).unwrap();
    }

    /// 回归（0.2.6 首版 bug）：旧库 session_meta 表无 parent_session_id 列时，
    /// init_all_schemas 必须先 ALTER 加列再执行 SCHEMA_SQL（SCHEMA_SQL 含
    /// 该列的索引创建，顺序颠倒会 no such column 启动失败）。
    #[tokio::test]
    async fn test_migration_adds_parent_session_id_before_schema_sql() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy.db");
        {
            // 模拟 0.2.5 旧库：session_meta 无 parent_session_id 列
            let db = SqliteDb::open(db_path.clone()).unwrap();
            {
                let conn = db.lock().await;
                conn.execute_batch(
                    "CREATE TABLE session_meta (
                        session_id  TEXT PRIMARY KEY,
                        header_json TEXT NOT NULL DEFAULT '{}',
                        created_at  INTEGER,
                        updated_at  TEXT DEFAULT (datetime('now'))
                    );
                    INSERT INTO session_meta (session_id) VALUES ('legacy-session');",
                )
                .unwrap();
            }
            db.init_all_schemas().await.expect("旧库迁移应成功");
            // 列已添加 + 索引已建
            let conn = db.lock().await;
            let has_col: bool = {
                let mut stmt = conn.prepare("PRAGMA table_info(session_meta)").unwrap();
                let cols = stmt
                    .query_map([], |r| r.get::<_, String>(1))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                cols.iter().any(|c| c == "parent_session_id")
            };
            assert!(has_col, "旧表应补 parent_session_id 列");
            // 旧数据保留
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM session_meta", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 1, "迁移不应丢数据");
        }
    }

    /// 新库（无 session_meta 表）：init_all_schemas 直接创建带列的表 + 索引。
    #[tokio::test]
    async fn test_fresh_db_creates_parent_session_id_column() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("fresh.db");
        let db = SqliteDb::open(db_path).unwrap();
        db.init_all_schemas().await.expect("新库初始化应成功");
        let conn = db.lock().await;
        let has_col: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(session_meta)").unwrap();
            let cols = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            cols.iter().any(|c| c == "parent_session_id")
        };
        assert!(has_col, "新表应含 parent_session_id 列");
    }

    /// 回归：旧库 background_tasks 表无 kind 列（8817eb7 加列时未迁移）时，
    /// init_all_schemas 补列成功，SELECT/INSERT 不再报 no such column。
    #[tokio::test]
    async fn test_migration_adds_kind_to_background_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy-bt.db");
        {
            // 模拟旧库：background_tasks 无 kind 列
            let db = SqliteDb::open(db_path.clone()).unwrap();
            {
                let conn = db.lock().await;
                conn.execute_batch(
                    "CREATE TABLE background_tasks (
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
                    INSERT INTO background_tasks (id, description, status, parent_session_id, created_at, seq)
                    VALUES ('bt_legacy', '旧任务', 'completed', 's1', 0, 1);",
                )
                .unwrap();
            }
            db.init_all_schemas().await.expect("旧库迁移应成功");
            let conn = db.lock().await;
            // kind 列已补（默认 delegate）
            let kind: String = conn
                .query_row(
                    "SELECT kind FROM background_tasks WHERE id = 'bt_legacy'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kind, "delegate", "旧行 kind 应回填默认值");
            // 含 kind 的 SELECT 可执行（后台任务持久化加载不再失败）
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM background_tasks WHERE kind IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }
}
