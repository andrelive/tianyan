# ADR-028: 会话消息统一事件推送与任务实时同步

日期：2026-09-04
状态：已提议

## 背景

后台任务/命令完成通知（System 消息）、唤醒轮汇总结果、定时任务产物等**服务端自发生产**的会话消息只落库（`SessionStore.append_message`），没有任何推送通道。前端 store 仅在两种场景下能看到消息：**流式期间**（POST /chat/stream 的 SSE 事件归约）和**挂载/刷新时**（fetch 全量历史）。会话 idle 时前端对"库里多了新消息"一无所知——后台任务完成通知不可见（重启后才显示），唤醒轮永不启动（`hasTaskNotice` 依赖本地 store 最后一条，永远不满足）。

根因是 chat/stream 的**请求驱动**语义：`POST /chat/stream` 的 SSE 连接随请求生命周期存在（发消息 → 建立 → 跑完 → 关闭）。服务端自发消息（任务完成通知、唤醒轮输出）发生在**无请求时刻**，没有可绑定的连接。

现状还有三处机制性缺陷：
- **事件通道语义混杂**：`task_event_tx`（broadcast）目前只被 `persist_sub_messages` 使用（子代理消息增量 → `/tasks/stream` → 任务面板），任务状态转移（pending/running/终态）没有事件；主会话消息完全不在通道内。
- **事件通道易失**：`broadcast::channel(256)` 有界 + 内存态——消费者慢则 Lag 丢事件，无消费者时 send 直接丢弃，进程重启清零。事件通道只能承担"在线实时通知"，不能承担数据完整性。
- **任务生命周期无守护**：`spawn_background_delegate` 的 `tokio::spawn` 丢弃 JoinHandle——任务闭包 panic 时 `mgr.complete/fail` 不执行，任务永久卡在 Running，无任何通知（无 watchdog 可检测）。

## 问题

1. **后台通知/唤醒轮结果前端不可见**（用户观察：重启后刷新才看到），唤醒轮链路（ADR-013）整体断点。
2. **任务状态无事件**：面板靠 3s 轮询 `fetchTasks()` 拉快照，运行中状态变化（Pending→Running→终态）无实时事件。
3. **命令输出无实时视图**：展开面板只见截断尾部（output_tail），跑动过程不可见；无完整日志查看器。
4. **任务 panic 悬死**：无 JoinHandle 收尾，无异常兜底。
5. **两套 SSE 语义割裂**：chat/stream（请求驱动、一次性）与 tasks/stream（常驻），消息同步逻辑分散在唤醒轮询/任务终态轮询/挂载 fetch 三处。

## 决策

### 1. 心智模型：SSE 是水管（常驻），循环是抽水机（请求驱动）

发消息/取消/任务通知都是**开关**——通知后端启动或停止该会话的循环；SSE 连接与循环生命周期解耦，常驻连接只在"有新事件"时流动。

### 2. 统一事件通道（单连接、单消费者）

