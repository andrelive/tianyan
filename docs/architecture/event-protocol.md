# 事件推送与订阅协议

> 天演前端与服务端之间"实时事件"的权威说明：统一事件通道、事件契约、订阅与快照恢复、
> 前端归约单点、可靠性分层、防死锁不变量。
> **依据**：ADR-013 / 028 / 029 / 030 / 031 / 032；代码核实基线 HEAD `afba701`。
> **最后更新**：2026-09-12（以源码为准，凡与 ADR 旧文冲突处见 §8）。

---

## 1. 两个"事件"子系统（先分清）

仓库里有两套互不相关的"事件"，阅读文档与代码时极易混淆：

| 子系统 | 位置 | 语义 | 协议 |
|--------|------|------|------|
| **A. T1 事件驱动（触发源）** | `core/src/events/`（`bus.rs`/`watcher.rs`/`rules.rs`/`types.rs`）、`server/src/api/events/` | 文件监听 / 外部 webhook → 事件总线 → 规则匹配 → 触发任务 / 唤醒会话（ADR-013 通路） | 进程内 `EventBus`（`mpsc::unbounded_channel`，每订阅者一条，`bus.rs:33`）+ HTTP `POST /api/v1/events` webhook（`handlers.rs:35`） |
| **B. 统一事件推送（本文主题）** | `core/src/agent/stream_forward.rs`、`server/src/event_push.rs`、`server/src/api/tasks/handlers.rs`、`server/src/api/chat/services.rs` | Agent 循环 → 事件 JSON → 统一通道 → SSE 下发前端（ADR-028/031/032） | `GET /api/v1/events`（常驻 SSE）+ `POST /api/v1/events/subscribe`（快照触发） |

> ⚠️ 任务材料里指向的 `server/src/api/events/*` 实为 **A（webhook）**；**B（SSE 推送）**的实际实现
> 在 `server/src/event_push.rs` 与 `server/src/api/tasks/handlers.rs`——`tasks/routes.rs` 同时挂了
> `/events` 与 `/events/subscribe`（见 §5 端点表）。本文只讲 B。

---

## 2. 通道模型（ADR-028/032）

### 2.1 心智模型：SSE 是"水管"，Agent 循环是"抽水机"

- **SSE 连接（水管）**：应用级常驻、与请求/循环生命周期**解耦**。前端启动即建一条
  `EventSource`（`GET /api/v1/events`），进程存活期间不关闭；只有"有新事件"时才流动
  （`gui-vite/src/hooks/use-unified-events.ts:162` `startUnifiedEvents`）。
- **Agent 循环（抽水机）**：**请求驱动**。`POST /api/v1/chat/stream` 收敛为"开关"——
  校验 + 启动循环 + 立即返回 `{status:"started"}`（不再承载 SSE 响应流），循环输出全部
  经常驻流下发（`server/src/api/chat/handlers.rs:42`、`routes.rs:12`）。
  取消是独立开关：`POST /api/v1/chat/streams/{session_id}/cancel`（显式置位 cancel 标志）。

### 2.2 统一事件通道（实现）

- 通道：`tokio::sync::broadcast::channel::<String>(256)`（`server/src/state.rs:373`，
  字段 `AppState.task_event_tx`，`state.rs:268`）。
  > 📌 ADR-028 的原决策是"有界 `mpsc` + `try_send`（单消费者）"，**实现已演化为 broadcast(256)**
  > （详见 §8）。所有推送路径共用这一个发送端；SSE 端点 `subscribe()` 得到接收端。
- **落库即推送（收窄为边界消息）**：ADR-031 后主/子会话**消息不再广播**。只有
  **边界消息**（`role == System` 或 `compression_marker == true` 的压缩摘要）在
  `BroadcastingSessionManager::add_structured_message` 落库后补发 `chat_stream`（`chunk_type=message`）
  事件（`server/src/event_push.rs:78-81`、`push_boundary_event` `event_push.rs:121`）。
- **事件契约**：统一为 `serde_json::Value` → `String` 下发；每条事件带 `type` 区分，
  会话类事件带 `session_id` 路由，任务类事件带 `task_id`。

### 2.3 三处接线 → 统一转发器（ADR-032）

一轮流式输出共有三条路径（用户轮 / 唤醒轮 / 子代理），全部改用 core 的唯一转发设施
`core/src/agent/stream_forward.rs`：

