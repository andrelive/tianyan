# 任务运行时模型（Task Runtime）

> 覆盖：后台任务的**运行时行为**——三类任务、并发与排队、生命周期、唤醒语义、
> 取消与回退、存储边界、面板数据流。
> 事件字段/协议见 [event-protocol.md](event-protocol.md)；上下文与压缩见
> [context-pipeline.md](context-pipeline.md)。
> 权威决策：[ADR-013](../architecture/decisions/013-unified-message-notification-wake.md)、
> [ADR-026](../architecture/decisions/026-background-tasks-unified-panel.md)、
> [ADR-027](../architecture/decisions/027-session-timeline-chain.md)、
> [ADR-030](../architecture/decisions/030-unified-agent-loop.md)。

---

## 1. 三类任务与 ID 约定

| 任务类 | 触发 | ID 前缀 | 运行时载体 | 实现入口 |
|--------|------|---------|-----------|---------|
| **委托**（子智能体） | `delegate_to_agent` 工具 | `bt_` | 独立 tokio 任务 + 独立**会话**（`task_id` 即 `session_id`） | `core/src/agent/background.rs`、`core/src/agent/tool_registry/agent_ops.rs` |
| **终端命令**（后台命令） | `execute_command(background: true)` | `cmd_` | 独立进程 + watcher 任务 | `core/src/executor/command.rs` |
| **定时任务** | 调度器扫描（间隔制，ADR-024） | 任务名 | 调度器循环内串行执行 | `core/src/scheduler/` |

关键建模（ADR-026/030）：**子智能体不是特殊机制，而是"带父会话引用的会话"**——
消息落库、事件推送、前端渲染三层完全复用主会话路径，只在两处特化：
① 消息落库**不索引 FTS**（见 §6）；② 事件按 `session_id = task_id` 路由到任务面板。

---

## 2. 并发与排队

| 层 | 常量 | 值 | 语义 |
|----|------|----|------|
| 委托（活跃） | `DEFAULT_MAX_BACKGROUND_TASKS` | 20 | 同时运行的子智能体上限 |
| 委托（排队） | `DEFAULT_MAX_BACKGROUND_QUEUE` | 40 | 超出活跃上限后的排队容量 |
| 终端命令 | `DEFAULT_MAX_CONCURRENT_COMMANDS` | 8 | 同时运行的后台命令上限 |

- 位置：`core/src/agent/background.rs:49-52`（常量）、`:277-291`（双信号量装配）、
  `core/src/executor/command.rs:29`（命令上限）。
- **排队而非拒绝**：超限时任务等待许可（`acquire_owned().await`），
  许可随任务结束自动释放。
- 命令侧另有注册表保留上限 `MAX_RETAINED_TASKS = 100`：
  超出时逐出**最旧的终态**任务（`evict_if_needed`），运行中的任务永不被逐出。

---

## 3. 生命周期

```
pending ──(许可)──► running ──(进程退出 / 循环结束)──► completed | failed | cancelled
                       │                                     │
                       └──────── 落库（SQL 权威）─────────────┘
                                                             ▼
                                        完成通知（注入父会话）+ 条件唤醒
                                                             ▼
                                   TTL 惰性逐出（终态且超 3 天，见下）
```

- **SQL 权威**：任务状态持久化在 `background_tasks` 表（经 `db::Database` 单连接），
  前端列表与任务面板的最终真相以表为准；`task_status` 事件只是"尽力而为"的实时通道。
- **TTL 逐出**：`evict_expired` 删除**终态且超过 3 天**的任务
  （`now_ms() - 3*24*3600*1000`，`core/src/agent/background.rs:860-872`）——
  **惰性触发**（在 snapshot 类调用时执行），不是独立定时器。
- **重启语义**：进程重启后重新加载任务表；**中断（仍为 running）的任务被标记为
  Failed**（回归测试 `test_persistence_reload_interrupted_becomes_failed`）——
  不假装还在运行，避免 `remaining` 计数永久大于 0。

---

## 4. 唤醒语义（ADR-013）

父会话在任务终态时被"唤醒"（继续一轮 LLM 对话），条件是：

```
should_wake = allComplete || failure
```

- `allComplete`：父会话中**仍运行的任务数 `remaining` 归零**；
- `failure`：本次结束的任务状态为 Failed（失败必须让主 agent 知道）；
- 部分完成（remaining > 0 且本次成功）**保持静默**，避免多次无效唤醒。

`remaining` 的来源是会话级计数（`session_counts`，命令侧）；
命令侧另有**就绪通知**通道：`on_command_ready`（端口监听/日志关键词命中）
与就绪超时说明——就绪是重要事件（主 agent 可据此开始后续工作）。

唤醒轮本身也走统一 Agent 循环（`process_wake`），其流式事件与用户轮同构
（见 event-protocol.md §2.3）。

