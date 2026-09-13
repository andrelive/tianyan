# 上下文工程管线（Context Pipeline）

> 本文是「上下文工程」的权威说明：把散落在代码注释、ADR 与排查笔记里的机制固化下来。
> 每个机制陈述都标注了源码依据（`文件:行`）。变更上下文组装 / 压缩行为时，需同步更新本文与相关 ADR。
>
> 涉及 ADR：[ADR-004](decisions/004-prefix-match-context-assembly.md)（前缀匹配组装）、
> [ADR-012](decisions/012-injectable-snapshot-persistence.md)（注入快照）、
> [ADR-018](decisions/018-session-authoritative-sqlite.md)（会话权威存储）、
> [ADR-027](decisions/027-session-timeline-chain.md)（时序链：存储层完整链 / 组装层压缩点视图）、
> [ADR-031](decisions/031-optimistic-render-user-message-id.md)（user 消息 id 回显）。

---

## 1. 总览

一轮用户消息（或唤醒轮）的处理，上下文相关分四步：

| 步骤 | 动作 | 入口 |
|---|---|---|
| ① 持久化 | 用户消息落库 + 写入内存状态（`StructuredMessage` 唯一创建点） | `core/src/agent/agent_core.rs:630`（`run_agent_turn` 内 `persist_user_message`） |
| ② 组装 | `ensure_injectable` 取前缀 → `ContextAssembler::assemble` → `insert_session_hint` | `core/src/agent/agent_core.rs:448`（`assemble_context`） |
| ③ 执行 | `AgentLoop`（多轮 LLM + 工具调用，内部持久化每条消息） | `core/src/agent/loop.rs` |
| ④ 轮后压缩 | **整轮收尾检查一次**，必要时生成摘要并持久化 | `core/src/agent/agent_core.rs:691`（`do_compress` 分支 → `maybe_compress_and_persist`） |

两类数据模型贯穿始终：

- **`StructuredMessage`**：存储与内存态的唯一真相（含 `parts`、`tokens`、`compression_marker`），
  见 [ADR-002](decisions/002-structured-message.md)、[ADR-018](decisions/018-session-authoritative-sqlite.md)。
- **`InjectableContext`**：soul / project_instructions / rules / memories 的**前缀**载体，
  会话级缓存 + 固化快照（[ADR-012](decisions/012-injectable-snapshot-persistence.md)）。

组装时把二者合成为传输层 `Vec<Message>` 发给 LLM。

---

## 2. 组装顺序（前缀匹配，ADR-004）

`ContextAssembler::assemble`（`core/src/context/assembler.rs:23-70`）严格遵循
**固定前缀 + 可变后缀**，顺序**不可变更**（变更会破坏 LLM 前缀缓存命中率，见 ADR-004）：

| # | 位置 | 内容 | 来源 | 代码 |
|---|---|---|---|---|
| 1 | `messages[0]` | soul（system） | VFS `AgentPath::Soul`（首次加载后缓存） | `assembler.rs:30-33` |
| 2 | 次条 system | **project_instructions**（会话工作区 `AGENTS.md`） | `injectable.project_instructions` | `assembler.rs:35-37` |
| 3 | 合并 system | rules + memories | 向量检索（learned rules / memories） | `assembler.rs:39-59` |
| 4 | system | 会话定位提示（`insert_session_hint`） | 由 `assemble_context` 追加 | `agent_core.rs:918-930` |
| 5 | 结尾 | history（从最后一个 `compression_marker` 起） | `structured_messages[..]` | `assembler.rs:60-68` |

### 2.1 project_instructions 层（最易被文档漏掉）

> ⚠️ **多份既有文档把组装顺序写成 `soul → rules+memories → history`，漏掉了
> `project_instructions` 这一层**（见 [ADR-004](decisions/004-prefix-match-context-assembly.md)
> 与 `.scratch/audit-report-2026-09-12.md` C1）。以代码为准：
> **顺序为 soul → project_instructions → rules+memories → history**，
> 由 `assembler.rs:35-37` 与回归测试 `test_assemble_injects_project_instructions_after_soul`
> （`assembler.rs:449-465`）双重确认——project_instructions 插在 **soul 之后、rules/memories 之前**。