| 路径 | 映射器 | 送达目标 | 通道容量 |
|------|--------|----------|---------|
| 用户轮 | `map_chunk_to_event`（`server/src/api/chat/services.rs:201`） | `BroadcastJsonDeliver` → `task_event_tx` | `STREAM_FORWARD_BUFFER = 100` |
| 唤醒轮 | `map_chunk_to_event`（stream_id=`"wake"`，context_window=0；`server/src/agent_builder.rs:173`） | `BroadcastJsonDeliver` → `task_event_tx` | 100 |
| 子代理 | `chunk_to_stream_json`（`core/src/agent/tool_registry/agent_ops.rs:49`，精简版） | `TaskSinkDeliver` → `TaskEventSink`（server 侧 `TaskEventBroadcaster`，`agent_builder.rs:31`） | 100 |

- **字段注入单点**：`type=chat_stream` + `session_id` 由 `inject_stream_event_fields`
  统一注入（`stream_forward.rs:107`，在转发器消费循环内调用）。子代理路径 `session_id=task_id`
  （`agent_ops.rs:509` `spawn_stream_forwarder(task_id.to_string(), ...)`），因此前端按
  `session_id` 就能把子代理事件路由到任务面板。
- **映射器/送达目标是有意保留的 seam**：用户轮与唤醒轮共用 server 展示类型 `ChatMessage`
  的转换（无法下沉 core），子代理用 core 侧精简映射（跳过 `Message`/`Error`）。

### 2.4 防死锁不变量（ADR-032，历史 bug `1151680`）

> **通道必须先于轮启动创建并立即开始消费。**

`spawn_stream_forwarder(...)`（`stream_forward.rs:123`）在**同一函数内原子地**建通道 +
`tokio::spawn` 消费任务，返回 `(StreamEventSender, JoinHandle<()>)`。调用方顺序恒为：
拿到 sender → 交给轮。反向顺序（先跑轮、后消费）在新结构下**无法表达**——输出不会再积压到
轮结束（旧唤醒轮接线正是此形态：先 `await process_wake`，再消费，容量 100 一满即死锁）。

- 轮结束后 `await` 返回的句柄 = 等待流排空（`rx` 关闭 ⟺ 轮内所有 sender 已释放）。
- `process_message_stream(..., sender)` 改由**调用方持有通道所有权**（`services.rs:85-137`
  先建转发器再调 agent，`agent_builder.rs` 的 `process_message_stream` 签名收 `sender`）；
  core 侧等待整轮结束，不内部 spawn。
- 无事件通道的路径走 `spawn_null_forwarder`（`stream_forward.rs:153`，`NullDeliver`）：
  消费端存活防发送端填满缓冲阻塞，事件静默丢弃。

---

## 3. 事件类型总表

`type` 字段共 **4 类**（`gui-vite/src/hooks/use-unified-events.ts:39` `UnifiedEvent` 联合类型）。

| `type` | 触发点 | 路由字段 | 前端消费位置 | 可靠性等级 |
|--------|--------|----------|--------------|-----------|
| `chat_stream` | Agent 循环（用户轮/唤醒轮/子代理）产出 chunk；边界消息落库后补发 | `session_id`（子代理 = `task_id`） | `chat-stream.ts:64` `handleChatStreamEvent` | 增量**尽力而为**；终态经落库 + 快照权威兜底 |
| `snapshot` | `POST /api/v1/events/subscribe` 订阅/重连时 | `session_id` | `use-unified-events.ts:153` `applySnapshot`（replace 窗口） | **权威**（读自 SQLite） |
| `task_status` | 后台任务/命令状态转移（pending/running/终态） | `task_id`（全局广播，无 session_id） | `AgentTasksPanel.tsx:55`（收到即 `poll()` 拉权威列表） | **尽力而为**；权威 = SQLite 任务表 + `GET /tasks` 轮询 |
| `command_output` | 后台命令输出流逐块读取 | `task_id`（全局广播，无 session_id） | `AgentTasksPanel.tsx:58`（终端视图 append） | **尽力而为**；权威 = 命令日志文件 |

> ⚠️ `tasks/routes.rs:17` 的文件注释仍写"事件带 type 区分（message / task_status / command_output /
> 子代理消息）"——**已过时**：`message` 类型在 ADR-031 后移除，实际是 `chat_stream`。

### 3.1 `chat_stream`（流式增量）

