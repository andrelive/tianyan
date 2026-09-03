# 后台任务与子智能体统一面板设计（草案）

> 状态：**已实现（2026-09-01，对应 [ADR-026](decisions/026-background-tasks-unified-panel.md)）**。
> 本文为设计讨论稿存档；实现细节以代码与 ADR-026 为准。
> 对应需求：后台任务与子智能体委托采用同一套展示机制，右侧竖排任务面板，
> 活跃在上、完成沉底，用户有掌控感。
> 关联：ADR-013（统一消息通知与唤醒）、ADR-018（会话权威存储 SQLite）、ADR-022（会话绑定任务 UX）。

## 1. 背景与目标

现状：委托任务（delegate_to_agent）与终端命令（execute_command background）在后端已是
统一注册表（`BackgroundTaskManager`，TaskKind = Delegate/Command），但前端展示割裂：
委托卡片在聊天流内（ToolCallCard），后台任务在底部停靠条（SessionTasksPanel），
子智能体的工作过程完全不可见（sub_messages 被丢弃）。

目标：

1. **统一展示**：后台任务与子智能体委托同一套展示机制——右侧竖排任务面板，
   活跃任务在上、完成沉底，可展开看过程/结果，可取消。
2. **子智能体过程可见**：子智能体的对话流与主对话流在结构和展示上完全一致，
   区别只是多一个父会话关联字段——存储、协议、渲染三层复用。
3. **实时流式**：子智能体消息流式推送（SSE），与主对话流同协议，非轮询。
4. **数据有界**：清理策略明确，内存/磁盘/FTS 不无限膨胀。
5. **并发可配置**：委托并发与终端并发由用户配置。

## 2. 现状分析

### 2.1 委托执行路径（core/src/agent/tool_registry/agent_ops.rs）

- `execute_delegate_to_agent`：`background=true` 走 `spawn_background_delegate`（异步）；
  缺省走 `run_delegation_loop`（同步，120s 预算，超时自动提升为后台任务）。
- `run_delegation_loop` 内部维护 `sub_messages: Vec<Message>`（子智能体完整对话历史），
  但 `spawn_background_delegate` 结束时只取 submit_result 的 JSON，**sub_messages 被丢弃**。
- 子智能体是"一次性、干净上下文"（2026-08 决策：移除 durable 角色会话 continue 模式）。

### 2.2 后台任务注册表（core/src/agent/background.rs）

- `BackgroundTaskManager`：内存 HashMap 权威 + SQLite 镜像（`persist_upsert` 失败仅告警），
  启动 `ensure_reloaded` 全量加载，**无逐出**（委托任务）。
- 并发控制：`Semaphore::new(DEFAULT_MAX_BACKGROUND_TASKS=4)`，`try_acquire` **立即拒绝**（不排队）。
- `CommandManager`（终端命令）：`DEFAULT_MAX_CONCURRENT_COMMANDS=8`，**排队**模型，
  `MAX_RETAINED_TASKS=100` 逐出最旧终态任务（内存）。

### 2.3 会话存储（ADR-018）

- `session_messages`（完整消息）+ `session_meta`（会话级状态）+ `session_messages_fts`（FTS5 回忆索引）。
- `SessionStore.append_message`：单事务原子取号 + 写 FTS（text 非空时）。
- `SessionHeader`：新增字段均为 Option（serde 缺省 None），旧头部与新头部双向兼容。
- `SessionRecall.search`：FTS 查询，无会话类型过滤。

### 2.4 前端

- 主对话流：SSE（/chat/stream）→ `consumeSseStream` 解析 → `createChatStreamReducer` 归约 →
  `MessageBubble` 段渲染（ThinkingBlock / MarkdownContent / ToolCallCard）。
- `SessionTasksPanel`：底部停靠条，3s 轮询 `GET /tasks`，按 `parent_session_id` 过滤。
- 工具结果在主业务流**完整不截断**（默认收起、点击展开）。

## 3. 方案设计

### 3.1 委托只支持异步

`delegate_to_agent` 删除同步分支：所有委托一律走 `spawn_background_delegate`
（注册 → 并发许可 → spawn → 立即返回 task_id）。同步预算、超时提升、
`promoted_to_background` 逻辑（约 60 行）全部删除。

