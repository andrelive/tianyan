# ADR-020: 统一写入门面 Database（单连接 + 业务域 Repository）

**日期**: 2026-08-30
**状态**: 已采纳
**影响范围**: 结构化存储（`core/src/db/`）、会话（`core/src/session/`）、可观测性（`core/src/observability/`）、
VFS 底层（`core/src/vfs/backend/sqlite.rs`）、组合根（`server/src/lib.rs`、`server/src/state.rs`）

---

## 背景

1. **8 个组件各自直接持有 `SqliteDb` 并写各自的表**：SessionStore/SessionRecall（session_messages/session_meta + FTS）、
   UsageStats（skill_calls/doc_access/daily_search_queries/retrieval_traces）、TraceCollector（trace_spans）、
   ExecutionLog（executions）、UsageLog（usage_logs）、SqliteBackend（vfs_entries）、BackgroundTask（background_tasks）。
   每个组件直接 `lock()` 连接执行 SQL——连接是共享的（ADR-005 单连接），但**访问入口分散**。
2. **schema 初始化分散**：`SqliteDb::init_all_schemas` 集中创建表，但各组件对"表结构由谁负责"的认知
   依赖文档约定，无单一入口。
3. **组件与连接耦合**：组件字段直接是 `SqliteDb`（值类型，clone 共享 Arc），无法在不改组件的情况下
   替换/装饰连接（如测试内存库、未来连接池）。

## 决策

1. **新建 `core/src/db/` 模块（统一写入门面）**：
   - `Database` 门面：持有 `SqliteDb`（单连接 `Arc<Mutex<Connection>>`），提供 `lock()`/`try_lock()`/`path()`
     统一访问 + `init_schemas()`（schema 集中初始化入口）；
   - `SqliteDb` 归位 `db/sqlite_db.rs`（自 `vfs/backend` 移入；原位置 re-export 兼容）；
   - 业务域 Repository：`stats.rs`/`trace.rs`/`execution.rs`/`usage.rs`——SQL 从组件收敛到仓储方法。
2. **组件改造**：8 个组件字段 `SqliteDb` → `Arc<Database>`（方法调用 `self.db.lock()` 不变——
   Database 提供兼容 lock）；公共 API 不变。
3. **会话存储归位 session 域**（ADR-018 例外）：会话是结构性异类（流式追加 vs 整块文档），
   `SessionStore`/`SessionRecall` 的 SQL **直接实现在 session 模块**（经 `db::Database` 单连接），
   不依赖 db 层通用仓储——避免 `db → session::types` 依赖（db 保持纯底层）。
4. **db 只依赖 `common`**：Repository 方法接收原始字段/计数（不依赖领域类型如 TokenUsage/ExecutionHistory），
   统计结构（SkillStats/ExecutionStat/TraceSpan/UsageStat）定义在 db 内——**db 是纯底层，无领域依赖**。

## 结果

- **统一入口**：全部结构化存储经 `db::Database` 门面（单连接 + schema 集中 + lock 统一）；
- **SQL 收敛**：~45 条 SQL 从组件移到 Repository/本模块（事务取号/FTS/统计落盘/查询）；
- **层次单向**：`session → db`、`vfs → db`、`observability → db`——db 只依赖 common；
- **组件瘦身**：SessionStore/SessionRecall/UsageStats/TraceCollector/ExecutionLog/UsageLog 变薄（转发/组装）。

## 权衡

- **Vfs 不迁移**：SqliteBackend 是 `StorageBackend` trait 的底层实现（AGENTS.md：SQLite 是 VFS 底层、
  不叠加抽象）——直接经 Database 访问是设计允许，迁移会叠一层抽象。
- **会话归位**：放弃"SessionRepo 在 db"的 2b 结构——会话持久化是 ADR-018 专属（结构性异类），
  归位 session 域消除 `db → session` 依赖，db 保持纯底层。