**触发点**：`chunk_to_stream_json` / `map_chunk_to_event` 把 `AgentStreamChunk` 映射为事件 JSON；
转发器注入 `type`/`session_id` 后下发（`stream_forward.rs:123-150`）。边界消息（System 通知 /
压缩摘要）由 `push_boundary_event` 复用同一映射生成 `chunk_type=message` 事件（`event_push.rs:121`）。

**字段清单**（server 侧 `ChatStreamEvent`，`server/src/api/chat/types.rs:80-135`；子代理精简版见
`agent_ops.rs:49`）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | string | 恒为 `"chat_stream"`（转发器注入） |
| `session_id` | string | 会话 id（子代理 = `task_id`）——路由字段 |
| `id` | string | 本响应流 id（`chatcmpl-<short_uuid>`），同流所有 chunk 共享 |
| `chunk_type` | string | 见 §4（7 成员，小写） |
| `delta` | string | 正文增量；`thought`/`tool_call`/`observation` 时被置空 |
| `thinking` | string? | `thought` chunk 的思考增量 |
| `message` | ChatMessage? | `message` chunk 的完整结构化消息（边界） |
| `finish_reason` | string? | 完成原因（`stop`/`length`/`interrupted`/`error`） |
| `skill_calls` | SkillCallInfo[]? | 技能调用列表 |
| `tool_call` | ToolCallEvent? | 结构化工具调用（工具卡片） |
| `tool_result` | ToolResultEvent? | 结构化工具结果（耗时/成败/结果内容） |
| `usage` | StreamUsage? | 本轮 token 用量（含 `context_window`） |
| `user_message_id` | string? | ADR-031 乐观渲染临时 id（`chunk_type=user_message_id`） |
| `message_id` | string? | 用户消息落库后的真实 id |

> 子代理路径的 `chunk_to_stream_json` 只产出 `chunk_type`/`delta`/`thinking`/`finish_reason`/
> `tool_call`/`tool_result`（无 `id`/`usage`/`message`），并跳过 `Message`/`Error` chunk。

### 3.2 `snapshot`（重连即快照）

**触发点**：`POST /api/v1/events/subscribe`（`server/src/api/tasks/handlers.rs:45` `subscribe_events`）
读会话历史 + `cursor`（最新持久化 seq）后推入统一通道（`handlers.rs:66-72`）。

**字段清单**：

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | string | 恒为 `"snapshot"` |
| `session_id` | string | 会话 id——路由字段 |
| `cursor` | number | 快照时最新持久化 seq（`SessionStore::last_seq`，`core/src/session/store.rs:402`） |
| `messages` | ChatMessage[] | 完整历史（`ChatMessage::from_structured_light`） |

**DSH 模式 + replace 兜底**：前端收到快照即 `setSessionMessages(sid, messages)`（**replace** 窗口）；
**流式进行中跳过 replace**（覆盖流式占位有害；增量由实时事件继续）（`use-unified-events.ts:153`）。
不使用 `Last-Event-ID` 续传——重连即快照天然覆盖服务端重启/WebView 重建等所有断线场景。

### 3.3 `task_status`（后台任务状态转移）

**触发点**：core 在两处管理器 emit（经 `TaskEventSink`，server 侧 `TaskEventBroadcaster.emit` 补
`task_id`，`agent_builder.rs:31`）：

- 委托任务 `BackgroundTaskManager`：pending（`background.rs:422`）/ running（`background.rs:454`）/
  终态（`background.rs:640`）。
- 终端命令 `CommandManager`：running（`command.rs:333`）/ 终态（`command.rs:417`）。

**字段清单**（字段随阶段不同；`kind` 区分 `command` 与委托的 `TaskKind`）：

| 阶段 | 字段 |
|------|------|
| 委托 pending | `type`, `task_id`, `kind`, `status:"pending"`, `description`, `created_at` |
| 委托 running | `type`, `task_id`, `kind`, `status:"running"` |
| 委托终态 | `type`, `task_id`, `kind`, `status`, `result`, `error`, `completed_at` |
| 命令 running | `type`, `task_id`, `kind:"command"`, `status:"running"`, `command`, `pid`, `created_at` |
| 命令终态 | `type`, `task_id`, `kind:"command"`, `status`, `exit_code`, `completed_at` |

**前端消费**：收到即 `poll()`（`AgentTasksPanel.tsx:55`）——**事件只是唤醒信号**，权威列表由
`GET /api/v1/tasks` 拉取；`usePolling` 兜底轮询。

### 3.4 `command_output`（命令输出增量）