- 读取逻辑：`ContextPipeline::load_injectable`（`core/src/context/pipeline.rs:81-118`）
  在**会话绑定工作目录**下探测 `AGENTS.md` / `agents.md`（大小写兼容），无则空
  （`load_agents_md`，`pipeline.rs:324-346`）。
- 与 soul/rules/memories 一样属"会话内前缀"，不随轮次变化；刷新语义见 §4.5 压缩点刷新。

### 2.2 组装作用域 = 最后一个压缩点

- 组装从**最后一个 `compression_marker`** 开始（`rposition`，`assembler.rs:60-68`）；
  压缩点**之前**的原始消息不发给 LLM（但**完整保留在存储层**，见 [ADR-027](decisions/027-session-timeline-chain.md)）。
- 压缩点本身（摘要消息）**包含**在组装范围内。
- 无压缩点时从头组装（完整链全部发给 LLM）。

### 2.3 压缩点的 user 锚定

`structured_to_messages`（`assembler.rs:78-…`）对 `compression_marker` 消息**统一归一为 user 角色**
（`assembler.rs:82-85`）：

- 新数据：`compress_for_session` 落库即 `user` 角色。
- 旧数据：历史摘要曾持久化为 `system`，由转换层兼容归一为 `user`。
- **动机**：保证"压缩后的组装视图恒有 ≥1 条 user"——全 system 请求会被云 API 判为无用户输入
  而返回空输出（历史事故，见 §7.2 与 `.scratch/wake-no-user-finding.md`）。
- 普通 system 消息（如通知）不受此归一影响（测试 `test_compression_marker_role_normalized_to_user`）。

> 其余组装细节：assistant 的 `reasoning_content` **无条件保留**（天演作为 agent 总携带 tools，
> DeepSeek 思考模型要求完整回传，即使该轮未工具调用，`assembler.rs:125-130`）。

---

## 3. 注入来源一览

| 注入项 | 来源 | 刷新点 | 相关代码 |
|---|---|---|---|
| soul | VFS `AgentPath::Soul`（`cached_soul` 缓存） | 会话内首载 + 压缩点刷新 | `pipeline.rs:90-116` |
| project_instructions | 会话工作目录 `AGENTS.md`/`agents.md` | 会话内首载 + 压缩点刷新 | `pipeline.rs:118-124` / `pipeline.rs:324` |
| rules_and_experiences | 向量检索 `ContextNamespace::Agent`（`learned_rules_top_k`），失败回退目录遍历 | 压缩点刷新 | `pipeline.rs:126-130` / `pipeline.rs:246-296` |
| memories | 向量检索 `ContextNamespace::Memory`（`default_top_k`） | 压缩点刷新 | `pipeline.rs:132-134` / `pipeline.rs:298-316` |
| session_hint | 运行时拼接会话 URI | 每轮组装 | `agent_core.rs:918` |

前缀内容按会话**固化快照**（`session_meta.header_json`），重启后逐字节沿用，不重新检索
（[ADR-012](decisions/012-injectable-snapshot-persistence.md)；`agent_core.rs:428` 恢复、
`persist_injectable_snapshot`（`agent_core.rs:471`）固化）。

---

## 4. 压缩（Compaction）机制

### 4.1 阈值常量（`core/src/context/compression/mod.rs:47-56`）

| 常量 | 值 | 语义 |
|---|---|---|
| `DEFAULT_CONTEXT_WINDOW` | `128000` | 默认上下文窗口（token） |
| `DEFAULT_COMPRESSION_THRESHOLD` | `0.6` | **触发线**：占用 > 窗口 × 0.6 时压缩 |
| `CRITICAL_THRESHOLD_RATIO` | `0.8` | **仅标记**临界（用于 `CompressionStatus.is_critical`，不额外动作） |
| `DEFAULT_PRESERVE_RECENT` | `10` | 保留最近消息数（配置字段） |
| `DEFAULT_MIN_MESSAGES_TO_COMPRESS` | `6` | 最小压缩消息数（配置字段） |
| `MIN_MESSAGES_BEFORE_COMPRESSION` | `6` | **压缩点之后**至少积累的消息数，硬门槛（定义于 `core/src/agent/agent_core.rs:33`） |

