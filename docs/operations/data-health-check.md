# 天演数据体检（data health check）

> 适用版本：0.3.16（撰写基线 HEAD `19dbbd8`）
> 目的：只读检查 SQLite（`tianyan.db`）与向量库（`lancedb/`）的体量、一致性、脏数据；
> 提供**可直接复制执行**的巡检 SQL 集与解读/处置。全部 SQL 均按 `core/src/db/sqlite_db.rs`
> 的 `SCHEMA_SQL`（约 159–301 行）实写，未核实项标注 **待核实**。

---

## 0. 快速开始

```powershell
# 默认数据目录：%LOCALAPPDATA%\tianyan
$DB = "$env:LOCALAPPDATA\tianyan\tianyan.db"

# 只读打开（WAL 模式下不影响正在运行的服务）
sqlite3 -readonly $DB
# 或（脚本化推荐）
python -c "import sqlite3; c=sqlite3.connect(r'file:$env:LOCALAPPDATA\tianyan\tianyan.db?mode=ro',uri=True); ..."
```

> 提示：DB 使用 `PRAGMA journal_mode=WAL`，运行期会有 `tianyan.db-wal` / `tianyan.db-shm`。
> **只读连接不会阻塞服务**；但不要在服务运行时对其做 `VACUUM`（会长时间独占写锁）。

---

## 1. 表清单（Schema 唯一真相源：`core/src/db/sqlite_db.rs`）

共 **10 张实表 + 1 张 FTS5 虚拟表**（合计 11 个 `sqlite_master` 条目）。

| # | 表名 | 类型 | 关键列 | 用途 |
| --- | --- | --- | --- | --- |
| 1 | `skill_calls` | 实表 | `id, skill_id, success, time_us, recorded_at` | 技能调用统计 |
| 2 | `doc_access` | 实表 | `id, uri, event_type, score, recorded_at` | 文档访问统计 |
| 3 | `daily_search_queries` | 实表 | `id, query_text, result_count, top_namespace, recorded_at` | 搜索查询日志 |
| 4 | `vfs_entries` | 实表 | `uri(PK), is_directory, abstract_content, overview_content, detail_content, created_at, updated_at` | VFS 条目内容（L0/L1/L2） |
| 5 | `background_tasks` | 实表 | `id(PK), kind, description, status, parent_session_id, result, error, created_at, completed_at, seq` | 后台任务/命令状态（ADR-013/026） |
| 6 | `trace_spans` | 实表 | `id, session_id, task_id, turn_index, kind, name, detail, duration_ms, tokens, success, error, recorded_at` | 结构化 Trace（G6） |
| 7 | `executions` | 实表 | `id, session_id, tool_name, category, task_description, success, execution_time_ms, skills_used, steps_json, result, ts, recorded_at` | GEPA 执行记录（ADR-017） |
| 8 | `session_messages` | 实表 | `id, session_id, seq, message_id, role, text, tool_text, tokens, ts, content_parts, recorded_at` | **会话消息权威存储**（ADR-018），`content_parts` = 完整 `StructuredMessage` JSON |
| 9 | `session_meta` | 实表 | `session_id(PK), header_json, created_at, updated_at, parent_session_id` | 会话级状态（标题/父会话引用） |
| 10 | `usage_logs` | 实表 | `id, session_id, provider, model, uncached_input, cached_input, completion_tokens, total_tokens, ts, recorded_at` | LLM 用量（每轮一行） |
| 11 | `session_messages_fts` | **FTS5 虚拟表** | `text`（`tokenize='trigram'`），`rowid` = `session_messages.id` | 中文子串回忆检索 |

**关键口径**

- `session_messages`：`seq` 为会话内追加序号（唯一索引 `idx_session_messages_uniq`）；
  `text` / `tool_text` 是**派生列**（供 FTS/窗口查询），且**被截断**——
  `text` 上限 `MAX_TEXT_CHARS=4000`、`tool_text` 上限 `MAX_TOOL_TEXT_CHARS=500`
  （`core/src/session/store.rs`）。**完整内容只在 `content_parts`**。