- **通道类型：有界 mpsc + try_send（非阻塞）**。服务端只有一个 SSE 连接、一个 forwarder 消费者——不需要 broadcast 的多消费者语义。**发送者永不阻塞是硬约束**：推送可能发生在 AgentLoop 路径上（落库后顺手推送），LLM 循环的节奏绝不能被前端渲染速度绑架。`try_send` 满则丢弃（Err::Full，tracing 记录 dropped 计数），可靠性不靠通道——通道是尽力而为的通知，数据库是权威，前端断点对齐（见下）从库补回。
- **落库即推送**：`PersistentSessionManager.add_structured_message`（server 装配层经 `SessionManager` trait 的推送 wrapper 包装，core 不感知，SessionStore 保持纯库）成功后，同一消息推送到统一事件通道。
- **事件契约**：`{type: "message", session_id, seq, message}`——seq 即 `SessionStore` 原子取号的持久化序号（每会话独立、单调递增），是完整性检测锚点。
- **常驻 SSE 端点**：`GET /events`——**全局单连接**，事件带 `session_id`，前端收到后按会话路由到本地 store（ChatPanel 渲染主会话，AgentTasksPanel 渲染子会话）。不做每会话一条连接。
- **重连即快照（DSH 模式）**：断线重连 = 重新打开 = 重新 fetch 快照（`replace` 全量）→ 继续收实时。**不使用 Last-Event-ID 续传**——重连即快照天然覆盖所有断线场景（服务端重启、WebView 重建、任何原因），少一个机制。
- **断点对齐兜底（跳号补偿）**：前端维护 lastSeq（最后收到的连续 seq），收到跳号（seq > lastSeq + 1，通道 Lag 丢事件）时，**从后往前找最后一个断点**，以断点前一个 seq 为对齐点，调 `GET /sessions/{id}/messages?after_seq=` 拉取到对齐点，**replace 替换**（丢弃本地断点前数据，直接换成拉取结果），再与断点后的连续片段接上。例：收到 1,2,3,5,6,8,10,11（缺 4,7,9）→ 最后一个断点在 8→10 → 拉 1-9 → 替换本地 1-9 → 与 10,11 接上 → 1-11 完整。**不需要精确追踪每个 gap，一次替换覆盖全部缺失**（对齐 DSH 客户端 `replace` 语义）。
- **Tauri 场景**：用户不会刷新页面，但服务端重启/WebView 重建会断线——重连即快照（主）+ 断点对齐（补偿）两层覆盖，任何时刻前端都能与数据库收敛到一致。

### 3. POST /chat/stream 语义改造：启动循环即返回

- `POST /chat/stream` 收敛为"开关"：校验 + 启动 AgentLoop（与取消/通知共用 turn_guard 排队）+ 立即返回（不再承载 SSE 响应流）。
- 输出全部经 `GET /events` 常驻流下发。删除 `spawn_sse_forwarder` 的 draining 状态机（断线不取消的语义由"循环不依赖连接"天然满足）。
- `POST /chat/streams/{id}/cancel`（现状已有）保持：显式停止 = 置位 cancel，循环在轮次边界停止。

### 4. 任务实时事件（尽力而为类）

- `BackgroundTaskManager` / `CommandManager` 在状态转移点 emit（register → pending、mark_running → running、finish → 终态 + 结果摘要）；`drain_output` 每读一块 emit `{type: "command_output", task_id, delta}`（时间窗节流：每 100ms 合并一批）。
- 与消息类共用同一通道 + 常驻 SSE；事件带 `type` 字段区分。任务面板按 task_id 归集渲染，终端视图实时 append。
- **可靠性边界**：任务类事件是尽力而为——通道 Lag 丢块/断线丢段可接受，数据完整性由日志文件兜底（见 5）。**消息类永久可靠**（落库 + 重连快照 + 断点对齐兜底）。

### 5. 命令输出：日志文件为权威

命令输出**不落库、不进消息通道、不做 seq 续传**——它就是文件：

- **权威存储**：`drain_output` 完整写日志文件（现状已有），内存 tail 仅作快照摘要。
- **实时体验**：`command_output` 事件 → 前端终端组件（黑色背景、等宽字体、自动跟随滚动、上滚暂停跟随）。
- **精确回看**：新端点 `GET /tasks/{id}/log?offset=&limit=`（分页读取文件），前端滚动查看器按需加载。
- **智能体排查**：通知消息已携带日志路径（现状），`read_file` 工具直接读文件（需验证安全策略允许读数据目录；必要时在 allowed_directories 补齐）。
- **用户与智能体共享同一权威源**：都读文件，看到的一定一致。

### 6. 任务生命周期守护（异步 IO 收尾，非 watchdog）

`spawn_background_delegate` 改为 watcher 模式：