---

## 5. 取消与回退

| 动作 | 入口 | 语义 |
|------|------|------|
| 取消任务 | `task_cancel` 工具 / `POST /tasks/{id}/cancel` | 终态任务幂等空操作；运行中则置 `cancelled` 并杀进程树（命令侧 `kill_process_tree`），写入退出标记 `--- terminated (killed) ---` |
| 主循环停止 | coordinator 注入 `delegation_cancel`（`AtomicBool`） | 子代理在轮顶与 chunk 循环内检查取消标志，及时中断 |
| 用户回退（会话级） | 前端回退锚点（用户输入） | 按 ADR-027：**锚点之后全部截断**（含该锚点之后发起的任务）——回退是会话时序操作，不是任务操作 |

取消的任务**不做唤醒**（`cancelled` 不是 `failed`）——用户主动中止无需再打扰。

---

## 6. 存储边界（子会话）

- **消息落库**：子会话消息写 `session_messages`（含 `content_parts`），
  但走 `add_structured_message_no_fts` → `SessionStore::append_message_no_fts`
  （`core/src/session/manager.rs:44-45,165-171`、`store.rs:114`；
  AgentLoop 侧由 `TurnPolicy::persist_no_fts` 控制，`core/src/agent/loop.rs:56,1150`）。
- **为什么**：子代理的工具输出体量大、生命周期短，索引进 FTS 会污染
  `session_recall` 的召回质量（用户搜自己的历史，不该被子代理中间输出淹没）。
- **影响**：`session_recall` **搜不到子会话消息**；要看子代理过程用任务面板
  （展开任务 → 读 store 中的子会话时间线）或按 `task_id` 直接读会话。
- **级联**：父会话删除应级联删除子会话与任务；出现"孤儿任务"
  （`parent_session_id` 无对应会话）说明级联未跑完，可用巡检 SQL 查出
  （见 [../operations/data-health-check.md](../operations/data-health-check.md) §S3）。

---

## 7. 面板数据流

```
SQLite（background_tasks，权威）
        │  ① 快照/列表
        ▼
GET /api/v1/tasks  ──────────────►  前端任务面板（活跃在上 / 完成沉底 / 可展开过程与输出）
        ▲
        │  ② 实时增量（尽力而为）
task_status 事件（统一事件流，按 session_id 路由）
        │  ③ 输出增量（100ms 时间窗合并，命令侧）
        ▼
command_output 事件
```

- 归属过滤：面板按任务的 `parent_session_id`（父会话）过滤——当前会话只看自己的任务；
- 事件丢失不影响正确性：重连/切回会重新拉取列表（SQL 权威）与消息快照；
- 命令输出**完整性由日志文件保证**（`{data_dir}/command_logs/{id}.log`），
  事件只承载实时视图（见 event-protocol.md §7 可靠性分层）。

---

## 8. 失败模式与排查

| 现象 | 可能原因 | 排查入口 |
|------|---------|---------|
| 任务卡在 running 不结束 | 进程未退出 / watcher 未跑完 / 重启后未标记 | 重启后应自动标 Failed；否则查 `background_tasks` 与进程是否存在（`Get-Process`） |
| 主会话长时间不"继续" | `remaining` 计数未归零（历史 bug：加载标志位提前置位导致恢复失败） | 查该会话运行中任务数；`remaining>0` 时唤醒被抑制 |
| 面板不实时 | 事件通道积压（有界 4096，超限丢弃）或 SSE 断连 | 日志看 `事件总线订阅者积压` 警告；切回会话触发快照 |
| 子代理输出看不到 | 子会话不索引 FTS（设计如此） | 任务面板展开读 store；不要用 `session_recall` 找 |
| 任务列表有孤儿/超期行 | 级联删除未跑 / TTL 逐出未触发 | 巡检 SQL（data-health-check §S3/S6） |

---

## 9. 参考

- ADR-013：统一消息通知与唤醒原语（`should_wake` / 任务持久化前置）
- ADR-026：后台任务与子智能体统一面板（并发/SQL 权威/TTL/不索引 FTS）
- ADR-027：会话时序链模型（回退 = 锚点之后全部截断，含任务）
- ADR-030：统一 Agent 循环框架（子代理与主 agent 同构；`task_id = session_id`）
- 代码：`core/src/agent/background.rs`、`core/src/agent/tool_registry/agent_ops.rs`、
  `core/src/executor/command.rs`、`core/src/session/{manager,store}.rs`、
  `server/src/api/tasks/handlers.rs`、`gui-vite/src/components/chat/AgentTasksPanel.tsx`
- 运维：[../operations/troubleshooting.md](../operations/troubleshooting.md)、
  [../operations/data-health-check.md](../operations/data-health-check.md)
