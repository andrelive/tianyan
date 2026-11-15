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
        // busy_timeout：写冲突（另一个进程 / 运维工具持写锁）时自旋等待 5 秒，
        //   而不是立刻返回 "database is locked"。它**不解决**双写分叉（那由 ADR-040
        //   单写者 + `desktop::ensure_server` 的 data_dir 校验拦），只把偶发冲突
        //   从「立刻失败」变成「等一下」。
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; \
             PRAGMA busy_timeout=5000;",
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
            // 幂等迁移：补 `first_request_tokens` 缓存列（列表查询不再解析 JSON）。
            // 老数据的值由运维工具 `backfill_first_request_tokens` 一次性回填；
            // 未回填时该列保持 NULL（列表侧按「基线未知」降级，不影响正确性）。
            let has_first_tokens_col: bool = {
                let mut stmt = conn.prepare("PRAGMA table_info(session_meta)")?;
                let cols = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                cols.iter().any(|c| c == "first_request_tokens")
            };
            if !has_first_tokens_col {
                conn.execute(
                    "ALTER TABLE session_meta ADD COLUMN first_request_tokens INTEGER",
                    [],
                )?;
            }
            // 幂等迁移：补 `visible_from_seq`（回退水位，ADR-056）。
            //
            // **必须补**：新库由 `SCHEMA_SQL` 建表时带该列，老库没有 → `list_meta`
            // 里 `sql < COALESCE(visible_from_seq, MAX)` 的可见性条件直接
            // `no such column: sm.visible_from_seq`，**整个会话列表 500** 且前端
            // 一片空白（1.2.7 实发事故：单测都在内存库上跑、走的是 SCHEMA_SQL 建表，
            // 唯一测不出来的就是「老库缺列」这条路径）。
            let has_visible_col: bool = {
                let mut stmt = conn.prepare("PRAGMA table_info(session_meta)")?;
                let cols = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                cols.iter().any(|c| c == "visible_from_seq")
            };
            if !has_visible_col {
                conn.execute(
                    "ALTER TABLE session_meta ADD COLUMN visible_from_seq INTEGER",
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
            let cols: Vec<String> = {
                let mut stmt = conn.prepare("PRAGMA table_info(background_tasks)")?;
                let collected = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                collected
            };
            if !cols.iter().any(|c| c == "kind") {
                conn.execute(
                    "ALTER TABLE background_tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'delegate'",
                    [],
                )?;
            }
            // 幂等迁移（U5）：补 working_directory 列——task_status 默认视野
            // 按归属工作目录过滤；旧库缺列时 SELECT 报 no such column。
            if !cols.iter().any(|c| c == "working_directory") {
                conn.execute(
                    "ALTER TABLE background_tasks ADD COLUMN working_directory TEXT",
                    [],
                )?;
            }
        }
        // 幂等迁移（0.6.8）：usage_logs 补请求性能列（TTFT / 总耗时，毫秒；
        // -1 = 未知——历史行与非流式路径）。底部统计条展示最近一次请求的
        // TTFT 与生成速度。同样必须在 SCHEMA_SQL 之前（CREATE TABLE 对已存在
        // 表不生效）。ADD COLUMN 为元数据级操作（不重写表）；单行 +5~6 B。
        let has_usage_table: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='usage_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if has_usage_table {
            let cols: Vec<String> = {
                let mut stmt = conn.prepare("PRAGMA table_info(usage_logs)")?;
                let collected = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                collected
            };
            if !cols.iter().any(|c| c == "ttft_ms") {
                conn.execute(
                    "ALTER TABLE usage_logs ADD COLUMN ttft_ms INTEGER NOT NULL DEFAULT -1",
                    [],
                )?;
            }
            if !cols.iter().any(|c| c == "duration_ms") {
                conn.execute(
                    "ALTER TABLE usage_logs ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT -1",
                    [],
                )?;
            }
        }
        // 迁移：vfs_entries 补 `importance` 列。早期这个值只活在记忆条目 detail 的
        // markdown 里（「- **重要性**: 0.76」），而 `read_entry` 重建元数据时取默认
        // 0.5 —— 界面显示、按重要性排序、以及任何加权计算拿到的都是假值。
        if has_vfs_entries {
            let cols: Vec<String> = {
                let mut stmt = conn.prepare("PRAGMA table_info(vfs_entries)")?;
                let collected = stmt
                    .query_map([], |r| r.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                collected
            };
            if !cols.iter().any(|c| c == "importance") {
                conn.execute(
                    "ALTER TABLE vfs_entries ADD COLUMN importance REAL NOT NULL DEFAULT 0.5",
                    [],
                )?;
            }
            if !cols.iter().any(|c| c == "access_count") {
                conn.execute(
                    "ALTER TABLE vfs_entries ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            if !cols.iter().any(|c| c == "last_accessed") {
                conn.execute("ALTER TABLE vfs_entries ADD COLUMN last_accessed TEXT", [])?;
            }
        }
        conn.execute_batch(SCHEMA_SQL)?;
        // 幂等迁移：`idx_session_messages_msgid` 升级为 UNIQUE（见函数文档）。
        // 必须在 SCHEMA_SQL 之后：它要读 `sqlite_master` 里实际建成的索引定义
        // （`CREATE UNIQUE INDEX IF NOT EXISTS` 对**已存在的同名普通索引**会短路）。
        ensure_msgid_unique_index(&conn)?;
        Ok(())
    }
}

