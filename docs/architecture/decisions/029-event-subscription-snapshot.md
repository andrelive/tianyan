# ADR-029: 事件订阅与快照恢复——单流按需订阅 + resident

**日期**: 2026-09-05
**状态**: 已采纳
**影响范围**: 服务端事件推送（`server/src/event_push.rs`、`server/src/api/tasks/handlers.rs`）、
前端事件接收（`gui-vite/src/hooks/use-unified-events.ts`、`use-session-history.ts`、
`gui-vite/src/lib/store/chat-slice.ts`）

---

## 背景

ADR-028 确立了统一事件推送：全局单连接 `GET /events` + 落库即广播 + 重连即快照 + 断点对齐。
实施后暴露三个问题：

1. **合并竞态**：打开会话 = fetch 历史（`useSessionHistory`）+ 广播事件（`mergeServerMessages`）
   两条独立路径，需要按 id 去重合并——用户消息插入位置、assistant 占位替换的复杂逻辑
   （`chat-slice.ts` 的 `mergeServerMessages`/`applyServerMessage`）全部源于此。快照（fetch）
   和实时事件（广播）的顺序无保证，只能靠去重兜底。
2. **全收全处理**：广播模型下所有会话的事件都到达前端（即使不显示），`sessionMessages`
   字典累积所有会话的消息。
3. **无快照恢复**：打开会话要单独 fetch，与实时事件有竞态；`useSessionHistory` 的
   `hasSessionMessages` 缓存守卫是"有缓存不 fetch"的妥协——缓存可能不完整（通道 Lag 丢块）。

## 问题

1. 合并竞态导致消息合并逻辑复杂（用户消息插入、去重、占位替换），且是历史 bug 的温床
   （b749eb0 用户消息双写冲突即源于此）。
2. 非显示会话的事件全推——浪费带宽与前端处理。
3. 打开会话 = fetch + 广播两条路径，快照与实时顺序无保证。

## 决策

### 1. 订阅端点（快照恢复的触发点）

```
POST /api/v1/events/subscribe  {session_id: "X"}
→ 后端读会话 X 历史尾部 + cursor（最新 seq）→ 快照帧推入该连接的 SSE 流
→ 响应 {status: "ok"}（快照走 SSE，不走响应体）
```

单流没有"打开流"动作（DSH 多流天然有），订阅请求是快照的显式触发点。

### 2. 按连接订阅注册表

每个 SSE 连接 handler 闭包持有自己的订阅状态（`Set<session_id>` + 每会话 cursor）——
axum 每连接一个 handler 实例，闭包局部变量即注册表，零额外成本。单连接场景
（本地单窗口）自然退化为一个集合。

### 3. 快照帧

```
{type: "snapshot", session_id: "X", cursor: 42, messages: [ChatMessage...]}
```

订阅时后端推快照（历史尾部 + cursor），前端 `setSessionMessages(sid, messages)`（replace）
并记录 `sessionSeqs[sid] = cursor`。**快照与后续实时事件同一条流，顺序由后端保证**
（订阅时记录 cursor，之后该会话事件按 `seq > cursor` 过滤推送）——合并竞态消失。

### 4. resident：打开过保持订阅

- 首次打开会话 → subscribe（快照恢复）
- 之后**保持订阅不取消**（DSH 模式：`Sessions remain resident after creation so their
  open Remote sources keep running off-screen`）——切回零延迟（窗口有累积）
- 流式会话切走保持订阅（未落库增量不能丢，fetch 拿不到）
- 会话从列表删除 → unsubscribe（或服务端在会话删除时清理）

### 5. 事件过滤

- `message` / `chat_stream`：**按订阅分发**——只推订阅了该会话的连接
- `task_status` / `command_output`：**全局广播**——任务面板不按会话订阅，不受影响

### 6. 前端订阅集合 + 重连重放

`useUnifiedEvents` 维护已订阅集合（`Set<session_id>`）；切换会话时检查未订阅则
subscribe；`onopen`（断线重连）时遍历集合重放订阅（连接断了，服务端订阅状态丢失）。

### 7. useSessionHistory 改造

挂载时的 `fetchSessionMessages` 换成 subscribe（快照即历史）——fetch 和广播两条路径
合并成一条，`mergeServerMessages` 的复杂插入逻辑大幅简化（快照 replace + 实时 append
天然顺序）。`reloadSession`（强制刷新）保留 fetch 作为 replace 语义。

### 8. 与 ADR-028 的关系（演进，非推翻）

| ADR-028 决策 | ADR-029 演进 |
|-------------|-------------|
| 全局单连接，事件带 session_id，前端按会话路由 | 保持单连接；推送范围从"所有会话"变为"订阅的会话" |
| 重连即快照（前端 fetch 全量） | 订阅时快照（后端推，快照 + 实时同一条流） |
| 落库即推送 | 保持 |
| 断点对齐兜底（跳号检测） | 保持（订阅期间丢事件仍靠它兜底，两者互补） |
| 任务事件尽力而为 | 保持（全局广播不受订阅过滤） |

## 后果

- **合并竞态消失**：快照 + 实时同一条流，顺序后端保证；`mergeServerMessages` 的
  用户消息插入/去重/占位替换逻辑可大幅简化。
- **按需接收**：非订阅会话零推送（省带宽/处理/内存）。
- **切回零延迟**：resident 窗口有累积，直接显示。
- **服务端新增**：订阅端点 + 快照生成 + 按连接过滤 + 每会话 cursor。
- **前端新增**：订阅集合管理 + 快照处理 + 重连重放。

## 实施顺序

1. server：订阅端点 + 快照生成（读会话历史尾部 + cursor）
2. server：事件推送按订阅分发（消息类），任务类保持全局广播
3. 前端：`useUnifiedEvents` 订阅集合 + 快照帧处理 + 重连重放
4. 前端：`useSessionHistory` 改造（subscribe 替代 fetch）+ `mergeServerMessages` 简化
5. 验证：切换会话快照恢复、断线重连重放订阅、流式会话 resident、任务面板不受影响

## 修订（2026-10-05）：ADR-031 后订阅集合移除，快照独立承担

[ADR-031](031-optimistic-render-user-message-id.md) 移除消息广播（`BroadcastingSessionManager`
收窄为纯委托）后，本 ADR 的"按订阅分发 message 事件"语义随之失效：

- **订阅注册表移除**：`AppState.event_subscriptions`（全局集合）与 `subscribe_events` 的
  insert 逻辑成为死代码（只写不读），已删除。
- **订阅端点保留**：`POST /events/subscribe` 仅承担**快照推送**（完整历史 + cursor →
  统一事件通道）——前端打开/重连时的权威兜底，与 ADR-031 的"快照独立兜底"一致。
- **前端订阅集合保留**：`subscribedSessions`（resident 语义）仍用于触发快照恢复与
  重连重放；`message` 事件分支与断点对齐（seq 跳号检测）随广播移除而删除。
- **事件过滤不再需要**：消息类事件（chat_stream）按活跃归约器路由（session_id），
  任务类事件（task_status/command_output）全局广播——无订阅过滤。

保留不变的决策：快照与实时同一条流（顺序后端保证）、resident 切回零延迟、
重连重放订阅、快照 replace 窗口（流式进行中跳过）。