**触发点**：`CommandManager::drain_output` 每读一块 stdout/stderr 即 emit
（`core/src/executor/command.rs:694`）。

**字段清单**：

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | string | 恒为 `"command_output"` |
| `task_id` | string | 命令任务 id（`cmd_*`）——路由字段 |
| `delta` | string | 输出增量（`String::from_utf8_lossy`） |

**前端消费**：`AgentTasksPanel.tsx:58` 累积到终端视图（本地尾部截断至 64KB，与后端
`OUTPUT_TAIL_MAX_BYTES` 对齐）。**权威 = 命令日志文件**；精确回看走
`GET /api/v1/tasks/{id}/log?offset=&limit=`（`handlers.rs:148` `get_task_log`，默认 64KB/段）。

---

## 4. `StreamChunkType` 全成员（7）

枚举定义 `core/src/agent/types.rs:78-97`，序列化为小写（`#[serde(rename_all = "snake_case")]`）；
前端联合类型 `gui-vite/src/lib/types.ts:5`。

| 成员 | 线上值 | 语义 | 载体字段 |
|------|--------|------|---------|
| `Thought` | `thought` | 思考过程 | `thinking` |
| `ToolCall` | `tool_call` | 工具调用 | `tool_call`（+ `delta` 置空） |
| `Observation` | `observation` | 观察/工具结果 | `tool_result`（+ `delta` 置空） |
| `Answer` | `answer` | 回答正文（默认值） | `delta` / `usage` |
| `Error` | `error` | 错误信息 | `delta`（+ `finish_reason=error`） |
| `Message` | `message` | 消息边界（流开始用户消息 / 流结束 assistant 消息 / 边界通知） | `message` |
| `UserMessageId` | `user_message_id` | 用户消息落库确认（ADR-031 乐观渲染 id 回显） | `user_message_id` + `message_id` |

> ⚠️ **旧文档遗留**：`system-architecture.md` / `module-relationships.md` 曾写 `chunk_type` 含
> `clarification`——**实为 `user_message_id`**（原 `clarification` 已改名/取代）。另 `usage` 并非 chunk_type，
> 而是 `Answer` chunk 上的 `usage` 字段（工具轮用量由 `send_turn_usage` 以空 delta 的 `answer` chunk 下发）。

**语义要点**：

- **assistant 纯流式（ADR-031）**：`answer`/`thought`/`tool_call`/`observation`/`usage` 增量即**终版**，
  无独立"权威版 message 广播"。丢增量的兜底 = 快照/历史（assistant 消息落库）。
- **`user_message_id` 生命周期**：请求（`ChatRequest.message.user_message_id`）→ 确认事件回显真实
  `message_id` → 弃（不持久化）。前端 `confirmUserMessageId` 把本地乐观消息替换为真实 id
  （`chat-stream.ts:80`）。
- **`message` 边界**：流开始携带用户消息、流结束携带 assistant 消息的完整结构（与历史加载同构）；
  另承载边界通知（System / 压缩摘要）。前端 `applyServerMessage` 按 id 查重追加。

---

## 5. 订阅与快照恢复（ADR-029 + 031）

### 5.1 端点

| 方法 | 路径 | Handler | 语义 |
|------|------|---------|------|
| GET | `/api/v1/events` | `tasks/handlers.rs:91` `stream_tasks` | 统一常驻 SSE 流（全局单连接） |
| POST | `/api/v1/events/subscribe` | `tasks/handlers.rs:45` `subscribe_events` | 触发快照（历史 + cursor）推入统一流 |
| GET | `/api/v1/tasks` | `list_tasks` | 后台任务权威列表（事件兜底轮询源） |
| GET | `/api/v1/tasks/{id}/log` | `get_task_log` | 命令日志分页（输出权威源） |

> 旧路径 `/tasks/stream` 已删除（ADR-028/030：并入统一流）。

### 5.2 订阅请求与响应

```jsonc
// 请求
POST /api/v1/events/subscribe
{ "session_id": "X" }
// 响应（快照走 SSE，不走响应体）
{ "status": "ok" }
```
（`SubscribeRequest`，`handlers.rs:29`）

### 5.3 订阅集合的移除（ADR-031）

ADR-029 原设计有"按连接订阅注册表"（只推订阅会话的消息）。ADR-031 移除消息广播后此语义失效：

