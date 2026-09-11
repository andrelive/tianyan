# ADR-032: 流式事件转发统一（三处接线收敛单点）

**日期**: 2026-09-11
**状态**: 已采纳

## 背景

ADR-031 把三条路径都"流式化"了（用户轮 / 唤醒轮 / 子代理），但**每处各自手写了
"最后一公里"接线**——建 mpsc 通道、起消费任务、映射为事件 JSON、注入
`type=chat_stream` / `session_id` 字段、送达宿主通道：

| 路径 | 建通道/消费位置 | 映射函数 | 送达目标 | 通道容量 |
|------|----------------|----------|----------|----------|
| 用户轮 | `server/api/chat/handlers.rs` 转发协程 + `services.rs::pump_stream` | `map_chunk_to_event` | `task_event_tx`（broadcast） | 100 |
| 唤醒轮 | `server/agent_builder.rs` 注入的 `forward` 闭包 | `map_chunk_to_event`（stream_id="wake"，context_window=0） | `task_event_tx` | 100 |
| 子代理 | `core/agent/tool_registry/agent_ops.rs` | `chunk_to_stream_json`（另一套） | `TaskEventSink` | 64 |

同一件事写了三遍，且已经漂移：容量 64/100 混用、字段注入三处手写、
子代理映射跳过 Message/Error 而另两处不跳、唤醒轮 context_window 硬编码 0。

**这个结构直接导致了一个线上 bug（1151680）**：唤醒轮转发器先
`await process_wake(...)` 跑完整轮、**之后**才调用 `forward` 开始消费——
轮内每产生一个 chunk 就往无人消费的通道里 send，容量 100 一满即阻塞，
`process_wake` 永不返回 → rx 永不消费 → **死锁**（表现为"后台命令完成后
好久没有输出，退出重进才看到"）。该 bug 只可能发生在唤醒轮这一处手写接线
里——用户轮那条接线天然"先建通道再消费"，不会写错。

修复（1151680）让 `process_wake` 改收调用方传入的 `StreamEventSender`，
通道创建权归调用方——这只是把"接线由调用方负责"显式化，**三处手写接线
仍然存在**，同类错误仍可能在别处复现。

## 决策

在 core 建立**唯一**的流式转发设施 `core/src/agent/stream_forward.rs`：

```rust
/// 建通道 + 立即启动消费任务 → (sender, 消费任务句柄)
pub fn spawn_stream_forwarder(
    session_id: impl Into<String>,
    mapper: StreamEventMapper,              // (session_id, chunk) → Option<event JSON>
    deliver: Arc<dyn StreamEventDeliver>,   // 事件去向
) -> (StreamEventSender, JoinHandle<()>);
```

三处接线全部改用它，只保留两个**有意可插拔**的差异点：

- **映射器**（`StreamEventMapper`）：用户轮/唤醒轮用 server 的
  `map_chunk_to_event`（依赖 server 展示类型 `ChatMessage`，无法下沉 core）；
  子代理用 core 侧精简映射 `chunk_to_stream_json`。
- **送达目标**（`StreamEventDeliver`）：`BroadcastJsonDeliver`（统一事件通道，
  SSE 广播源）/ `TaskSinkDeliver`（子代理面板）/ `NullDeliver`（无通道路径）。

其余全部单点化：通道容量（`STREAM_FORWARD_BUFFER = 100`）、消费循环、
错误 chunk 防御性跳过、`type=chat_stream` + `session_id` 字段注入
（`inject_stream_event_fields`）、"rx 关闭即消费结束"（调用方 await 句柄 =
等待流排空）。

### 配套：`process_message_stream` 改为 sender 注入

用户轮此前在 `AgentCoordinator::process_message_stream` 内部建通道 +
`tokio::spawn` 整轮，返回 `Receiver` 给 server 消费——这是三处里唯一
"通道所有权在 core"的形态。统一为与 `process_wake` 同构：

```rust
async fn process_message_stream(..., sender: StreamEventSender) -> Result<()>;
```

- 通道所有权归调用方（server）；core 侧**等待整轮结束**（不内部 spawn）。
- server 的请求协程 `tokio::spawn` 整个调用，立即返回 HTTP 响应（行为不变）。
- 通道先建并立即消费 → 死锁形态从结构上不可能再现（不变量见模块文档）。

## 不变量（写死为结构约束）

**通道必须先于轮启动创建并立即开始消费。** 调用方顺序恒为：
`spawn_stream_forwarder(...)` → 拿到 sender → 交给轮。反向顺序（先跑轮、
后消费）在新结构下无法表达——通道与消费任务由同一函数原子创建。

## 后果

- 三处接线收敛为一个实现；新增流式路径（如未来的新触发源）只需提供
  映射器 + 送达目标，不会再手写消费循环。
- 字段注入单点：`type` / `session_id` 不再散落三处手写（含 System 通知
  边界事件、运行时 error 事件复用同一函数）。
- 通道容量统一 100；唤醒轮 context_window 仍传 0（映射器参数，保留现状——
  唤醒轮的 usage 展示不在本次范围）。
- 用户轮 assistant 边界事件改为经 `deliver` 下发（与流事件同一路径、排在
  流事件之后），语义与顺序不变。
- 回归保护：`stream_forward` 单测（映射/字段注入/轮进行中送达/句柄收尾/
  null 排空）+ `agent_core` 端到端测试
  `test_wake_forwarder_delivers_events_while_turn_running`（锁死 1151680
  那条路径：唤醒轮进行中事件必须已送达）。

## 关键文件

- `core/src/agent/stream_forward.rs` — 新增：转发器 + 送达抽象 + 字段注入
- `core/src/agent/agent_core.rs` — `AgentWakeForwarder` 改用统一转发器
  （注入 mapper + deliver，不再注入裸 `forward` 闭包）
- `core/src/agent/coordinator.rs` — `process_message_stream` 改收 sender；
  `wake_session` 用 `spawn_null_forwarder`
- `core/src/agent/tool_registry/agent_ops.rs` — 子代理改用统一转发器
- `server/src/api/chat/{handlers,services}.rs` — 用户轮：删手写转发协程与
  `pump_stream`，改为"建转发器 → 交 sender → 等排空 → 发边界"
- `server/src/agent_builder.rs` — 唤醒轮：注入 mapper（`map_chunk_to_event`）
  + `BroadcastJsonDeliver`