- FTS 只索引**非空 `text`**（空消息/纯图片占 `seq` 但不进 FTS）；**子智能体会话**（`persist_no_fts`）
  全程**不写 FTS**（`session_meta.parent_session_id IS NOT NULL` 的会话）。
- `compression_marker` **不是独立列**——它是 `content_parts` JSON 里的一个布尔字段
  （`"compression_marker":true`）；压缩点以 **user 角色**锚定（旧数据可能为 system）。
- 时间列：`ts` / `created_at` / `completed_at` 为**毫秒 epoch**；`recorded_at` / `updated_at`
  等文本列为 SQLite `datetime('now')`（UTC，`YYYY-MM-DD HH:MM:SS`）。

---

## 2. 巡检 SQL 集（可直接复制执行）

> 建议在只读连接的交互会话里依次执行；每条都给出「什么算异常」。

### S1. 体量总览（表行数 + DB 文件大小）

```sql
-- 各表行数
SELECT 'skill_calls' t, COUNT(*) n FROM skill_calls
UNION ALL SELECT 'doc_access', COUNT(*) FROM doc_access
UNION ALL SELECT 'daily_search_queries', COUNT(*) FROM daily_search_queries
UNION ALL SELECT 'vfs_entries', COUNT(*) FROM vfs_entries
UNION ALL SELECT 'background_tasks', COUNT(*) FROM background_tasks
UNION ALL SELECT 'trace_spans', COUNT(*) FROM trace_spans
UNION ALL SELECT 'executions', COUNT(*) FROM executions
UNION ALL SELECT 'session_messages', COUNT(*) FROM session_messages
UNION ALL SELECT 'session_meta', COUNT(*) FROM session_meta
UNION ALL SELECT 'usage_logs', COUNT(*) FROM usage_logs
ORDER BY n DESC;
```

```sql
-- 会话消息体量 TOP-10（按 content_parts 字符总量）
SELECT session_id, COUNT(*) AS msgs, SUM(LENGTH(content_parts)) AS bytes
FROM session_messages
GROUP BY session_id
ORDER BY bytes DESC LIMIT 10;
```

文件级大小（在 shell 里看，含 WAL）：

```powershell
Get-ChildItem "$env:LOCALAPPDATA\tianyan\tianyan.db*" | Select-Object Name, Length
```

### S2. 压缩点分布

```sql
-- 每会话压缩点数量 + 最后一个压缩点的 seq
SELECT session_id,
       COUNT(*)  AS compression_points,
       MAX(seq)  AS last_point_seq
FROM session_messages
WHERE content_parts LIKE '%"compression_marker":true%'
GROUP BY session_id
ORDER BY compression_points DESC;

-- 压缩点角色分布（旧数据可能是 system；组装层会归一为 user）
SELECT role, COUNT(*) AS n
FROM session_messages
WHERE content_parts LIKE '%"compression_marker":true%'
GROUP BY role;
```

### S3. 孤儿 / 超期任务（3 天 TTL）

```sql
-- 终态任务中超期 3 天者（本应被惰性逐出；仍存在 = 逐出未触发或写路径异常）
SELECT id, kind, status, parent_session_id,
       datetime(completed_at/1000,'unixepoch','localtime') AS completed_local
FROM background_tasks
WHERE completed_at IS NOT NULL
  AND completed_at < (CAST(strftime('%s','now') AS INTEGER)*1000 - 3*86400*1000)
ORDER BY completed_at;

-- 孤儿任务：parent_session_id 已不存在
SELECT b.id, b.kind, b.status, b.parent_session_id
FROM background_tasks b
WHERE NOT EXISTS (
    SELECT 1 FROM session_meta s WHERE s.session_id = b.parent_session_id
);

-- 仍在 Running/Pending 的“僵尸”任务（重启后应被标记 Failed）
SELECT id, kind, status, datetime(created_at/1000,'unixepoch','localtime') AS created_local
FROM background_tasks
WHERE status IN ('running','pending')
ORDER BY created_at;
```