- **服务端订阅注册表已删除**——`subscribe_events` **仅承担快照推送**，无 insert/过滤逻辑。
- **前端订阅集合保留**（`subscribedSessions`，`use-unified-events.ts:90`）：resident 语义
  （打开过保持订阅，切回零延迟），用于触发快照恢复与断线重连**重放订阅**（`onopen` 时遍历重放，
  连接断了服务端订阅状态即丢失）。
- **事件过滤不再需要**：`chat_stream` 按活跃归约器（`session_id`）路由，`task_status`/`command_output`
  全局广播。

### 5.4 resume 语义（`since` / cursor）

- **cursor**：快照带 `cursor` = `SessionStore::last_seq(session_id)`（最新持久化 seq）。
- **断点对齐（seq 跳号检测）随 message 广播移除**——重连快照独立兜底（ADR-031）：
  不再有"前端维护 lastSeq + 跳号补偿"机制。
- **replace 兜底**：快照到达即 replace 窗口（流式进行中跳过）；`reloadSession`（强制刷新）
  保留 fetch 作为 replace 语义（`use-session-history.ts:37`）。

---

## 6. 前端归约单点

**入口**：`gui-vite/src/hooks/use-unified-events.ts`（应用级常驻单例 `EventSource`）。
文件头注释 1–31 行即设计说明（应用级常驻 / 断线自动重连 + onopen 重放订阅 / 可靠性分层 / DSH
模式不使用 Last-Event-ID）。

**归一化分发**（`startUnifiedEvents`，`use-unified-events.ts:162`）：

1. `snapshot` → `applySnapshot`（replace 窗口，流式跳过）；
2. `chat_stream` → `handleChatStreamEvent`（**纯函数**，无状态/无实例/无注册表）；
3. 其余 `type`（`task_status`/`command_output`）→ 遍历 `unifiedEventsListeners` 回调，
   由组件自行消费。

**归约单点**：`gui-vite/src/lib/chat-stream.ts:64` `handleChatStreamEvent`——每个 `chat_stream`
事件翻译为 store 动作（会话归属、轮次边界、截断/中断语义、usage 归位、工具结果挂卡）。

**按 sessionId 管理活跃流**：store 用**字典**（不是每流一个 reducer 实例）——
`sessionMessages: Record<string, ChatMessage[]>` + `streamStatus: Record<string, StreamStatus>`
（`gui-vite/src/lib/store/chat-slice.ts:48,100`）。事件自带 `session_id`，直接路由到对应字典键
（主会话 / 子代理 `task_id` 同构）。**组件只负责启停归约器/消费事件**：`useUnifiedEvents(handler)`
注册回调，卸载只移除回调、**不关闭连接**（应用级常驻）。

> 📌 早期设计中每个 SSE 响应一条独立连接 + per-stream 归约器实例 + 活跃流注册表（Map）——
> ADR-028/031 收敛为统一通道后**已删除注册表**，"按 sessionId 用 Map 管理活跃流"现指 store 字典
> （见 §8）。

---

## 7. 可靠性分层

| 事件/数据 | 等级 | 丢了会怎样 | 如何恢复 |
|-----------|------|-----------|---------|
| `chat_stream` 增量（thought/answer/…） | 尽力而为 | 打字机少几段增量 | assistant 消息**已落库**；快照/历史 replace 恢复完整消息 |
| `chat_stream` `message` 边界 | 尽力而为 | 本地消息 id 暂时不同步 | 同上（落库 + 快照兜底） |
| `chat_stream` `user_message_id` 确认 | 尽力而为 | 乐观消息保持占位 id | 不影响正文；快照按真实 id 收敛 |
| `snapshot` | **权威** | 历史加载失败（重试/重连） | 读自 SQLite `session_messages`；重连 `onopen` 重放订阅 |
| `task_status` | 尽力而为 | 面板状态暂不更新 | 权威 = SQLite `background_tasks`；`GET /tasks` 轮询（`usePolling`）兜底 |
| `command_output` | 尽力而为 | 终端视图缺段 | 权威 = 命令日志文件；`GET /tasks/{id}/log` 分页回看 |
| 命令完整输出 | **权威（文件）** | — | 落日志文件（`command.rs` 写文件 + 内存 tail 仅摘要 32KB） |
| 后台任务状态 | **权威（SQLite）** | — | `background_tasks` 表 `persist_upsert`；重启 `ensure_reloaded` 兜底（Running/Pending → Failed） |