/// 把 `idx_session_messages_msgid` 升级为 UNIQUE（幂等；失败降级 + 告警）。
///
/// **为什么要显式比对**：老库上同名索引是**非唯一**的，而 `SCHEMA_SQL` 里的
/// `CREATE UNIQUE INDEX IF NOT EXISTS` 会因为「索引已存在」直接短路——拿不到
/// 唯一性，必须先 DROP 再建。
///
/// **为什么值得 UNIQUE**：`find_seq` 用 `LIMIT 1` 取一条来定位回退锚点。消息 ID
/// 的唯一性只由进程内的 `AtomicU64` 计数器保证（`msg_{ms}_{seq}`），跨进程双写、或
/// 同一毫秒内重启（计数器归零）都可能重复 → `find_seq` 返回**不确定的那条** →
/// 回退静默错位、前端列表 key 冲突。唯一索引把「静默错位」变成「写入时明确失败」。
///
/// **失败降级**：存量若有重复行（历史脏数据），建唯一索引会失败——此时重建普通
/// 索引并告警，**不让服务起不来**（重复行需人工处理；数据安全优先于索引完备）。
fn ensure_msgid_unique_index(conn: &Connection) -> Result<(), rusqlite::Error> {
    const NAME: &str = "idx_session_messages_msgid";
    let existing: Option<String> = match conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
        [NAME],
        |r| r.get::<_, Option<String>>(0),
    ) {
        Ok(sql) => sql,
        // 索引不存在（表还没建 / 首次迁移）→ 交给 SCHEMA_SQL 建
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(()),
        Err(e) => return Err(e),
    };
    if existing
        .as_deref()
        .is_some_and(|sql| sql.to_ascii_uppercase().contains("UNIQUE"))
    {
        return Ok(());
    }
    conn.execute(&format!("DROP INDEX IF EXISTS {NAME}"), [])?;
    let ddl = |unique: bool| {
        format!(
            "CREATE {}INDEX IF NOT EXISTS {NAME} ON session_messages(session_id, message_id)",
            if unique { "UNIQUE " } else { "" }
        )
    };
    match conn.execute_batch(&ddl(true)) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "会话消息 ID 索引升级为 UNIQUE 失败（存量存在重复 message_id？）——\
                 降级保留普通索引；重复 ID 上的 `find_seq` 定位可能不确定"
            );
            conn.execute_batch(&ddl(false))
        }
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
        -- 条目重要性（0.0–1.0）。**必须结构化落库**：早期只有记忆条目的 detail
        -- markdown 里写着「重要性: 0.76」，而 `read_entry` 重建元数据时取默认 0.5
        -- ——界面/排序/加权拿到的全是假值（实测记忆页 131 条清一色 0.50）。
        created_at       TEXT DEFAULT (datetime('now')),
        updated_at       TEXT DEFAULT (datetime('now')),
        importance       REAL NOT NULL DEFAULT 0.5,
        -- 使用计数与最后使用时间：只有「检索命中并注入上下文」才累加
        access_count     INTEGER NOT NULL DEFAULT 0,
        last_accessed    TEXT
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
        seq                INTEGER NOT NULL,
        working_directory  TEXT
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
    -- 按消息 ID 定位 seq（回退/重做的锚点定位）：覆盖索引，含 seq 不回表——
    -- 没有它只能全量加载整条链再线性找位置（实测 1300+ 条会话每次回退多花上百毫秒）
    -- UNIQUE：`find_seq` 以 `LIMIT 1` 定位回退锚点，ID 必须唯一（见
    -- `ensure_msgid_unique_index` 的文档——升级老库的非唯一同名索引走那里）。
    CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_msgid ON session_messages(session_id, message_id);

    -- 会话级状态（ADR-018：替代 JSONL 首行 SessionHeader；
    -- injectable 快照 + title/ended_at/created_at 唯一持久化 home）
    CREATE TABLE IF NOT EXISTS session_meta (
        session_id  TEXT PRIMARY KEY,
        header_json TEXT NOT NULL DEFAULT '{}',
        created_at  INTEGER,
        updated_at  TEXT DEFAULT (datetime('now')),
        -- ADR-026：子智能体会话关联主会话（NULL = 主会话；查询/级联/过滤直接走 SQL）
        parent_session_id TEXT,
        -- 会话**首条带输入用量的 assistant** 的 `tokens.input`（缓存列；NULL = 未知）。
        -- 会话列表直接读它：此前是用子查询「找 seq 最小且 tokens.input > 0 的
        -- assistant」，其 `json_extract` 条件要求逐条解析 JSON —— 实测 52 个会话 /
        -- 13.6 万条消息时该查询 1802 ms（每次列表刷新都付）。列在首条落库时固化一次
        -- （`rewrite` 时按内存里的完整链重算），老数据由 `backfill_first_request_tokens`
        -- 一次性回填；未回填期间列表返回 NULL，前端按「基线缺失」回退原行为。
        first_request_tokens INTEGER,
        -- 回退水位（ADR-056）：`seq >= visible_from_seq` 的行是「已回退但尚未清理」
        -- 的重做数据——它们**仍在表里**（重做 = 把水位移回 NULL，一行都不用搬），
        -- 但**对一切读不可见**。NULL = 全部可见。
        -- ⚠️ 不变式：所有读 `session_messages` 的 SQL 必须带可见性条件
        -- （`SessionStore::visible_cond`），漏一处就会读到已回退的内容
        -- （FTS 能搜到、列表 count 虚高）——这类错误是静默的。
        visible_from_seq INTEGER
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
        -- 请求性能（毫秒；-1 = 未知——历史行 / 非流式路径 / 无 usage）
        ttft_ms           INTEGER NOT NULL DEFAULT -1,
        duration_ms       INTEGER NOT NULL DEFAULT -1,
        ts                INTEGER NOT NULL,
        recorded_at       TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_usage_logs_ts ON usage_logs(ts);
    CREATE INDEX IF NOT EXISTS idx_usage_logs_model ON usage_logs(provider, model);
    -- 会话级统计（`/usage/stats?session_id=` 与「每步刷新底部统计行」）都按 session
    -- 过滤：没有这条索引就得全表扫 —— 而表随每次 LLM 调用长一行（含子代理与演化
    -- 任务），量级上来后每步一次查询会变成几十毫秒的固定开销。
    CREATE INDEX IF NOT EXISTS idx_usage_logs_session ON usage_logs(session_id, ts);

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

    /// 回归（U5）：旧库 background_tasks 表无 working_directory 列时，
    /// init_all_schemas 幂等补列（task_status 归属过滤依赖该列；缺列时
    /// 持久化 SELECT 报 no such column）。
    #[tokio::test]
    async fn test_migration_adds_working_directory_to_background_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy-bt-wd.db");
        {
            // 模拟旧库：background_tasks 有 kind、无 working_directory 列
            let db = SqliteDb::open(db_path.clone()).unwrap();
            {
                let conn = db.lock().await;
                conn.execute_batch(
                    "CREATE TABLE background_tasks (
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
                    INSERT INTO background_tasks (id, kind, description, status, parent_session_id, created_at, seq)
                    VALUES ('bt_legacy', 'delegate', '旧任务', 'completed', 's1', 0, 1);",
                )
                .unwrap();
            }
            db.init_all_schemas().await.expect("旧库迁移应成功");
            let conn = db.lock().await;
            // working_directory 列已补（旧行回填 NULL）
            let wd: Option<String> = conn
                .query_row(
                    "SELECT working_directory FROM background_tasks WHERE id = 'bt_legacy'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(wd.is_none(), "旧行 working_directory 应为 NULL");
            // 含 working_directory 的 SELECT 可执行（持久化加载不再失败）
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM background_tasks WHERE working_directory IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }

    /// 老库上的 `idx_session_messages_msgid` 是**非唯一**的；`CREATE UNIQUE INDEX
    /// IF NOT EXISTS` 会因「索引已存在」短路 → 必须在 `init_all_schemas` 里显式
    /// 比对并重建，否则存量库永远拿不到唯一性（`find_seq` 的 `LIMIT 1` 仍可能
    /// 取到不确定的一条）。
    #[tokio::test]
    async fn test_msgid_index_upgraded_to_unique() {
        let db = SqliteDb::open_in_memory().unwrap();
        {
            let conn = db.lock().await;
            conn.execute_batch(
                "CREATE TABLE session_messages (
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
                CREATE INDEX idx_session_messages_msgid
                    ON session_messages(session_id, message_id);",
            )
            .unwrap();
        }
        db.init_all_schemas().await.expect("旧库迁移应成功");
        let conn = db.lock().await;
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' \
                 AND name = 'idx_session_messages_msgid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.to_ascii_uppercase().contains("UNIQUE"),
            "非唯一索引应被升级为 UNIQUE：{sql}"
        );
    }

    /// 存量若有重复 `message_id`（跨进程双写 / 同一毫秒内重启的历史脏数据），
    /// 建唯一索引必然失败——**降级保留普通索引 + 告警，不让服务起不来**
    /// （数据安全优先于索引完备；重复行需人工处理）。
    #[tokio::test]
    async fn test_msgid_index_downgrades_on_existing_duplicates() {
        let db = SqliteDb::open_in_memory().unwrap();
        {
            let conn = db.lock().await;
            conn.execute_batch(
                "CREATE TABLE session_messages (
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
                CREATE INDEX idx_session_messages_msgid
                    ON session_messages(session_id, message_id);
                INSERT INTO session_messages (session_id, seq, message_id, role, ts)
                    VALUES ('s1', 0, 'dup', 'user', 1);
                INSERT INTO session_messages (session_id, seq, message_id, role, ts)
                    VALUES ('s1', 1, 'dup', 'user', 2);",
            )
            .unwrap();
        }
        db.init_all_schemas()
            .await
            .expect("存量重复不应让 schema 初始化失败（降级而非报错）");
        let conn = db.lock().await;
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' \
                 AND name = 'idx_session_messages_msgid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !sql.to_ascii_uppercase().contains("UNIQUE"),
            "有重复行时应降级为普通索引：{sql}"
        );
        // 数据仍在（降级不删数据）
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "降级不得触碰存量行");
    }

    /// **1.2.7 实发事故回归**：老库 `session_meta` 没有 `visible_from_seq` 列时，
    /// `init_all_schemas` 必须把它补上。
    ///
    /// 漏了会怎样：`list_meta` 的可见性条件 `sql < COALESCE(visible_from_seq, MAX)`
    /// 直接报 `no such column: sm.visible_from_seq` → 会话列表接口 500 → 前端
    /// 左侧列表一片空白（用户实测 1.2.7 装完必须退回 1.2.6）。
    ///
    /// **为什么单测一直测不出来**：所有测试都在内存库上 `init_schemas` 后使用，
    /// 走的是 `SCHEMA_SQL` 建表（新表自带该列）；唯一会暴露问题的是「已有旧表的
    /// 库跑迁移」这条路径——本测试专门构造它。
    #[tokio::test]
    async fn test_legacy_session_meta_gets_visible_from_seq() {
        let db = SqliteDb::open_in_memory().unwrap();
        {
            let conn = db.lock().await;
            // 模拟 1.2.6 的 session_meta（有 first_request_tokens，没有 visible_from_seq）
            conn.execute_batch(
                "CREATE TABLE session_meta (
                    session_id  TEXT PRIMARY KEY,
                    header_json TEXT NOT NULL DEFAULT '{}',
                    created_at  INTEGER,
                    updated_at  TEXT DEFAULT (datetime('now')),
                    parent_session_id TEXT,
                    first_request_tokens INTEGER
                );
                INSERT INTO session_meta (session_id) VALUES ('s-legacy');",
            )
            .unwrap();
        }
        db.init_all_schemas().await.expect("旧库迁移应成功");

        let conn = db.lock().await;
        let has_col: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(session_meta)").unwrap();
            let cols = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            cols.iter().any(|c| c == "visible_from_seq")
        };
        assert!(
            has_col,
            "老库必须补上 visible_from_seq 列（漏了会让会话列表 500）"
        );
        // 存量行保留，且新列为 NULL（= 全部可见，语义正确）
        let (n, visible): (i64, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(visible_from_seq) FROM session_meta",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n, 1, "迁移不得丢数据");
        assert_eq!(visible, None, "存量会话 = 全部可见");
    }
}