### S4. FTS 未索引消息

```sql
-- 主会话中「text 非空但 FTS 无对应行」的消息（索引缺口）
SELECT COUNT(*) AS unindexed_main
FROM session_messages m
JOIN session_meta s ON s.session_id = m.session_id
WHERE s.parent_session_id IS NULL
  AND m.text <> ''
  AND NOT EXISTS (
      SELECT 1 FROM session_messages_fts f WHERE f.rowid = m.id
  );
```

```sql
-- 反向：FTS 有行但 session_messages 已无对应（重写/删除残留）
SELECT COUNT(*) AS orphan_fts
FROM session_messages_fts f
WHERE NOT EXISTS (
    SELECT 1 FROM session_messages m WHERE m.id = f.rowid
);
```

> 说明：子会话（子智能体）的消息**故意不进 FTS**，因此上面第一条已用
> `s.parent_session_id IS NULL` 把子会话排除，避免把它们误报为“缺口”。

### S5. 向量库（LanceDB）与 VFS 条目一致性

向量库是**独立目录** `<data_dir>\lancedb\`，纯 SQL 查不到——需两边分别取数再比对。

```sql
-- SQLite 侧：非目录的 VFS 条目数
SELECT COUNT(*) AS vfs_leaves FROM vfs_entries WHERE is_directory = 0;
```

```python
# Python 侧：LanceDB 行数（表名 = storage.vector.collection_name）
import sqlite3, lancedb
data_dir = r"%LOCALAPPDATA%\tianyan"          # ← 改成实际 data_dir
col = "tianyan_data"                          # ← 改成 storage.vector.collection_name

db = lancedb.connect(data_dir + r"\lancedb")
tbl = db.open_table(col)
print("lancedb rows:", tbl.count_rows())

c = sqlite3.connect(data_dir + r"\tianyan.db")
print("vfs leaves:", c.execute("select count(*) from vfs_entries where is_directory=0").fetchone()[0])
```

> 若你的运行环境没有 Python `lancedb` 包：可临时用 `pip install lancedb`，
> 或跳过此项并标注 **待核实**（本机未实际执行）。

对照 SQL 侧与向量侧的 VFS `uri`（若需精确到条目级，可在两端各自导出 `uri` 集合做差集）：

```sql
-- SQLite 侧 uri 集合（前 20 条示意）
SELECT uri FROM vfs_entries WHERE is_directory = 0 ORDER BY uri LIMIT 20;
```

### S6. 超大工具结果统计

```sql
-- 工具消息按 content_parts 体量 TOP-20（历史大输出存量）
SELECT session_id, seq, role, tokens, LENGTH(content_parts) AS bytes
FROM session_messages
WHERE role = 'tool'
ORDER BY bytes DESC LIMIT 20;