**通道层易失细节**：`broadcast(256)` 有界——消费者慢时 `RecvError::Lagged` 被**静默 `continue`**
丢弃（`tasks/handlers.rs:111`）；无订阅者时 `send` 直接丢弃（broadcast 语义）。事件通道只承担
"在线实时通知"，数据完整性靠权威源（DB / 日志文件）。

**命令/任务通知文本**：后台命令/任务完成通知是**注入父会话的 System 消息**（只落库，供 LLM 上下文），
携带 join 信号 `remaining`；全部完成/失败时经 `process_wake` 触发唤醒轮，唤醒轮输出经 `chat_stream`
推送（`build_command_notification_text`，`background.rs:1035`）。

---

## 8. 与 ADR / 旧文的描述不一致之处（核实结果）

| # | 材料所述 | 代码实际 | 位置 |
|---|---------|---------|------|
| 1 | 统一通道是"有界 mpsc + try_send（单消费者）"（ADR-028 §2） | 实为 `broadcast::channel::<String>(256)`，可多订阅者，`Lagged` 静默丢 | `state.rs:373`、`tasks/handlers.rs:111` |
| 2 | 落库即推送 message 事件（ADR-028 §2） | 主/子会话消息**不再广播**；仅 System / 压缩点边界消息补发 `chat_stream(message)` | `event_push.rs:78-81` |
| 3 | 事件 `type` 含 `message`（ADR-028 / `tasks/routes.rs:17` 注释） | 已移除；实际 4 类 = `chat_stream`/`snapshot`/`task_status`/`command_output` | `use-unified-events.ts:39` |
| 4 | `chunk_type` 含 `clarification`（旧 `system-architecture.md` / `module-relationships.md`） | 实为 `user_message_id`（ADR-031 改名/取代） | `types.rs:78-97` |
| 5 | `command_output` "每 100ms 合并一批（时间窗节流）"（ADR-028 §4） | `drain_output` 每读一块即 emit，**无节流** | `command.rs:692-702` |
| 6 | 订阅注册表按连接过滤消息（ADR-029 §2/§5） | 已删除；端点仅推快照，无过滤 | `handlers.rs:45`、ADR-029 §修订 |
| 7 | 断点对齐（seq 跳号检测）兜底（ADR-028 §2） | 随 message 广播移除；重连快照独立兜底 | ADR-031、`use-unified-events.ts` |
| 8 | 任务材料称 SSE 实现位于 `server/src/api/events/*` | 该目录是 **T1 webhook（`POST /api/v1/events`）**；SSE 推送实现在 `server/src/event_push.rs` + `server/src/api/tasks/handlers.rs` | §1 |
| 9 | 任务材料称前端归约单点在 `gui-vite/src/lib/use-unified-events.ts` | 实为 `gui-vite/src/hooks/use-unified-events.ts`（`lib/` 下无此文件） | 路径 |
| 10 | 任务材料称"按 sessionId 用 Map 管理活跃流" | 实为 store 字典 `Record<string, …>`，reducer 为无状态纯函数 | `chat-slice.ts:48,100`、`chat-stream.ts:64` |

> 其余（SSE 水管/循环抽水机、统一转发器三处收敛、字段注入单点、防死锁不变量、快照 replace
> 语义、DSH 模式不用 Last-Event-ID、子代理 `session_id=task_id` 路由）**与代码一致**。

---

## 9. 参考文献

- ADR-013：统一消息通知与唤醒原语（唤醒语义 / 任务持久化）
- ADR-028：会话消息统一事件推送与任务实时同步（通道模型 / 常驻 SSE / 日志权威）
- ADR-029：事件订阅与快照恢复（`/events/subscribe` / resident / 快照帧 / 修订：订阅集合移除）
- ADR-030：统一 Agent 循环框架（子代理 `task_id` = `session_id` 事件目标）
- ADR-031：乐观渲染 + `user_message_id` 确认 + 统一流式（消息广播移除 / assistant 纯流式）
- ADR-032：流式事件转发统一（三处接线收敛单点 / 防死锁不变量）
- 代码：`core/src/agent/stream_forward.rs`、`core/src/agent/types.rs`、
  `core/src/agent/background.rs`、`core/src/executor/command.rs`、`server/src/event_push.rs`、
  `server/src/api/tasks/{handlers,routes}.rs`、`server/src/api/chat/{handlers,services,types}.rs`、
  `gui-vite/src/hooks/use-unified-events.ts`、`gui-vite/src/lib/chat-stream.ts`、
  `gui-vite/src/components/chat/AgentTasksPanel.tsx`
