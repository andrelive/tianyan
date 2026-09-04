# ADR-018: 会话权威存储迁移至 SQLite（VFS 会话例外）

**日期**: 2026-01-01
**状态**: 已采纳（用户确认：不考虑向后兼容，本地数据直接重建）
**影响范围**: 会话存储与检索（`core/src/session/`）、VFS 底层（`core/src/vfs/backend/sqlite_db.rs`）、
调度任务（`core/src/scheduler/tasks/`，删除死任务 memory_task/rule_task）、工具注册表（`core/src/agent/tool_registry/`）、
组合根（`server/src/state.rs`）、AGENTS.md 硬约束

---

## 背景

1. **JSONL 是 LocalFileBackend 时代的存储形态。** 会话以"整块文本"（首行 SessionHeader + 逐行消息）
   存于 VFS。迁移到 SqliteBackend 后（ADR-005），会话"文件"实为 `vfs_entries.detail_content`
   列里的一行文本——伪文件：读取要全量取列再解析（O(n)），追加靠 SQL 字符串拼接，而查询却依赖
   另一张派生索引表 `session_messages`（ADR-017 决策 6）。两套真相并存，需要持续同步。
2. **VFS 的内容模型是"整块文档"**（read/write/append 整块 + L0/L1/L2 摘要检索），对技能/知识库/
   记忆条目等文档型内容完全适配；**会话是唯一的结构性异类**：高频小量追加、历史不可变、
   按行号（seq）寻址、按序访问。会话从未参与 VFS 的摘要/向量检索管线（ADR-017 决策 1 已排除
   Session 命名空间生成 L0/L1；回忆检索是 session 模块自己的 FTS5 体系）。
3. **已知问题**（ADR-017 后逐步暴露）：
   - 每次 `get_session` 全量读 JSONL 并解析（O(n)），长会话每轮对话都承受；`list_sessions`
     对每个会话全量加载（O(n·m)）；
   - seq 分配依赖派生索引（`MAX(seq)+1`），索引写入失败会导致静默漂移、并发追加会撞唯一索引、
     取号失败回退 0 自造漂移；
   - 双存储同步成本：append 需同步写索引、全量覆写需 rebuild、解析格式知识（`parse_message_lines`）
     在多处重复；
   - 会话内容混在 VFS 文档模型中，模糊了"VFS = 文档基座"的语义。
4. **死代码残留**：ADR-017 将记忆/技能/规则/组织形态演化统一到 `evolution` 任务（注册清单只有
   summary_generation / evolution / garbage_collection / usage_stats_flush / snapshot_gc / reminder），
   但 `memory_task.rs` / `rule_task.rs` 文件与导出仍留存，未被注册运行。

---

## 决策

**会话权威存储从 VFS 整块 JSONL 迁移到 SQLite 表（同一 SqliteDb 库），VFS 不再存储会话内容。**
会话成为继快照（ADR-006/008）之后第二个 VFS 例外，AGENTS.md 硬约束相应修订。

### 1. 表结构（`core/src/vfs/backend/sqlite_db.rs`）

- 新表 `session_meta`（会话级状态，替代 JSONL 首行 SessionHeader）：

```sql
CREATE TABLE IF NOT EXISTS session_meta (
    session_id  TEXT PRIMARY KEY,
    header_json TEXT NOT NULL DEFAULT '{}',   -- SessionHeader 完整序列化（injectable 快照等）
    created_at  INTEGER,
    updated_at  TEXT DEFAULT (datetime('now'))
);
```

- `session_messages` 升级为权威消息存储：新增列 `content_parts TEXT NOT NULL DEFAULT ''`
  （完整 StructuredMessage 的 JSON 序列化；`text`/`tool_text`/`tokens`/`ts` 保持为派生列，
   FTS/回忆继续使用）。

### 2. 新模块 `session::store::SessionStore`（持 `Arc<SqliteDb>`）

| 方法 | 语义 |
|------|------|
| `create(session_id, header)` | 建 `session_meta` 行 |
| `append_message(session_id, msg) -> i64` | **单事务**原子取号：`INSERT ... SELECT COALESCE(MAX(seq),-1)+1`，写完整消息（content_parts）+ 条件写 FTS（text 非空时）；失败显式上抛 |
| `load(session_id) -> Option<(SessionHeader, Vec<(i64, StructuredMessage)>)>` | 按 seq 范围查询；按需分页 |
| `rewrite(session_id, header, &[StructuredMessage])` | 单事务：清 FTS + 清消息 + 逐条写（含 FTS） |
| `header(session_id)` / `update_header` | 会话级元数据读写 |
| `list_meta() -> Vec<SessionMeta>` | 轻量列出（不读消息，修掉 O(n·m)） |
| `delete(session_id)` | 单事务：清 FTS + 消息 + meta |
| `export_jsonl(session_id) -> Option<String>` | 生成含 header 的 JSONL 文本（vfs_read 兼容层） |

- `SessionRecall` 保持检索职责（search/window/messages_since 不变，读同一表）；
  **删除 `rebuild_session`（无重建概念）、`next_seq` 与 `index_message`（FTS 维护归 store，
  原子取号并入 append_message）**；
- **`PersistentSessionManager` 不再持有 recall**（索引同步职责从 manager 剥离，归 store）；
- 失败语义变更：存储写入失败**显式上抛**，不再 best-effort debug 日志吞掉。