> **注**：触发线在 2026-09 从 0.5 上调到 0.6（在仍有余量时保留原文，减少压缩频次与摘要损耗）。
> `CRITICAL_THRESHOLD_RATIO` 只影响 `get_status`（`mod.rs:397-…`）的状态展示，不触发动作。
>
> `preserve_recent_messages` / `min_messages_to_compress` 是 `CompressionConfig` 的配置字段，
> 但**当前默认策略 `Hybrid` 走全量摘要**（`compress_hybrid`，`mod.rs:371-392`：所有消息进一条摘要，
> 不保留原文），并未实际使用这两个字段；`Select` 策略（`compress_by_selection`）也不按它们保留。
> 消息数下限实际由 `MIN_MESSAGES_BEFORE_COMPRESSION` 在 `agent_core` 侧把关。**（待核实是否有意保留）**

### 4.2 触发时机：只在一轮收尾检查一次

- **每轮只检查一次**：`run_agent_turn` 末尾，`if options.do_compress { maybe_compress_and_persist(.., false) }`
  （`core/src/agent/agent_core.rs:691-693`）。
- 轮内的多次工具调用 / 多次 LLM 交互**不做**压缩检查——这就是"体感上要接近 80% 才压"的机制原因：
  一个长轮次内占用可能在末尾检查时已越过窗口的 60%（甚至逼近/超过窗口），但压缩要等这一轮彻底结束才发生。
- `do_compress` 由调用方按路径语义给定：`process_message` / `process_message_stream` 均 `true`
  （`core/src/agent/coordinator.rs:260`、`:317`）；澄清回答轮为 `false`。

### 4.3 触发条件与取数口径（单点）

`maybe_compress_and_persist`（`core/src/agent/agent_core.rs:527-590`）逻辑：

1. **作用域**：只看最后一个 `compression_marker` **之后**的消息（`rposition` → `start_idx`，`:531-539`）。
2. **消息数门槛**：`messages_since_marker.len() < MIN_MESSAGES_BEFORE_COMPRESSION(6)` → 直接返回（`:543-545`）。
3. **取最近实测占用**：从后向前找**第一条带 usage**（`input>0 || cache.read>0`）且**非压缩点**的消息，
   取其 `prompt_side_tokens()`（`:553-566`）。**压缩点自身不参与取数**——它的 tokens 是摘要请求
   （压缩前上下文重发）的用量，不代表会话当前占用。
4. **判定**：`compress_for_session(.., recent_input_tokens, force=false)` →
   `compress_if_needed` → `compressor.should_compress(recent_input_tokens)`（`:568`）。

**取数口径单点**：`StructuredMessage::prompt_side_tokens()`
（`core/src/common/types/structured_message.rs:239-241`）：

```rust
self.tokens.input.max(self.tokens.cache.read)
```

- `tokens.input` 是**完整输入**（**含**缓存命中部分，`tokens.cache.read` 是其子集）——故取 `input`。
- **严禁 `input + cache.read`**：两者相加会把缓存命中**重复计一遍**（判定值 ≈ 真实值 × 2，
  真实占用远低于阈值也会提前压缩，或触发"上下文预算不足"误报）。此口径在 `loop.rs`
  的实测输入恢复处也使用（`loop.rs:620-651`），**同一实现点**。

### 4.4 摘要（生成与落地）

- **策略**：默认 `Hybrid`（`CompressionStrategy::default()`，`mod.rs:42-43`）→ 全量摘要。
  摘要提示词 `DEFAULT_SUMMARY_PROMPT`（`mod.rs:125-149`）要求**固定四字段**结构：
  `## 用户意图` / `## 已执行步骤` / `## 当前状态` / `## 下一步`；
  增量合并提示词 `INCREMENTAL_SUMMARY_PROMPT`（`mod.rs:151-…`）在已有摘要上合并并保持四字段。
