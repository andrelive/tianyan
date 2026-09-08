# ADR-002: StructuredMessage —— 核心数据结构

**日期**: 2026-06  
**状态**: ✅ 已采纳  
**影响范围**: 持久化、会话组装、跟踪、统计

---

## 背景

LLM 对话产生的消息需要持久化、组装成 LLM 输入、跟踪 Token 消耗和成本。若这些职责分散在不同模块，会导致数据不一致和重复转换。

## 决策

`StructuredMessage` 作为 **单一真相源**，贯穿持久化 → 会话组装 → 跟踪 → 统计全流程：

```rust
pub struct StructuredMessage {
    pub id: String, pub parent_id: Option<String>, pub role: MessageRole,
    pub parts: Vec<Part>,                  // Text | Reasoning | ToolCall | ToolResult
    pub tokens: DetailedTokenUsage,        // input/output/reasoning/cache
    pub cost: f64, pub model_id: Option<String>, pub time: MessageTime,
    pub session_id: String, pub finish: Option<String>,
    pub compression_marker: bool,          // ★ 会话压缩锚点
}
```

定义位置：`core/src/common/types/structured_message.rs`

## 四个核心职责

1. **持久化**：JSONL 格式写入 VFS。`AgentLoop` 每产生一条消息，实时调 `SessionManager::add_structured_message()` 落盘。工具调用消息不丢弃，全部持久化。

2. **会话组装**：存储与传输分离 — `StructuredMessage`（存储层）↔ `Message`（传输层）。`ContextAssembler::assemble()` 负责转换。顺序：soul → rules+memories → history（含当前用户输入）。

3. **会话跟踪**：`compression_marker` 标记压缩产生的摘要消息。加载会话时反向扫描到最近 marker，只加载 marker 及之后的消息（旧消息保留在磁盘）。压缩触发条件：marker 后 > 6 条消息。

4. **Token 统计**：LLM 响应 `TokenUsage` → `AgentLoop` 捕获 → `DetailedTokenUsage` 存入字段 → 持久化 → 聚合到 `AgentState.total_tokens`。

## 后果

- Agent 自闭环：`Agent::process_message(session_id, msg)` 负责完整生命周期（加载 → 执行 → 持久化 → 压缩 → 返回）
- Server 不手动管理 session
- 每条消息精确追踪 Token 消耗和成本

## 关键文件

- `core/src/common/types/structured_message.rs` — 类型定义
- `core/src/session/manager.rs` — `add_structured_message()`
- `core/src/context/assembler.rs` — `assemble()` 存储→传输转换

---

## 修订记录（0.3.4）

**压缩摘要消息携带真实 token 用量**：摘要请求不走 AgentLoop（无 assistant 消息承载该请求的
input/output/cache），此前 `tokens` 保持全零——压缩消耗从会话统计中完全丢失，且前端
`lastMessageUsage` 跳过摘要消息导致压缩后圆环占用不变。`summarize`/`incremental_summarize`
改用 `chat_completion()`（丢弃 usage 的 `chat()` 便捷方法弃用），`CompressionResult.summary_usage`
→ `CompressionOutcome` 透传，`compress_for_session` 将压缩请求真实 token 用量写入摘要消息的
`tokens` 并持久化。前端 `sumSessionUsage` 自动计入（压缩请求 = 一次完整 LLM 请求）；圆环
`lastMessageUsage` 对 `compression_marker` 消息用「第一条 assistant 输入（≈系统前缀）+ 摘要
输出」估算压缩后上下文（摘要 input 是压缩前上下文，不能直接作占用）。API 层 `ChatMessage`
新增 `compression_marker` 字段（历史加载 + 流式边界事件映射）。