- 数据统一：所有委托都进 `background_tasks` 注册表，面板数据源单一。
- 与 ADR-013 完全对齐：委托完成 → 通知注入 → 唤醒轮汇总。
- 工具描述已引导"不要轮询，继续工作直到被通知"。
- 风险：主 agent 对简单短任务也变成"委托→等通知→汇总"两轮，需真实场景验证。

### 3.2 子智能体 = 带父会话引用的会话

子智能体消息流**直接走会话存储**，不另起 transcript 机制：

- `SessionHeader` 加字段：`parent_session_id: Option<String>`（关联主会话）、
  `kind: Option<String>`（"delegate"）、`role: Option<String>`（角色名）。
  `session_meta` 加独立列 `parent_session_id`（查询/过滤/级联直接走 SQL，不解析 JSON；
  需 schema 迁移，挂 `server/src/migration.rs`）。
- `run_delegation_loop` 每轮消息 `SessionStore.append_message(bt_xxx, msg, index_fts=false)`。
  **"一次性干净上下文"语义不变**：上下文组装仍从内存 sub_messages 构建，持久化只是记录。
- 会话 id = 任务 id（bt_xxx），任务状态机（`background_tasks`）与消息流（`session_messages`）
  通过 task_id 关联。

### 3.3 流式推送（与主对话流同协议）

- 新增聚合 SSE 端点 `GET /tasks/stream`：所有活跃委托任务的事件走一个连接，
  事件复用 `ChatStreamEvent` 结构（thought/tool_call/observation/answer/message），
  加 `task_id` 字段归集到面板对应任务。
- 面板打开时：先 `GET /sessions/{bt_xxx}/messages` 拉历史，再订阅增量——
  与主对话流"历史加载 + 流式增量"同构。
- 服务压力：SSE 事件驱动推送（有事件才发），一个聚合连接覆盖所有任务，桌面量级无压力。
- **shutdown 感知**（实现时补充）：本流是前端 EventSource 常驻订阅（会话存在期间不关闭），
  forwarder 必须轮询 `shutdown_flag`（1s 间隔，与 `/chat/stream` 的 `spawn_sse_forwarder` 同模式）——
  否则 axum 优雅关停等待所有活跃连接结束将永不完成，桌面端托盘「退出」卡死（只能杀进程）。

### 3.4 渲染复用

- 把 `MessageBubble` 的段渲染抽成共享组件（thinking → 折叠块、text → Markdown、
  tool_call → ToolCallCard），主对话流与子智能体流共用——视觉完全一致。
- 工具结果完整不截断（与主业务流一致，默认收起、点击展开）。
- 新消息到达自动滚到底部（实时滚动）。

### 3.5 右侧任务面板（会话级）

- 位置：右侧竖排面板（340px，可折叠成 40px 竖条），替代底部 `SessionTasksPanel`。
- 结构：头部（标题 + 运行中/已结束计数 + 折叠）→ 进行中区（活跃任务在上）→
  已结束区（完成沉底，可展开看过程/结果）。
- 任务卡片：状态图标（pending 灰 spinner / running 蓝 spinner / completed 绿勾 /
  failed 红叉 / cancelled 灰减号）+ 类型徽章（委托/终端）+ 角色徽章 + 描述 + 耗时 + 取消按钮。
- 委托任务展开：子智能体消息流（复用 MessageBubble 渲染，SSE 实时）。
- 终端任务展开：输出尾部（32KB 内存尾部，实时）+ "查看完整日志"（按需读 log_file）。
- 流内 ToolCallCard 与 System 通知消息**不动**（通知注入是主 LLM 汇总机制，ADR-013）。

### 3.6 清理策略

**A. 级联删除**：主会话删除时，连带删除其所有子智能体会话
（`delete_session` 级联 `DELETE ... WHERE parent_session_id = ?`，三表：messages/meta/FTS）。

**B. 上限保护**：子智能体会话总数超 300 时，删"最不活跃主会话"的子会话
（`session_meta.updated_at` 最旧的主会话），循环直到总数 ≤ 300。触发时机：惰性（创建子会话时检查）。