- **落库形态**（`compress_for_session`，`core/src/context/pipeline.rs:184-252`）：
  - `id = cmp_{epoch_ms}`、`compression_marker: true`、`finish: None`。
  - **`role: MessageRole::User`**（user 锚定，§2.3）。
  - `parts = [{ type: "text", text: "[对话摘要] 以下是对历史对话的摘要：\n…\n[摘要结束]" }]`。
  - `tokens` 取自**压缩请求的真实 usage**（`input/output/total/cache`，`:219-229`）。

### 4.5 压缩请求用量随摘要消息持久化

- 压缩**不走 AgentLoop**（没有 assistant 消息承载该次请求的 input/output/cache）。
  若摘要消息的 `tokens` 保持全零，压缩消耗会从会话统计（`sumSessionUsage` / `UsageStats`）中丢失。
- 因此 `CompressionResult.summary_usage`（`mod.rs:80-82`）随摘要消息持久化
  （`pipeline.rs:208-229`）。前端 `sumSessionUsage` 累加所有带 usage 的消息，自动计入
  （`gui-vite/src/lib/token-usage.ts:60-78`）。
- **注意**：压缩请求用量**不进 `usage_logs` 表**（该表由 AgentLoop 每轮写入，
  `core/src/agent/loop.rs:756`）——见 §6.3。

### 4.6 压缩点刷新（会话内唯一免费刷新点）

压缩成功后（自动 / 手动共用）`maybe_compress_and_persist` 调用
`invalidate_injectable_snapshot`（`agent_core.rs:484-495`）：清空内存 `injectable_context` +
置空持久化快照，下一轮 `assemble_context` 重新检索 learned rules / memories 并固化新快照。
理由：压缩已重建 system 前缀（摘要消息插入），此刻刷新零额外缓存成本——会话内唯一免费刷新点
（[ADR-012](decisions/012-injectable-snapshot-persistence.md)）。

### 4.7 失败降级

`compress_if_needed` 出错时仅 `warn`，本次会话**以降级（不压缩）方式继续**
（`pipeline.rs:197-202`）——压缩不阻断主流程。

---

### 4.8 工具表变化触发的主动压缩（ADR-030 后续演进）

除阈值与手动两条路径外，还有一条**事件驱动**的压缩入口：**真用户轮**在请求前
比对"会话工具表指纹"（`ToolRegistry::toolset_fingerprint`，确定性哈希）——
工具集发生变化（MCP 增删 / 配置热重载）时**主动压缩一次**（`force=true`）。

- 目的：把"因工具变化导致的提示词前缀缓存失效"与一次压缩合并到同一轮
  （变化立即可见 + 掉缓存值得）；仅对用户输入轮付一次指纹成本（唤醒轮不检查）。
- 首轮（新会话/重启后）只记录基线、不触发压缩。
- 消息数不足时由压缩门槛（`MIN_MESSAGES_BEFORE_COMPRESSION`）自然拒绝——
  此时工具表仍已刷新（请求侧每轮现取），只是不做摘要。

---

## 5. 手动压缩 API

| 项 | 值 |
|---|---|
| 路由 | `POST /sessions/{id}/compress`（`server/src/api/sessions/routes.rs:24`） |
| Handler | `compress_session`（`server/src/api/sessions/handlers.rs:195-215`） |
| 响应 | `CompressSessionResponse { compressed: bool, message: Option<ChatMessage> }`（`server/src/api/sessions/types.rs:9`） |
| 与轮互斥 | 先取 `turn_guard`（`core/src/agent/coordinator.rs:339-352`） |

调用链：`handler` → `agent.compress_session(&session_id)`（`coordinator.rs:339`）
→ `maybe_compress_and_persist(&state, session_id, **force=true**)`。

- **`force=true` 跳过窗口阈值**（`pipeline.rs:156-158`）——用户主动点击即明确意图；
  只受 `MIN_MESSAGES_BEFORE_COMPRESSION`（6 条）下限保护。
