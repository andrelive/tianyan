# ADR-056: 回退改「可见水位」—— 重做零成本，`rewrite` 退役

**日期**: 2026-11-14
**状态**: ✅ 已采纳（已实施）
**影响范围**: 会话存储（`core/src/session/store.rs` schema 列 + 读 SQL + 追加/截断、
`core/src/session/search.rs` 四处检索）、工作集（`core/src/agent/working_set.rs`：
`rewind_to` / `redo_visible` / `clear_messages`，整表重写退役）、回退编排
（`core/src/agent/rollback.rs`）、快照重做（`core/src/snapshot/redo.rs` 消息副本退役）、
服务端两处清空（`api/chat/services.rs`、`scheduled_tasks/manager.rs`）

---

## 背景

回退 / 重做此前有两条「整表重写」路径（`SessionStore::rewrite`：单事务里
`DELETE` 全部消息 + 逐条 `serde_json::to_string` 重插 + FTS 全删全建）：

1. **回退的 fallback**：`truncate_to` 只能增量删尾部；落点落在压缩点之前时（段内索引
   会变负）退回 `full_chain()` + `rewrite(&chain[..index])`——1300+ 条消息的会话实测
   1186 ms 主要耗在这里。
2. **重做**：`snapshot/redo.rs` 把被回退的消息另存一份 `messages-{key}.json`
   （序列化 + 落盘），重做时读回、`full_chain()` 再 `extend` + `rewrite`——为恢复几条
   消息把整条链删光重插，连带 FTS 索引全部重建。
3. **清空会话**：`rewrite(&[])`（两处调用）。

三者共同的形状问题是：**被回退的消息从未变过，却要被「复制一份 → 删掉 → 再搬回来」**。
消息 ID、`seq`、FTS 索引、派生列（`first_request_tokens` 基线）全在原位，唯一需要改变
的只是「从哪一条开始算可见」。

## 决策

回退不再删除任何消息，只前移一个**可见水位** `session_meta.visible_from_seq`：

| 操作 | 库层动作（单事务） | 内存段 |
|------|-------------------|--------|
| 回退到 `index` | 删**上一次**的待重做段（`seq >= 旧水位`）→ 水位 = `index` → 重算基线 | 段内截断；落点在段起点之前则清空段 + `rebuild` |
| 重做 | 水位 = `NULL` → 重算基线（**一行都不搬**） | `rebuild`（段 = 新可见尾的后缀） |
| 新消息落库 | 删待重做段（`seq >= 水位`）→ 水位 = `NULL` → 取号插入 | 正常追加 |
| 清空会话 | `truncate_from(0)` 物理删除（**不并入水位**：清空必须真删） | 段清空 |

**语义边界**：`seq >= visible_from_seq` 的行 = 「最近一次回退删掉的、唯一一段待重做
数据」。重做永远恢复到**全可见**（`NULL`），不保存「上一个水位」——因为回退时会先删掉
上一层，旧层已没有回到路径（**一层撤销**，产品上重做也只有一次）。

**为什么可行**：`truncate_from` 只删 `seq >= from`、与段起点无关；`rebuild()` 自己查
`last_seq` + `last_marker_seq` 定位段，压缩点被水位遮掉时会自动退到更早的压缩点。可见区
永远是完整链的**前缀**，所以「位置即 seq」不变式在可见区内依然成立（损坏行仍可用占位
消息顶替）。

## 关键实现约束

1. **可见性过滤必须出现在所有读 `session_messages` 的 SQL 里**（`seq <
   COALESCE(visible_from_seq, i64::MAX)`）：`SessionStore` 的 10 处（`load` /
   `load_from` / `load_after` / `load_before` / `last_user_seq_before` / `last_seq` /
   `find_seq` / `list_meta` 的 count 与最后时间）+ `SessionRecall` 的 4 处（FTS /
   回忆窗口 / 会话内增量 / 跨会话增量）。
2. **`last_seq()` 返回可见尾**（不是 `MAX(seq)`）。漏改这里是**必错**而非静默：追加
   取号会从孤儿之后开始。
3. **`find_seq` 只认可见行**：水位之后的锚点「不存在」——否则能回退到已回退的位置。
4. **追加前清理待重做段必须与插入同事务**：崩在中途整体回滚 → 重做段仍完整（不能出现
   「删了一半」的重做数据）。取号 `MAX(seq)+1` 依赖这次清理，否则新消息会排到孤儿之后。
