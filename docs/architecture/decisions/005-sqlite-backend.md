# ADR-005: SQLite 作为主存储后端

**日期**: 2026-07  
**状态**: ✅ 已落地 —— `SqliteBackend` 已实现并接入 `StorageBackend` seam；e2e 全量（26 用例）在 sqlite 后端验证通过后（2026-08-15），默认后端翻转：`StorageBackendType::default = Sqlite`，本地配置与 e2e 配置均显式 `backend = "sqlite"`。`backend = "local"` 仍可回退本地文件系统。  
**影响范围**: 全局存储 — 结构化数据存储的 adapter seam（`StorageBackend`），默认后端为 SQLite

---

## 背景

天演早期版本使用 `LocalFileBackend`（本地文件系统）存储 VFS 条目内容和会话消息：
- 条目内容分散在 `_abstract.md` / `_overview.md` / `_detail.md` 三个文件中
- 会话消息以 JSONL 文件逐行读写
- 文件锁、目录遍历、手动 JSON 解析逻辑散落在各模块

随着系统演进，这种方案暴露出多个问题：
- **非原子写入**：L0/L1/L2 三层独立写入，崩溃时可能只写入部分层级
- **O(n) 查询**：列出会话、查找条目均需遍历目录并逐个读取文件
- **无聚合能力**：无法高效查询"哪些文档被引用最多"、"哪个技能成功率最低"
- **崩溃恢复脆弱**：依赖手工文件锁

## 决策

采用 **SQLite** 作为 VFS 的结构化数据存储后端，LanceDB 保留用于向量搜索。

### 架构

```
                     VFS trait（不变）
                    /          \
                   /            \
        ContentStore          VfsSearch
        (结构化数据)          (向量搜索)
              │                    │
              ▼                    ▼
        SqliteBackend          LanceDB
    (替代 LocalFileBackend)    (不变)
```

### SQLite 数据库结构

单个 `tianyan.db` 文件，包含以下表：

```sql
-- VFS 条目内容（替代 _abstract.md / _overview.md / _detail.md）
vfs_entries(uri, is_directory, abstract_content, overview_content, detail_content)

-- 会话管理（替代 JSONL 文件）
sessions(session_id, title, created_at)
session_messages(session_id, msg_id, role, parts_json, tokens_json, ...)

-- 使用统计（新增）
skill_calls(skill_id, success, time_us, recorded_at)
doc_access(uri, event_type, score, recorded_at)
daily_search_queries(query_text, result_count, top_namespace)
```

### 共享连接设计

`SqliteDb` 是对 `Arc<Mutex<Connection>>` 的封装，单个数据库文件在进程内共享：

```
server boot → SqliteDb::open("tianyan.db") ─┬→ SqliteBackend  → VFS 内容存储
                                             ├→ SqliteSessionStore → 会话消息
                                             └→ UsageStats         → 统计追踪
```

### 热路径优化

高频写入路径（工具调用计数、文档检索命中）使用 `DashMap<_, AtomicU64>` 内存计数器，定时批量刷入 SQLite，避免每条记录都触发一次 `INSERT`。

## 后果

### 正面
- **原子写入**：`INSERT OR REPLACE` 单条语句完成三层内容写入
- **O(log n) 查询**：B-tree 索引替代目录遍历
- **内置聚合**：`COUNT` / `SUM` / `GROUP BY` 替代手动 JSON 扫描
- **崩溃恢复**：WAL 模式自动恢复
- **零运维**：`rusqlite` 的 `bundled` feature 编译静态链接 `libsqlite3`，用户无感知

### 对子系统的约束
- **VFS trait 不变**：所有消费者继续通过 `dyn VirtualFileSystem` 操作，不感知底层后端
- **禁止直接操作 SqliteDb**：各模块不得绕过 VFS trait 直接读写 SQLite（统计模块除外）
- **禁止引入第二个 SQLite 连接**：全系统共用一个 `SqliteDb` 实例

### 已删除组件
- `LocalFileBackend`（`core/src/vfs/backend/local.rs`）— 由 `SqliteBackend` 替代
- `UriMapper`（`core/src/vfs/uri_mapper.rs`）— URI→路径映射不再需要

## 关键文件

- `core/src/vfs/backend/sqlite.rs` — `SqliteBackend` 实现
- `core/src/vfs/backend/sqlite_db.rs` — 共享 `SqliteDb` 连接（自 `observability/` 下沉，ADR-007）
- `core/src/observability/usage_stats.rs` — 使用统计追踪
- `server/src/lib.rs` — `create_app()` 中统一初始化 `SqliteDb`
