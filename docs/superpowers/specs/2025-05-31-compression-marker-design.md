# 压缩持久化与会话历史完整性设计

## 概述

当前 Agent 存在两个问题：
1. **会话历史不完整**：AgentLoop 中产生的工具调用消息（tool_calls、tool_results）在 loop 结束后丢弃，持久化层只记录最终 Answer
2. **压缩结果不持久化**：压缩后的摘要仅在内存中替换 `state.structured_messages`，重登后丢失，预压缩消息重新加载

本设计让 Agent 持有 `SessionManager`，自闭环管理会话的加载、执行、持久化和压缩。Server 退化为薄路由层。

## 数据模型变更

### StructuredMessage 新增字段

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `compression_marker` | `bool` | `false`（serde default） | 标记该消息为压缩分界点 |

- `true`：该消息是压缩产生的摘要 System Message，标记从此处开始为加载工作集
- 序列化时始终输出（`skip_serializing_if = false`），兼容旧文件

## 接口变更

### SessionManager 新增方法

```rust
async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()>;
```

直接持久化 `StructuredMessage`，不经过 `Message` 中间转换。现有 `add_message(Message)` 标记为废弃。

### 读取时 Marker 截断

`PersistentSessionManager::load_session_from_vfs` 解析完 JSONL 后，从后向前扫描 `compression_marker == true` 的消息，只保留该消息及其后的所有消息作为 `Session.messages`。旧消息保留在文件中但不加载。

### AgentCoordinator 签名变更

```rust
// 旧
async fn process_message(&self, state: Arc<RwLock<SessionState>>, message: &str)
    -> Result<AgentResponse>;

// 新
async fn process_message(&self, session_id: &str, message: &str)
    -> Result<AgentResponse>;
```

### Agent 新增依赖

- `session_manager: Arc<dyn SessionManager>`

## 核心流程

### Agent::process_message(session_id, msg)

```
1. 加载会话
   session = session_manager.get_session(session_id)
   → JSONL 反序列化（已在读取时按 compression_marker 截断）
   → 构建 SessionState

2. 持久化用户消息
   user_sm = message_to_structured(&user_msg, session_id, parent_id)
   session_manager.add_structured_message(session_id, user_sm)
   state.add_structured_message(user_sm)

3. 上下文组装
   prepare_context() → Vec<Message>

4. AgentLoop 执行
   agent_loop.run(&mut messages)
   → loop 内每产生一条消息，实时调用 session_manager.add_structured_message()

5. 压缩判断 + 持久化
   state.structured_messages 从后向前扫描最近 compression_marker
   → 统计 marker 后消息数
   → 超过阈值 → LLM 摘要
   → summary_sm = StructuredMessage { compression_marker: true, role: System, parts: [Text(summary)] }
   → session_manager.add_structured_message(session_id, summary_sm)

6. 返回 AgentResponse
```

### AgentLoop 实时持久化

```rust
// 每轮 LLM 调用后
let assistant_sm = message_to_structured(&assistant_msg, session_id, parent_id);
session_manager.add_structured_message(session_id, assistant_sm)?;
messages.push(assistant_msg);

// 工具执行后
for (call_id, result) in tool_results {
    let tool_sm = message_to_structured(&tool_msg, session_id, parent_id);
    session_manager.add_structured_message(session_id, tool_sm)?;
    messages.push(tool_msg);
}
```

### 压缩策略简化

当前压缩策略（`CompressionStrategy::Summarize/Select/Hybrid`）中的 `preserve_recent_messages` 分割逻辑在新模型下移除。压缩统一为：取最近 marker 之后的所有消息 → LLM 全量摘要 → 输出一条 System Summary 消息。

cached_summary 增量摘要机制保留。

触发条件：marker 后的消息估算 token 数超过上下文窗口 50%（与当前 `compress_if_needed` 条件一致）。

### AgentLoop 接口变更

`AgentLoop::new` 新增 `session_manager: Arc<dyn SessionManager>` 参数。`AgentLoop::run` 新增 `session_id: &str` 参数，用于构造 `StructuredMessage` 时填充 `session_id` 字段。

`parent_id` 链：loop 中首条消息的 parent_id = 会话最后一条消息的 id；后续消息的 parent_id = 前一条新增消息的 id。Loop 内部维护追踪。

### Server 简化

`ChatService::process_message` 从：
```rust
// 旧
let (last_message, state) = bootstrap_session(session_id, messages).await;
let response = agent.process_message(state, &last_message).await;
session_manager.add_message(session_id, assistant_msg).await;
```

简化为：
```rust
// 新
let response = agent.process_message(session_id, &last_message).await;
// 无需手动持久化 assistant 回复 — Agent 已在 loop 中完成
// 无需 bootstrap_session — Agent 内部加载
```

### 流程对比

```
当前:  Server 构建 SessionState → Agent 执行 → loop 消息丢弃 → Server 只存最终回复 → 压缩只在内存

新设计: Server 传 session_id + msg → Agent 加载 → Agent 执行 + 实时持久化 → Agent 压缩 + 持久化 → 返回
```

## 变更清单

| 文件 | 变更 |
|---|---|
| `core/src/common/types/structured_message.rs` | 新增 `compression_marker: bool` 字段 |
| `core/src/session/manager.rs` | Trait 新增 `add_structured_message()`；读取时扫描 marker 截断工作集；`add_message()` 标记废弃 |
| `core/src/agent/loop.rs` | 注入 `Arc<dyn SessionManager>`，loop 中每产生一条消息实时调 `add_structured_message()` |
| `core/src/agent/coordinator.rs` | `process_message` 签名改为 `(session_id, msg)`；Agent 持 `SessionManager`；流程改为自闭环：加载→执行→持久化→压缩 |
| `core/src/agent/builder.rs` | `AgentBuilder` 新增 `with_session_manager(Arc<dyn SessionManager>)` |
| `core/src/context/pipeline.rs` | 压缩结果返回 `StructuredMessage`（含 `compression_marker`），不再原地替换 `state.structured_messages` |
| `core/src/context/compression/mod.rs` | 移除 `CompressionStrategy` 中与 `preserve_recent_messages` 相关的分割逻辑 |
| `server/src/api/chat/services.rs` | 删除 `bootstrap_session`；删除手动 `add_message` 持久化；简化为 `agent.process_message(session_id, content)` |
| `server/src/agent_builder.rs` | `WizardModeAgent` 存根签名同步 |

## 兼容性

- 旧 session 文件无 `compression_marker` 字段 → serde default `false` → 全部消息正常加载（行为不变）
- `add_message(Message)` 标记废弃但保留实现，避免破坏现有 `create_session` 内部调用