- 自动压缩保留窗口阈值（防频繁压缩）。
- 返回值：`Some(summary_sm)` = 实际发生压缩（摘要已持久化）；`None` = 无需压缩（消息不足或未超阈值）。
- 前端入口：`ContextRing` 详情面板「压缩会话」按钮（`gui-vite/src/components/chat/ContextRing.tsx:129-160`）。
- 向导模式（未装配 Agent）返回 `Ok(None)`（`server/src/agent_builder.rs:380-386`）。

---

## 6. 上下文构成的量化方法

### 6.1 口径关系

三个口径**同源**（都以 provider 返回的 prompt 侧 token 为基准）：

```
provider usage.prompt_tokens（完整输入，含缓存命中）
  ├─ cache_read（缓存命中部分，prompt_tokens 的子集）
  └─ prompt_tokens - cache_read = uncached（未命中部分）
```

| 口径 | 定义 | 计算 | 使用处 |
|---|---|---|---|
| `prompt_side_tokens` | 最近一次请求的 prompt 侧真实占用 | `max(tokens.input, tokens.cache.read)` | 压缩判定 / 实测输入恢复（单点） |
| `tokens.input` | 完整输入（含缓存命中） | 落库 = `usage.prompt_tokens` | `pipeline.rs:220` |
| `ContextRing` 占用百分比 | 上下文占用 = `prompt_tokens / context_window` | 前端 `pct` | `ContextRing.tsx:44-48` |
| 缓存命中率 | `cache_read / prompt_tokens` | 前端 `cachePct` | `ContextRing.tsx:49-52` |
| `UsageStat` 命中率 | `cached_input / (uncached_input + cached_input)` | SQL 聚合 | `core/src/db/usage.rs:214-221` |

**前端 `ContextRing` 的百分比口径与压缩判定同源**：分子都用 prompt 侧**完整**输入（含缓存命中），
分母都用 `context_window`。区别只是阈值不同——压缩判定比较 `prompt_side_tokens` 与
`context_window × 0.6`，圆环只展示 `prompt_tokens / context_window`。
`context_window` 由服务端从模型规格解析后注入 `StreamUsage`（失败取兜底 32K，
`server/src/api/chat/handlers.rs:66`、`server/src/api/chat/services.rs:245-251`、
`server/src/api/chat/types.rs:113-128`）。

> **重要**：缓存命中部分（`cache_read`）**算占用**（它仍占据窗口），因此**不得**用
> `input - cache_read`（"未命中"）作为上下文占用——那是成本口径，不是窗口占用口径。

### 6.2 从 `session_messages` 聚合（SQL 示例）

表结构（`core/src/db/sqlite_db.rs:259-272`）：`tokens` 派生列为**总 token**
（落库 `msg.tokens.total`，`core/src/session/store.rs:153-165`），
`content_parts` 为完整 `StructuredMessage` JSON（唯一真相，含 `tokens.input` / `tokens.cache.read` /
`compression_marker`）。逐条 prompt 侧占用需从 JSON 提取：

```sql
-- ① 某会话最近几条消息的 prompt 侧口径（与 prompt_side_tokens 同源）
SELECT seq, role,
       json_extract(content_parts, '$.tokens.input')      AS input_tokens,
       json_extract(content_parts, '$.tokens.cache.read')  AS cache_read,
       MAX(json_extract(content_parts, '$.tokens.input'),
           json_extract(content_parts, '$.tokens.cache.read')) AS prompt_side
FROM session_messages
WHERE session_id = :sid
ORDER BY seq DESC
LIMIT 5;

-- ② 按 role 聚合（本次请求上下文构成的粗略分解）
SELECT role,
       COUNT(*)   AS n,
       SUM(tokens) AS total_tokens
FROM session_messages
WHERE session_id = :sid
GROUP BY role;

-- ③ 压缩判定作用域：只看最后一个压缩点（compression_marker）之后的消息
SELECT seq, role,
       json_extract(content_parts, '$.compression_marker') AS is_marker,
       json_extract(content_parts, '$.tokens.input')       AS input_tokens,
       json_extract(content_parts, '$.tokens.cache.read')   AS cache_read
FROM session_messages
WHERE session_id = :sid
  AND seq >= (
      SELECT COALESCE(MAX(seq), -1) FROM session_messages
      WHERE session_id = :sid
        AND json_extract(content_parts, '$.compression_marker') = 1
  )
ORDER BY seq;
```