**C. FTS 不索引子会话**：`SessionStore.append_message` 加 `index_fts: bool` 参数，
`run_delegation_loop` 传 `false`——子会话消息不写 FTS：
- FTS 索引不膨胀（回忆检索性能问题从源头解决）；
- `session_recall` 天然搜不到子会话（FTS 里没有），查询逻辑零改动；
- 记忆提取（走 FTS/消息）天然排除子会话（子会话无"人"提供的信息）。

**D. 注册表 SQL 权威 + TTL**：`BackgroundTaskManager` 改为 SQL 权威：
- 内存只保留未终态任务（Running ≤ 并发上限 + Pending 排队），终态任务落 SQLite；
- 逐出 = `DELETE WHERE completed_at < now - 3d`（定时或惰性触发）；
- 启动 `ensure_reloaded` 简化：不再全量加载，只把遗留活跃任务标记 Failed。
- `CommandManager` 同样：活跃任务保留内存（进程句柄），终态落 SQLite。
- 行为变化点：SQL 写失败从"仅告警"改为上抛（或影响状态机），语义要明确。

### 3.7 并发配置（可配置）

| 配置项 | 默认值 | 语义 |
|---|---|---|
| `agent.max_background_concurrency` | 20 | 委托任务同时运行上限 |
| `agent.max_background_queue` | 40 | 委托任务排队上限（超出拒绝） |
| `agent.max_command_concurrency` | 16 | 终端命令同时运行上限 |

排队模型（委托）：双信号量——

```rust
run_sem:   Semaphore::new(max_background_concurrency)  // 运行许可
queue_sem: Semaphore::new(max_background_queue)        // 排队槽位
```

1. `queue_sem.try_acquire_owned()` —— 拿不到（排队已满）→ 拒绝（"排队已满"）；
2. `run_sem.acquire_owned().await` —— 阻塞排队等运行许可；
3. 运行结束释放两个许可。

同时运行 ≤ 20，排队 ≤ 40，第 61 个委托才被拒绝。排队中的任务 = Pending（"等待中"），
面板可见。`spawn_background_delegate` 顺序调整：先拿 queue 槽位 → register（Pending）→
等 run 许可 → mark_running → spawn。

终端命令保持现有排队模型，上限改为可配置（默认 16）。

## 4. 配置项定义

```toml
[agent]
# 委托任务并发上限（同时运行）
max_background_concurrency = 20
# 委托任务排队上限（超出拒绝）
max_background_queue = 40
# 终端命令并发上限（同时运行）
max_command_concurrency = 16
```

放 `AgentConfig`（core/src/config/agent.rs），serde default 函数 + validate 校验
（并发 ≥ 1，排队 ≥ 并发）。

## 5. 实现清单

| 项 | 改动 |
|---|---|
| 委托只支持异步 | `agent_ops.rs` 删同步分支/超时提升 |
| `SessionHeader`/schema | 加 `parent_session_id`/kind/role 字段 + `session_meta` 独立列（迁移） |
| `SessionStore.append_message` | 加 `index_fts` 参数 |
| `run_delegation_loop` | 每轮消息 `append_message(bt_xxx, msg, false)` |
| 聚合 SSE 端点 | `GET /tasks/stream`（事件带 task_id） |
| 渲染复用 | 抽 `MessageBubble` 段渲染为共享组件 |
| 右侧面板 | 新 `AgentTasksPanel`（替代 `SessionTasksPanel`） |
| 级联删除 | `delete_session` 级联删子会话 |
| 上限保护 | 子会话创建时惰性检查阈值 300 |
| 注册表 SQL 权威 | `BackgroundTaskManager` 内存只留活跃 + 3 天 TTL 逐出 |
| 并发配置 | `AgentConfig` 加 3 项 + 双信号量排队模型 |
| 会话列表过滤 | 查询排除 `parent_session_id IS NOT NULL` 的会话 |

## 6. 待定项 / 风险

1. **主 agent 异步化行为验证**：委托只支持异步后，验证主 agent 不会在委托后空转等待。
2. **SQL 权威的写失败语义**：从"仅告警"改为上抛，需评估对状态机的影响。
3. **子会话清理的"最不活跃"定义**：`session_meta.updated_at` 是否随消息追加刷新（现状
   `updated_at` 是创建时默认值，需确认更新时机）。
4. **面板宽度/折叠交互**：340px 是否合适，可折叠成竖条。
5. **终端命令并发 16 的实测**：桌面机器 16 个并发进程的实际压力。
