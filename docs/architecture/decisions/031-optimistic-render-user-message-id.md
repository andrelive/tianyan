# ADR-031: 乐观渲染 + user_message_id 确认 + 统一流式

**日期**: 2026-09-07
**状态**: 已采纳

## 背景

ADR-028/029 建立了统一事件推送（落库即广播 message 事件 + 订阅快照），但遗留两个架构问题：

1. **用户消息不乐观渲染**（b749eb0）：发送后等广播/边界事件插入——有延迟，且靠"插入位置 + id 去重"模糊匹配（本地 randomUUID 与广播 id 不一致曾导致重复）。
2. **消息广播与流式增量双路径交错**：assistant 消息的"权威版"（message 广播）与本地增量累积（chat_stream）异步交错——多工具同轮时把 segments 拼出重复段（工具卡片显示两次）。
3. **后台任务完成通知绕路**：任务完成 → system 通知落库 → 广播 message 事件 → 前端渲染通知气泡 + 轮询唤醒——但通知的对象本是 LOOP（`process_wake` 已有），只是唤醒轮是 Plain（非流式），输出没有别的通道才依赖广播。

## 决策

### 1. 乐观渲染 + user_message_id 确认（id 回显）

```
前端发送：POST /chat/stream（message 带 user_message_id: <前端uuid>）
         → 本地立即插入用户消息（乐观渲染，id: null 占位）
服务端：  落库用户消息（真实 id）→ 流式序列内发确认事件
         { chunk_type: 'user_message_id', user_message_id, message_id }
前端：    比对 user_message_id → 本地乐观消息替换为真实 id（字段删除）
```

- `user_message_id` 生命周期：请求 → 确认 → 弃（不持久化、不参与回退/对齐）。
- 回显走**事件**不走 HTTP 响应（POST /chat/stream 是开关语义立即返回，落库在协程内异步发生）。
- 用户消息边界事件（send_message_boundary）保留（非乐观客户端兼容）；乐观渲染后前端按 `user_message_id` 存在跳过边界插入，确认后按 id 去重。

### 2. assistant 纯流式（无权威版广播）

- 增量事件（thought/tool_call/observation/answer/usage）已完整——前端累积即终版。
- **message 广播移除**（`BroadcastingSessionManager` 收窄为纯委托）：主会话消息经流式增量 + 完成事件（user_message_id 确认 / pump 边界）到前端。
- 丢事件兜底：订阅快照（打开/重连 replace）——与广播职责重叠，移除后快照独立承担。

### 3. 任务完成通知走 LOOP（唤醒轮流式化）

- 任务完成 → `process_wake`（已有）**流式化**：run_stream + StreamEventSender → 宿主（server）转发到统一事件通道（chat_stream 事件）→ 前端打字机看到汇总输出。
- system 通知消息**只落库**（LLM 上下文），不再广播；前端 `hasTaskNotice`/`wakeActive`/唤醒轮询逻辑删除。

### 4. 子代理流式事件补全路由字段

- 子代理流式转发（run_subagent_loop → TaskEventSink）此前**无 type/session_id**——事件被前端丢弃（子代理逐 token 流式实际未生效）。
- 修复：事件带 `type=chat_stream` + `session_id=task_id`——前端 `routeChatStreamEvent` 按 session_id 路由到任务面板的活跃归约器。

## 事件协议（变更后）

```
请求：POST /chat/stream
  message: { role, content, user_message_id?, images?, timestamp }

事件（GET /events 统一通道）：
  chat_stream（增量 + 完成）：
    { chunk_type: 'thought'|'tool_call'|'observation'|'answer'|'usage'|'error'|'message' }
    { chunk_type: 'user_message_id', user_message_id, message_id }   ← 新增
  snapshot（订阅快照：历史 + cursor）
  task_status / command_output（任务状态，全局广播）

移除：{ type: 'message' } 广播（user/assistant/system 全移除）
```

## 后果

- 前端主路径收敛为：**乐观插入 → 增量累积 → 确认事件（id 回显）→ 快照兜底**——单一顺序流，合并竞态（工具卡片重复）从根上消失。
- 用户消息发送即显示（乐观渲染），回退锚点（用户消息 id）由确认事件保证。
- 唤醒轮输出打字机展示（替代通知气泡 + 轮询指示）。
- 子代理面板 = 订阅快照 + 活跃流 reducer（chat_stream 事件路由）。
- 断点对齐（seq 跳号检测）随 message 广播移除——重连快照独立兜底。

## 关键文件

- `core/src/agent/types.rs` — StreamChunkType::UserMessageId + send_user_message_id
- `core/src/agent/agent_core.rs` — run_agent_turn 确认事件；process_wake 流式化（返回 Receiver）；AgentWakeForwarder 注入转发器
- `core/src/agent/tool_registry/agent_ops.rs` — 子代理流式事件补 type/session_id
- `server/src/event_push.rs` — BroadcastingSessionManager 收窄为纯委托（广播移除）
- `server/src/api/chat/{types,services}.rs` — ChatRequest.user_message_id；map_chunk_to_event 映射确认字段
- `server/src/agent_builder.rs` — 唤醒轮事件转发（map_chunk_to_event → chat_stream → 统一通道）
- `gui-vite/src/components/chat/ChatPanel.tsx` — 乐观渲染（本地插入 + user_message_id）；删唤醒轮询
- `gui-vite/src/lib/chat-stream.ts` — reducer 处理 user_message_id 确认
- `gui-vite/src/lib/store/chat-slice.ts` — confirmUserMessageId；删 mergeServerMessages；迁移逻辑含乐观 user
- `gui-vite/src/hooks/use-unified-events.ts` — 删 message 分支/断点对齐（快照独立兜底）