> `content_parts` 是完整 `StructuredMessage` JSON，字段路径与序列化一致
> （`$…$.tokens.input`、`$…$.tokens.cache.read`、`$…$.tokens.output`、`$…$.compression_marker`，
> `core/src/common/types/structured_message.rs:6-33`、`:140-151`）。

### 6.3 与 `usage_logs` 口径的关系

`usage_logs` 表（`core/src/db/sqlite_db.rs:289-303`）**每次走 AgentLoop 的 LLM 调用一行**：

| 列 | 含义 |
|---|---|
| `uncached_input` | `prompt_tokens - cache_read`（未命中输入） |
| `cached_input` | `cache_read`（缓存命中输入） |
| `completion_tokens` / `total_tokens` | 输出 / 总量 |

即 `uncached_input + cached_input = prompt_tokens`（完整输入，见
`core/src/observability/usage_log.rs:12-20`）。按会话聚合 prompt 侧总量：

```sql
SELECT SUM(uncached_input) + SUM(cached_input) AS prompt_side_total,
       SUM(cached_input)                       AS cache_hit,
       SUM(completion_tokens)                  AS output
FROM usage_logs
WHERE session_id = :sid;
```

**口径关系提示**：

- `usage_logs` 记的是**每次请求**（含多轮工具循环），因此同一轮内多次 LLM 调用的 `prompt_tokens`
  会**重复累加**——它反映"消耗/成本"，**不等于**某一次请求的窗口占用。
  窗口占用看**单次**：`session_messages` 里最后一条带 usage 的消息的 `prompt_side_tokens`（§4.3）。
- **压缩请求不在 `usage_logs`**（不经过 AgentLoop）——压缩消耗只体现在摘要消息的 `tokens` 里（§4.5）。

---

## 7. 故障模式与排查

### 7.1 "上下文预算不足"误报（口径重复计算 → 已修）

- **症状**：日志/错误出现 `agent_loop: 上下文预算不足（不足 1024 tokens）`，
  会话被卡死（尤其 1M 窗口、高缓存命中的会话）。
- **根因**：实测输入恢复曾用 `input + cache.read`，把缓存命中**重复计一遍**（≈真实 × 2），
  1M 窗口下高命中会话直接越过窗口 → 动态 `max_tokens` 归零 → 报"预算不足"。
- **修复**：统一改用 `prompt_side_tokens`（`input.max(cache.read)`）**单点**
  （`structured_message.rs:239`；使用点 `loop.rs:620-651`、`agent_core.rs:553-566`）。
- **预算计算入口**：`AgentLoop::prepare_request`（`core/src/agent/loop.rs:251-297`）——
  动态 `max_tokens` 由实测输入（`prompt_tokens`）与窗口计算；`max_tokens < 1024` 时不发请求，
  直接报错交由压缩链路恢复。回归测试见 `loop_tests.rs:1463-1520`。

### 7.2 全 system 请求 → 上游空输出（压缩点 user 锚定）

- **症状**：某轮无输出，会话落一条**空 assistant 消息**（`parts=[]`、`tokens=0`、
  `finish="load"`）。
- **机制**（`.scratch/wake-no-user-finding.md`）：压缩点恰好落在通知前 → 组装视图 =
  `摘要(system) + 通知(system) + 唤醒指令(system)`，**没有任何 user 消息**；云 API 对"无 user"
  请求返回空内容（`finish_reason="load"`，0 token）。
- **修复**：压缩点以 **user 角色**锚定（§2.3）——压缩后组装视图**恒有 ≥1 条 user**。
  转换层同时兼容两代数据（旧 `system` → `user`）。
  实现：`pipeline.rs:210-217`（落库 user）、`assembler.rs:82-85`（归一）；
  事件推送门控同步为 `role == System || compression_marker`
  （`server/src/event_push.rs:79`）。

