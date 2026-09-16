# ADR-027: 会话时序链模型——存储层完整链，组装层压缩点视图

**日期**: 2026-09-04
**状态**: 已采纳
**影响范围**: 会话存储（`core/src/session/`）、上下文组装（`core/src/context/assembler.rs`）、
回退/重做（`server/src/api/sessions/`）、后台任务（`core/src/agent/background.rs`、
`core/src/executor/command.rs`）、Agent 协调器（`core/src/agent/coordinator.rs`）

---

## 背景

1. **会话是一条无分支的时序数据链。** 用户输入、模型输出、工具结果、压缩摘要、异步通知
   都是链上的节点，按追加顺序排列（`Vec<StructuredMessage>`，`seq` 递增）。所有操作
   （发消息、LLM 回复、压缩、回退）都是在这条链上做"追加"或"回退到某个时序点"。
2. **回退 bug（0.2.12 实测）**：`load_session_from_store` 在返回链之前做两道截断
   （compression_marker 截断 + MAX_SESSION_MESSAGES 截断），把完整链变成"工作集"。
   `delete_message` / `redo_message` 复用 `get_session` 拿到截断后的列表，truncate +
   rewrite 写回时，**压缩点前的历史被永久抹掉**（数据库证据：压缩摘要从 seq=556
   变成 seq=0，压缩点前 556 条历史 = 0 条）。
3. **"工作集"概念污染了存储操作。** 截断是为 LLM 上下文组装设计的（只把压缩点之后
   发给模型），但被放在了加载层，导致回退/重做/快照恢复三个存储操作全部误用截断后的
   列表（快照索引与完整链 seq 错位）。

---

## 决策

**存储层永远返回完整链；压缩点截断只发生在组装层视图。** 会话 = 无分支时序链，
节点 = user / assistant / tool / system 四种角色平等；回退 = 锚点（用户输入）之后
全部截断，四种节点同等处置。

### 1. 存储层：完整链（无截断）

- `load_session_from_store` 删除 compression_marker 截断与 MAX_SESSION_MESSAGES 截断；
- `get_session` 永远返回完整链（含压缩点前历史）；
- 删除 `MAX_SESSION_MESSAGES` / `KEEP_RECENT_MESSAGES` 常量（无硬上限）；
- `SessionState`（内存态）同样不裁剪——内存态与存储层一致，完整链保留
  （回退/重做/快照索引依赖完整链）。

### 2. 组装层：从最后一个压缩点开始

- `ContextAssembler::assemble` 从最后一个 `compression_marker` 开始组装
  （无压缩点从头）；压缩点（摘要消息）本身包含在组装范围内；
- 压缩点截断只发生在组装视图，不产生任何物理截断，存储操作永远基于完整链。

### 3. 任务时序锚点（方案 B）

- `BackgroundTask` / `CommandTask` 新增 `anchor_seq`（任务启动时链上消息数
  = 下一条消息的 seq）；
- delegate / command 工具注册时从会话链长获取锚点；
- 回退时锚点 > 回退点的任务属于"回退点之后"，一并取消。

### 4. 回退编排（四种状态统一）

```
用户点击回退（锚点 = 用户输入 U）
→ ① 停止 LLM 输出：置位会话 stream_cancel 标志（AgentLoop 轮次边界停止）
→ ② 取消任务：取消锚点 > U.seq 的所有任务（委托 cancel + 命令 kill），
     等待全部终态（取消通知入库，截断时一并丢弃）
→ ③ 回退文件：restore(session_id, U.seq) 恢复 U 处理前的工作区快照
     （快照索引 = 完整链 seq，与截断点严格对应）
→ ④ 链截断：完整链上 truncate 到 U 之前，rewrite 写回
```

四种状态（有无输出/有无任务）统一覆盖，每步对"不存在"的情况幂等。

---

## 理由

1. **单一真相**：存储层 = 完整链，组装层 = 压缩点视图，职责分离。回退/重做/快照
   恢复基于完整链，压缩点前的历史永远保留（可被 `vfs_read` 检索——`insert_session_hint`
   告知 LLM 的语义）。