### 3. `PersistentSessionManager` 改造

- 不再持有 `Arc<dyn VirtualFileSystem>` 与 recall，改持 `Arc<SessionStore>`；`SessionManager` trait
  签名不变——**server/mcp/agent 层零改动**（组合根 `server/src/state.rs` 改为传入 SessionStore）。
- `load_session` 的 compression_marker 截断、MAX_SESSION_MESSAGES 上限逻辑保留。
  （注：ADR-027 已移除这两道截断——存储层返回完整链，压缩点截断移到组装层。）
- title/ended_at/created_at 同步（原 write_session_header）改为 `update_header`。

### 4. 兼容层与死代码清理

- `vfs_read` 工具：对 `tianyan://session/{id}` 特判，经 SessionStore 导出 JSONL 返回——
  LLM"读取压缩前原始记录"的提示语与能力不变（ToolRegistry 注入可选 session_store）；
- **删除死任务**：`memory_task.rs` / `rule_task.rs`（ADR-017 已移除其注册，仅存文件与导出）；
  记忆/技能/规则演化统一由 `evolution` 任务承担，综述智能体经 session_recall（FTS 回忆）访问会话，
  不依赖 JSONL 文本读取；
- `parse_message_lines` / `SessionHeader::parse_line` 保留（导出解析、vfs_read 兼容层用）。

### 5. 数据重建（pre-release：不考虑向后兼容，本地数据直删）

- **不做迁移**：天演尚未上线，无历史会话需要保留。旧结构数据（`vfs_entries` 中的会话行、
  无 `content_parts` 列的 `session_messages`）在初始化时检测到即**直接丢弃重建**；
- 实现：`SqliteDb::init_all_schemas` 前执行幂等清理——
  `PRAGMA table_info(session_messages)` 缺 `content_parts` 列 → `DROP TABLE session_messages_fts/session_messages`；
  `DELETE FROM vfs_entries WHERE uri LIKE 'tianyan://session/%'`；
- 非会话内容（知识库/记忆/技能/角色）完全不受影响。

---

## 理由

1. **单一真相**：消息只存一处（`session_messages` 含完整内容），消灭 JSONL↔索引同步、
   rebuild 概念、格式解析知识在多处的重复。
2. **性能**：append/加载/列表全部 O(1) 或按需范围查询；`list_sessions` 不再全量加载。
3. **正确性**：seq 原子分配（单语句 MAX+1，SQLite 写事务串行）根除并发撞索引；写入失败显式上抛，
   消除静默漂移与回退 0 自造漂移。
4. **语义回归**：VFS 恢复纯净的"文档基座"定位，只服务技能/知识库/记忆等文档型内容；
   会话的流式访问模式由自己的存储承担（参照快照例外 ADR-006 先例）。
5. **抽象边界已就绪**：调研确认 server/mcp/agent 全部经 `SessionManager` trait 访问会话，
   拆存储后端不触碰任何上层 API。
6. **死代码清理**：memory_task/rule_task 已无注册（ADR-017 替代），删除后会话的文本消费路径
   只剩 vfs_read 兼容层，演化流程经 FTS 回忆访问会话。

---

## 反方观点与回应

- **"VFS 统一抽象价值受损，开发者要学两套内容访问路径。"** 回应：统一抽象对文档型内容正确；
  会话是结构性异类（流式 vs 文档），强扭的成本（双存储同步、O(n) 加载、漂移风险）已超过
  统一入口的收益；且 URI 语义（`tianyan://session/{id}`）在 API 层完整保留。
- **"数据有丢失风险。"** 回应：pre-release 无历史数据包袱（用户确认本地数据可重建）；
  非会话内容（知识库/记忆/技能）不触碰。
- **"引入新存储抽象违反 AGENTS.md。"** 回应：硬约束服务于架构，本 ADR 即修订该约束
  （session 成为第二个 VFS 例外，与 snapshot 同列）。

---

## 影响文件清单

- `core/src/vfs/backend/sqlite_db.rs` — schema（session_meta + content_parts 列 + 旧数据清理）
- `core/src/session/store.rs` — 新增 SessionStore
- `core/src/session/search.rs` — 删 rebuild_session/next_seq/index_message，失败上抛
- `core/src/session/manager.rs` — 存储路径重写（VFS → SessionStore）
- `core/src/session/mod.rs` — 导出 SessionStore
- `core/src/scheduler/tasks/memory_task.rs`、`rule_task.rs` — **删除**（死代码）
- `core/src/agent/tool_registry/mod.rs` + `file_ops.rs` — vfs_read 会话特判 + store 注入
- `server/src/state.rs` — 组合根：构造 SessionStore，PersistentSessionManager 改接 store
- `docs/architecture/decisions/018-*.md`（本文档）、`AGENTS.md`（硬约束修订）
- 测试：`manager.rs` 测试从 MockVfs 改写为 SessionStore 内存库；新增 store 单测
  （原子取号并发、export_jsonl 兼容、旧数据重建）

---

## 回滚

无迁移、无备份（pre-release 本地数据）。回滚 = git revert 本 ADR 的代码变更，
会话数据以 SQLite 新表为准（本地数据可整体删除重建）。