-- 超过 200KB 的工具结果总数
SELECT COUNT(*) AS big_tool_results, SUM(LENGTH(content_parts)) AS total_bytes
FROM session_messages
WHERE role = 'tool' AND LENGTH(content_parts) > 200000;
```

---

## 3. 解读与处置

| 巡检项 | 什么结果算异常 | 处置 |
| --- | --- | --- |
| **S1 体量** | 单会话 `content_parts` 总量异常大（如 > 50MB）；`session_messages` 行数远超预期 | 定位大消息（S6）；确认读通道治理（0.3.14 起 `read_file`/`execute_command` 已截断）；历史存量不自动收缩，必要时按会话导出后清理 |
| **S2 压缩点** | 出现 `role='system'` 的压缩点（旧数据）；或某会话压缩点异常密集 | `system` 压缩点是**旧数据**，组装层会归一为 user，无需手改；密集压缩关注 §“压缩阈值 60%”口径是否被误触发 |
| **S3 超期任务** | `completed_at` 超 3 天仍在库；或存在 `running/pending` 僵尸 | 终态超期本应由 **TTL 逐出（3 天，惰性，`snapshot()` 时触发）** 删除——若仍在，说明 snapshot 未被调用或写路径失败（检查日志 `后台任务 TTL 逐出失败`）；僵尸 Running/Pending 由启动迁移标记 Failed，否则检查启动流程 |
| **S3 孤儿任务** | `parent_session_id` 无对应 `session_meta` | 主会话删除应级联删子会话/任务；出现孤儿说明级联未跑，可手工清理（见下） |
| **S4 FTS 缺口** | `unindexed_main` 显著 > 0 | 正常情况应为 0（`append` 与 FTS 同事务写入）。非 0 说明历史写入路径异常或旧库未重建；可触发 `session_recall` 前先重建 FTS（见下） |
| **S4 FTS 残留** | `orphan_fts` > 0 | 重写/删除的 FTS 清理未完成；重建 FTS 修正 |
| **S5 向量一致性** | LanceDB 行数 ≠ VFS 叶子数（明显偏差） | 向量是**可从 VFS 摘要文本再生的派生数据**：维度不一致时启动会**自动重建表**、空表由**启动期回填**重嵌入；若长期不平，删除 `<data_dir>\lancedb\` 目录后重启触发回填 |
| **S6 超大工具结果** | 存在数百 KB~数 MB 的工具结果 | 0.3.14 起新输出已截断；历史存量用于评估上下文压力，无强制清理必要 |

### 处置命令（按优先级）

**① 清理孤儿 / 超期任务**（先备份 DB！）

```sql
-- 备份（在 shell 里先复制 tianyan.db* 到一个安全位置）
-- 然后（需可写连接，建议停服务后执行）：
DELETE FROM background_tasks
WHERE completed_at IS NOT NULL
  AND completed_at < (CAST(strftime('%s','now') AS INTEGER)*1000 - 3*86400*1000);

-- SQLite 的 DELETE 不支持表别名（无 `DELETE FROM t AS b` 语法），用相关子查询
DELETE FROM background_tasks
WHERE NOT EXISTS (
  SELECT 1 FROM session_meta s
  WHERE s.session_id = background_tasks.parent_session_id
);
```

**② 重建 FTS 索引**

```sql
-- 需可写连接，建议停服务后执行
DELETE FROM session_messages_fts;
INSERT INTO session_messages_fts (rowid, text)
SELECT m.id, m.text
FROM session_messages m
JOIN session_meta s ON s.session_id = m.session_id
WHERE s.parent_session_id IS NULL AND m.text <> '';
```

**③ 重建向量索引**（派生数据，最安全的“清空重来”）

```powershell
# 停服务后删除向量目录，重启应用 → 启动期回填重嵌入
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\tianyan\lancedb"
```

**④ 回收空间（VACUUM）**

```sql
-- 只在停服务后执行（独占写锁，耗时与库大小成正比）
VACUUM;
```

> ⚠️ 任何写操作（①②④）都应在**停止天演服务**后执行；服务运行期 WAL 允许并发读，但写/`VACUUM`
> 会与服务抢锁。执行前务必备份 `tianyan.db`（连同 `-wal`/`-shm`）。

---

## 附：核实来源

- `core/src/db/sqlite_db.rs`（`SCHEMA_SQL`，约第 159–301 行：全部 10 张实表 + FTS5）
- `core/src/session/store.rs`（`append_message_inner`、FTS 写入条件、`MAX_TEXT_CHARS=4000`、
  `MAX_TOOL_TEXT_CHARS=500`、`extract_parts`）
- `core/src/session/search.rs`（FTS 查询、`sanitize_fts_query`：查询词 < 3 字符返回空）
- `core/src/agent/background.rs`（TTL 逐出 3 天、`background_tasks` 读写）
- `core/src/vfs/vector/lancedb/mod.rs`（`{data_dir}\lancedb`、schema、维度自愈重建）
- `core/src/config/storage.rs`（`data_dir` 默认、`vector.collection_name`）
- `.scratch/probe_db.py`、`.scratch/probe_db6.py`（体量/工具结果探查脚本，供参考）