```rust
let handle = tokio::spawn(task_closure);          // 任务主体：只返回 Result
tokio::spawn(async move {                          // watcher：专职等句柄
    match handle.await {
        Ok(Ok(v))     => mgr.complete(id, v).await,
        Ok(Err(e))    => mgr.fail(id, e).await,
        Err(join_err) => mgr.fail(id, format!("任务进程异常终止: {join_err}")).await,
    }
});
```

`JoinHandle.await` 在任务**任何方式**结束时 resolve（正常/panic/abort）——panic 也转 fail + 通知，任务不悬死。watcher 协程挂起不占线程，主调用链 spawn 后立即返回。**无需 watchdog**：任务状态机每个转移点均有事件/通知，任何已发生的变化必有消费者获知；服务重启由 `ensure_reloaded`（Running/Pending → Failed）兜底。

### 7. 前端收敛

- 删除：唤醒轮询（3s 轮询会话消息）、任务终态感知轮询（合并服务端消息）——全部由事件驱动取代。
- 恢复 `mergeServerMessages`（按 id 去重合并服务端消息到本地 store，追加语义；历史实现见 43d9c81，被 986b87e 以错误前提 revert）——用于实时 append 与断点对齐 replace 的公共合并原语。
- 任务面板状态更新由事件驱动（`task_status` 事件），轮询 `fetchTasks()` 降级为挂载快照。
- 子代理消息自动归位：它是"id=bt_xxx 的会话"的消息，走消息类通道，按 session_id 路由——ChatPanel 渲染主会话，AgentTasksPanel 渲染子会话。`/tasks/stream`、`TaskEventBroadcaster` 特殊机制删除。

## 后果

- 后台通知/唤醒轮结果实时可见：落库 → 推送 → 前端合并 → 唤醒轮询启动 → 汇总显示，全链路事件驱动。
- 消息可靠性明确分层：消息类永久可靠（库 + 重连快照 + 断点对齐），任务实时类尽力而为（文件兜底）。
- 事件通道职责单一化：`type: message`（可靠）+ `type: command_output`/`task_status`（实时），不再承载完整数据责任。
- 任务生命周期闭环：panic 不悬死，任何异常有通知。
- 删减：`spawn_sse_forwarder` draining 状态机、唤醒轮询、任务终态感知轮询、`/tasks/stream` 特殊端点（并入统一流）、Last-Event-ID 续传机制（重连即快照取代）。
- 新增常驻 SSE 端点必须带 shutdown 感知（ADR-026 约束）：`GET /events` 需 `tokio::select!` 轮询 `shutdown_flag`，否则托盘退出卡死。

## 实施顺序

1. core：JoinHandle watcher 收尾（任务生命周期守护）+ 状态转移事件 emit
2. server：`PersistentSessionManager` 落库后推送（type=message，带 seq）+ `GET /events`（全局单连接 + shutdown 感知）+ `GET /tasks/{id}/log` 分页端点 + `GET /sessions/{id}/messages?after_seq=` 增量端点
3. server：`POST /chat/stream` 改造为启动循环即返回；删除 draining forwarder
4. 前端：恢复 mergeServerMessages → 常驻 EventSource（全局单连接，按 session_id 路由 + 断点对齐 replace 兜底）→ 删除唤醒轮询/任务终态轮询 → 终端组件 + 日志查看器
5. 验证：通知实时可见（10s 后台命令场景）、断线重连（服务重启期间消息补齐）、任务 panic 兜底（注入 panic 测试）

## 后续演进

本 ADR 的"全局广播 + 前端 fetch 快照"模型已被 [ADR-029](029-event-subscription-snapshot.md)
演进：推送范围从"所有会话"变为"订阅的会话"（按连接订阅分发），快照恢复从"前端 fetch
全量"变为"订阅时后端推快照"（快照与实时同一条流，消除合并竞态）。单连接、落库即推送、
断点对齐兜底、任务事件尽力而为等决策保持不变。