### 7.3 `finish_reason: "load"` / 空 assistant 响应（上游异常）

- **症状**：上游返回既无正文也无工具调用的空响应（思考模型"想完没说话"，或网关异常）。
- **处理**：
  - `run_turns` 对空响应**同轮内重试一次**（`core/src/agent/loop.rs:696-713`）；
    重试仍空则按正常结束（空输出 = completed，对齐 DSH）。
  - 非标准 `finish_reason`（如网关 `network_error`）在 provider 层**显式上抛**为错误而非静默丢弃
    （`core/src/model/provider/chat.rs:556-567`）。
  - `finish_reason` 随 assistant 消息**持久化**到 `StructuredMessage.finish`
    （`loop.rs:992-993`；`agent_core.rs:718-744`）。
- **排查入口**：按失败会话的 `message_id` 查 `session_messages` 该条 `content_parts` 的
  `finish` 字段与 `tokens`（全 0 = 异常空响应）；对照 `usage_logs` 中该会话记录
  （若对应行全 0，说明请求未产生正常输出）。

---

## 8. 参考

**ADR**

- [ADR-004 上下文组装前缀匹配原则](decisions/004-prefix-match-context-assembly.md)
- [ADR-012 注入上下文快照持久化](decisions/012-injectable-snapshot-persistence.md)
- [ADR-018 会话权威存储迁移至 SQLite](decisions/018-session-authoritative-sqlite.md)
- [ADR-027 会话时序链模型](decisions/027-session-timeline-chain.md)
- [ADR-031 乐观渲染 + user_message_id 确认 + 统一流式](decisions/031-optimistic-render-user-message-id.md)

**源码入口**

| 关注点 | 文件:行 |
|---|---|
| 组装顺序 / 压缩点截断 / user 归一 | `core/src/context/assembler.rs:23-70`、`:60-68`、`:82-85` |
| 前缀加载（soul/project_instructions/rules/memories） | `core/src/context/pipeline.rs:81-134` |
| `AGENTS.md` 探测 | `core/src/context/pipeline.rs:324-346` |
| 压缩阈值常量 / 状态 | `core/src/context/compression/mod.rs:47-56`、`:397-…` |
| 摘要提示词（四字段） | `core/src/context/compression/mod.rs:125-…` |
| `should_compress` | `core/src/context/compression/mod.rs:201-208` |
| `compress_for_session` / 摘要落库 | `core/src/context/pipeline.rs:184-252` |
| 触发时机 / 取数 / 持久化 / 刷新 | `core/src/agent/agent_core.rs:527-590`、`:691-693`、`:484-495` |
| 组装编排 / 会话定位注入 | `core/src/agent/agent_core.rs:448-470`、`:918-930` |
| 取数口径单点 | `core/src/common/types/structured_message.rs:239-241` |
| 实测输入恢复 / 预算不足 / 空响应重试 | `core/src/agent/loop.rs:620-651`、`:251-297`、`:696-713` |
| 手动压缩（force） | `core/src/agent/coordinator.rs:339-352` |
| 手动压缩路由 / Handler | `server/src/api/sessions/routes.rs:24`、`handlers.rs:195-215` |
| 流式 usage 口径 | `server/src/api/chat/services.rs:245-251`、`types.rs:113-128` |
| 前端圆环 / usage 聚合 | `gui-vite/src/components/chat/ContextRing.tsx:44-52`、`gui-vite/src/lib/token-usage.ts:60-78` |
| 表结构（`session_messages` / `usage_logs`） | `core/src/db/sqlite_db.rs:259-303` |
| usage 落库口径 | `core/src/observability/usage_log.rs:12-20`、`core/src/db/usage.rs` |

**排查笔记（`.scratch/`，未纳入版本控制）**

- `.scratch/wake-no-user-finding.md` — 全 system 空输出事故
- `.scratch/dsh-compaction-study.md` — DSH compaction 对照研究
- `.scratch/audit-report-2026-09-12.md` — D 节本文件大纲