5. **回退的「删旧层 + 移水位」同事务**：这是新的数据销毁点（第二次回退会真删第一次的
   待重做段），不原子会变成「旧段已删、水位还是旧值」。
6. **派生列 `first_request_tokens` 随可见区重算**：基线行可能被遮（回退）或放回（重做），
   两个操作都在同一事务里跑 `recompute_first_request_tokens_sql()`（`ORDER BY seq
   LIMIT 1` + `json_valid` 保护，早期终止）。

## 后果

**收益（实测）**

- 跨压缩点回退（5500 条链，压缩点 5000，回退到 10）：**30 ms → < 1 ms**（内存库 +
  debug，看相对量级）。生产库上旧路径还要额外付出 15 MB 链的读盘与 FTS 重建。
- 重做从「全链读 + 整表重写 + FTS 全重建」变成**单行 UPDATE**——FTS 索引、派生列、正文
  原样恢复（行从未离开表）。重做不再依赖快照管理器/工作区：消息撤销与文件撤销解耦
  （`RollbackOutcome.redo_available` 现在恒为「消息层可撤销」）。
- 清空会话从整表重写降到一条 `DELETE`。
- `snapshot/redo.rs` 的 `messages-{key}.json` 退役（少一次整段序列化 + 落盘 + 读回）；
  连续回退不再堆多份重做树（写新树前清本会话 redo 目录里其它锚点的文件）。
- `SessionStore::rewrite` 生产零调用 → 降级为 `#[cfg(test)]` 测试构造原语；
  `SessionWorkingSet::rewrite` 删除（保留它只会诱导未来把回退写回整表替换）。

**代价（诚实记账）**

- 「表里存在不可见行」成为常态：将来任何新增读 SQL 漏掉可见性条件就是**静默**错误
  （FTS 能搜到已回退内容、列表 count 虚高）。守法是穷举读面的契约测试
  （`test_rollback_watermark_hides_tail_from_all_reads`——新增读 API 必须在此补断言）
  加两处口径注释（`SessionStore::visible_cond` / `search.rs` 模块文档）。
- 回退后既不重做也不发消息 → 那段行一直占空间（一次回退千条 ≈ 十几 MB）。**不做定时
  GC**：定时清理会毁掉重做能力，只能靠「下次追加」或「重做」两个时机回收。
- 重做数据不再跨进程独立存活——但前端只有回退响应里那句「可撤销」（gui-native 无重做
  按钮），「重启后还能重做」本来就没有入口，实际能力无损失。

## 被否决的替代方案

| 方案 | 否决理由 |
|------|---------|
| 内存重做段（回退时把被删消息放内存） | 进程重启即丢；仍是「第二份副本」，只是换了介质 |
| 独立 redo 表搬运（`INSERT..SELECT` + `DELETE`） | 重做要搬回 + 重建 FTS（rowid 变化）；派生列也要重算——水位方案下这些全免 |
| 多层撤销栈（`visible_from` + `redo_visible_from`） | 回退时删旧层已消除栈的必要性；产品语义只有「重做一次」 |
| 逻辑删除 + 定时 GC 回收孤儿 | 定时清理与「保留重做数据」直接冲突 |
| 回退继续走 `truncate_from`（物理删） | 重做必须重新插入 + 重建 FTS；这是本 ADR 要消除的成本 |

## 验收

- 契约测试（`core/src/session/store.rs`）：读面穷举 / 重做恢复（含 FTS 与基线）/
  追加清段 / 两层回退丢弃旧层 / `truncate_from` 与水位的关系；工作集侧
  `test_rewind_moves_watermark_and_keeps_rows`（回退 → 重做 → 清空三段）。
- 反向注入两次，均精确变红：① 去掉 `visible_cond` 的过滤 → 「load 只返回可见区」
  left 5 / right 3；② 追加时跳过待重做段清理 → 「新消息复用待重做段的起点」left 4 /
  right 2（seq 跳到孤儿之后）。
- 全量：core 1270 / server 222 + 25（含 e2e）/ task-builtin 39 全绿；`scripts/test.ps1 lint` 通过。