2. **无硬上限**：MAX_SESSION_MESSAGES 是"无法向用户解释的硬设置"，删除后用户可回退
   到任意用户输入（包括 5000 条之前的）。上下文预算由压缩机制（token 阈值 + 摘要替换）
   控制，不需要消息数兜底。
3. **任务与链对齐**：时序锚点让"回退到某个时序点"的语义完整——进行中的任务也是
   "该点之后的数据"的一部分，一并取消。
4. **快照索引对齐**：加载层不再截断后，快照 `trees/{index}.json` 的 index 与完整链
   seq 严格对应，恢复不会错位。

---

## 反方观点与回应

- **"完整链加载性能差。"** 回应：`store.load` 是 `SELECT content_parts ... ORDER BY seq`
  全量读，长会话（数千条）确实有成本，但这是正确性的代价；压缩机制控制上下文预算，
  存储层完整是回退正确性的前提。若未来需要，可加"按需分页加载"（ADR-018 已预留
  `load` 的按 seq 范围查询能力），但**不允许**在加载层做物理截断。
- **"回退到压缩点之前会重新全量组装。"** 回应：用户主动回退 = 明确意图，压缩点前的
  原始消息重新进入组装是正确行为（当初压缩是因为长，回退后用户期望看到原始内容）。

---

## 影响文件清单

- `core/src/session/manager.rs` — 删除加载层截断（compression_marker + MAX_SESSION_MESSAGES）
- `core/src/session/mod.rs` — 删除 MAX_SESSION_MESSAGES / KEEP_RECENT_MESSAGES 常量
- `core/src/agent/session_state.rs` — 删除内存态 trim_conversation（完整链保留）
- `core/src/context/assembler.rs` — 组装从最后一个 compression_marker 开始
- `core/src/agent/background.rs` — BackgroundTask 新增 anchor_seq；register 签名
- `core/src/executor/command.rs` — CommandTask 新增 anchor_seq；spawn_background 签名
- `core/src/agent/tool_registry/agent_ops.rs` — delegate/command 注册时获取锚点
- `core/src/agent/coordinator.rs` — AgentCoordinator::rollback_session（取消锚点后任务）
- `server/src/api/sessions/handlers.rs` — 回退编排：置位 cancel → rollback_session → delete_message
- `docs/architecture/decisions/027-*.md`（本文档）、`018-*.md`（截断描述修订）

---

## 回滚

回滚 = git revert 本 ADR 的代码变更。存储层恢复截断会重新引入回退丢历史 bug，
不建议回滚；若需性能优化，走"按需分页加载"而非物理截断。

---

## 修订（2026-09-16）：快照键从位置改为消息 ID

原文理由 4「快照 `trees/{index}.json` 的 index 与完整链 seq 严格对应」描述的是
**位置键**。ADR-035 实施中发现两个问题后改键：

1. **位置不稳定**：`SessionStore::rewrite`（删消息 / 回退 / 编辑）用 `enumerate`
   重新编号 seq——被保留消息的位置整体前移，「index 与 seq 严格对应」只在未发生
   截断的会话里成立；
2. **内存态一旦段化即错位**：捕获端当时用 `structured_messages.len()` 表达位置，
   该值仅在「内存态 == 完整链」时才等于位置（隐式不变式）。

改为**消息 ID**（`trees/{msg_id}.json`）：回退按钮本就挂在具体消息上，
`message_id` 是天然稳定键，且与 `redo/` 既有策略统一（`redo.rs` 早已因
「数字索引不可靠」改用消息 ID）。捕获时机随之后移到用户消息落库之后（ID 已生成；
两步之间无工作区操作，语义不变）。详见 ADR-035 §4 澄清块与实施记录。

**本 ADR 其余决策不变**：存储层返回完整链、组装层从压缩点开始、回退 = 锚点之后
全部截断。「完整链」（截断语义的正确性前提）与「快照键」（定位符）是两件事。
