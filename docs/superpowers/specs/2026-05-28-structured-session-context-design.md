# Structured Session Context Design

## Overview

将会话上下文存储从"面向 LLM 传输的扁平 Message 格式"改造为"面向持久化和统计的结构化格式（Message + Part 二级结构）"，在传输时通过 `ContextAssembler` 拼装为 DeepSeek 缓存最优的消息数组。

## Motivation

1. **统计与查询**：存储层包含 token 用量、费用、缓存命中率、时间戳等结构化元数据，支持会话级别的成本分析和使用统计
2. **DeepSeek 前缀缓存优化**：将易变内容（检索结果）从 system prompt 前缀中移除，改为末尾"注入窗"模式，使稳定的历史消息前缀最大化缓存命中率
3. **存储/传输解耦**：存储格式不再绑定 OpenAI 消息格式，为后续适配不同模型的消息格式留出空间

## Chapter 1: Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      Runtime Memory                          │
│                                                              │
│  SessionState                                                │
│  ├── structured_messages: Vec<StructuredMessage>  ← new      │
│  └── injectable_context: InjectableContext         ← new      │
│                                                              │
└──────────────┬──────────────────────┬────────────────────────┘
               │ persistence           │ assembly
               ▼                       ▼
┌──────────────────────┐  ┌────────────────────────────────────┐
│  Persistence (VFS)   │  │  ContextAssembler (new module)      │
│                      │  │                                    │
│  StructuredMessage   │  │  Input: structured_messages          │
│  + Part JSONL        │  │        + injectable_context         │
│                      │  │        + current_input              │
│                      │  │                                    │
│                      │  │  Output: Vec<Message> (cache-optimized)│
└──────────────────────┘  └──────────────┬─────────────────────┘
                                         │
                                         ▼
                              ┌──────────────────────┐
                              │  Transmission Layer   │
                              │  (unchanged)          │
                              │                      │
                              │  ChatCompletionRequest│
                              │  → convert_messages() │
                              │  → async-openai       │
                              └──────────────────────┘
```

### Changes

1. `SessionState` gains two new fields; existing `conversation: Vec<Message>` is replaced by `structured_messages`
2. New `ContextAssembler` module at `core/src/context/assembler.rs` — pure function, no side effects
3. Storage format upgrades from `MessageRecord` JSONL to `StructuredMessage` JSONL

### Unchanged

- `Message` type stays, used only in transmission layer
- `convert_messages()` in `core/src/model/provider/chat.rs` unchanged
- VFS three-tier architecture (Abstract/Overview/Detail) unchanged
- `AgentLoop`, `ChatCompletionRequest` unchanged

---

## Chapter 2: Storage Model

### StructuredMessage

```rust
struct StructuredMessage {
    id: String,
    parent_id: Option<String>,
    role: MessageRole,
    parts: Vec<Part>,
    tokens: TokenUsage,
    cost: f64,
    model_id: Option<String>,
    time: MessageTime,
    session_id: String,
    finish: Option<String>,       // "stop" | "length" | "tool_calls"
}
```

### Part

```rust
enum Part {
    Text       { text: String, time: PartTime },
    Reasoning  { text: String, time: PartTime },
    ToolCall   { id: String, name: String, arguments: String, time: PartTime },
    ToolResult { tool_call_id: String, content: String, time: PartTime },
}
```

`ToolCall` and `ToolResult` fields align with existing types in `core/src/common/types/tool.rs`.

### InjectableContext

```rust
struct InjectableContext {
    soul: String,
    rules_and_experiences: Vec<String>,
    memories: Vec<String>,
    last_updated: DateTime<Utc>,
}
```

**Boundaries:**

- **Memory** answers "what / who": user profile, environment facts, historical events — acquired by observation and recording
- **Experience** answers "how": lessons from practice, effective patterns, failure strategies — acquired by practice → reflection → induction

Both grow slowly and are merged into a single injection slot (splitting would add complexity with zero cache benefit).

Growth mechanisms (memory extraction, experience induction triggers) are out of scope — only container structure is defined here.

### Relationship to existing types

| Existing Type | Treatment |
|--------------|-----------|
| `Message` | Retained unchanged, transmission-only |
| `ToolCall` / `FunctionCall` | Retained, fields referenced by `Part::ToolCall` |
| `MessageRole` | Retained unchanged |
| `MessageRecord` (JSONL) | Replaced by `StructuredMessage` JSONL |
| `SessionState.conversation` | Replaced by `structured_messages` + `injectable_context` |

---

## Chapter 3: ContextAssembler Assembly Logic

### Signature

```
Input:  structured_messages, injectable, current_input
Output: Vec<Message> (for LLM)
```

### Assembly Pattern

```
messages[0]:   System  ← injectable.soul
messages[1]:   System  ← merge(injectable.rules_and_experiences, injectable.memories)
messages[2]:   User    ← structured_messages[0] parts::Text
messages[3]:   Asst    ← structured_messages[1] parts::Text + parts::ToolCall...
messages[4]:   Tool    ← structured_messages[2] parts::ToolResult
messages[5]:   Asst    ← structured_messages[3] parts::Text
 ──────────────── ↑ cache boundary ────────────────
messages[N-1]: User   ← current_input (changes every turn)
```

### StructuredMessage → Vec<Message> Conversion

One `StructuredMessage` may produce 1~N transmission `Message`s:

| StructuredMessage.role | parts content | Produces |
|----------------------|--------------|----------|
| User | Text | 1 User Message |
| Assistant | Text + ToolCall + Text | N: 1 Assistant (with tool_calls array) + corresponding Tool Results |
| Tool | ToolResult | 1 Tool Message |

ToolCall and ToolResult are matched by `Part::ToolCall.id` ↔ `Part::ToolResult.tool_call_id`.

### reasoning_content Retention Rule

Only retain `reasoning_content` in transmission messages when the **previous turn's assistant message contained tool calls**. If the previous assistant message was a plain text reply, discard reasoning_content.

Implementation: embedded in `ContextAssembler`'s `structured_to_messages()` — for each assistant `StructuredMessage`, check if `parts` contains `ToolCall` to decide.

**Rationale:** When the model output tool calls, its reasoning includes "why I'm calling this tool, what result I expect, how I'll use the result" — without this, the API sees a broken context chain.

### Existing `Message` Type Addition

Add `reasoning_content: Option<String>` to `Message` in `core/src/common/types/message.rs`.

---

## Chapter 4: Persistence

### JSONL Format

One `StructuredMessage` per line, serialized as JSON:

```json
{"id":"msg_xxx","parent_id":"msg_yyy","role":"assistant","parts":[{"type":"reasoning","text":"...","time":{"start":...,"end":...}},{"type":"text","text":"...","time":{"start":...,"end":...}}],"tokens":{"input":1000,"output":200,"reasoning":300,"cache":{"read":0,"write":0},"total":1500},"cost":0.0023,"model_id":"deepseek-v4-pro","time":{"created":...,"completed":...},"session_id":"ses_xxx","finish":"stop"}
```

### PersistentSessionManager Adaptation

- `Session.messages`: `Vec<Message>` → `Vec<StructuredMessage>`
- `save_message()`: serialize `StructuredMessage`
- `load_session()`: deserialize `StructuredMessage`
- File extension stays `.jsonl`, physical path: `{data_dir}/session/{session_id}/content.jsonl`

### Compatibility

No backward compatibility needed — the system has not been released.

---

## Chapter 5: Converter & Coordinator Adaptation

### Conversion Functions

Located in `core/src/context/assembler.rs`:

```rust
fn structured_to_messages(msg: &StructuredMessage) -> Vec<Message>
fn message_to_structured(msg: &Message, role: MessageRole, session_id: &str) -> StructuredMessage
```

- `Reasoning` parts enter transmission only via `reasoning_content` field, not as separate messages
- `ToolCall` parts are packed into the owning Assistant Message's `tool_calls` array
- `ToolResult` parts produce standalone Tool Messages

### Coordinator Changes

Before:
```
user input → append to state.conversation → context_pipeline.run()
→ inject system prompt → agent_loop.run(&mut conversation)
→ persist
```

After:
```
user input → construct StructuredMessage(User) → append to state.structured_messages
→ context_pipeline.run() (populates InjectableContext only, no conversation injection)
→ assembler.assemble(structured_messages, injectable, current_input) → Vec<Message>
→ agent_loop.run(&mut messages)
→ convert new Messages → StructuredMessages → append to state.structured_messages
→ persist
```

### ContextPipeline Simplification

- **Keep**: load soul → populate `injectable_context.soul`
- **Keep**: vector retrieval → populate `injectable_context.memories` and `injectable_context.rules_and_experiences`
- **Keep**: conversation compression → return compressed text → populate `injectable_context`
- **Remove**: system prompt injection into conversation

---

## Chapter 6: Error Handling & Testing

### Error Handling

`ContextAssembler` is a pure function — no I/O, no external calls — no new error variants needed.

Edge cases:
- Empty `structured_messages` → returns empty `Vec<Message>` (valid for newly created sessions)
- Empty `injectable_context` → skip messages[0] and messages[1], proceed to history + input
- Part/role mismatch (e.g., User message with ToolCall) → `debug!` log, skip the part, do not interrupt assembly

No new `TianyanError` variants.

### Testing Strategy

| Target | Type | Coverage |
|--------|------|----------|
| `structured_to_messages` | Unit | All Part combinations, reasoning_content retention/discard rules |
| `message_to_structured` | Unit | Round-trip conversion consistency |
| `ContextAssembler::assemble` | Unit | Cache boundary correctness, injection slot position |
| `SessionState` new fields | Unit | Structured message CRUD |
| `PersistentSessionManager` | Integration | JSONL read/write of StructuredMessage |
| Coordinator | Integration | End-to-end: input → persist → assemble → LLM call |

All tests use `MockVectorStorage` + temp directories, no external dependencies.

---

## Chapter 7: File Change List

### New Files

| File | Content |
|------|---------|
| `core/src/common/types/structured_message.rs` | `StructuredMessage`, `Part`, `PartTime`, `TokenUsage` |
| `core/src/context/assembler.rs` | `ContextAssembler`, `structured_to_messages()`, `message_to_structured()` |

### Modified Files

| File | Change |
|------|--------|
| `core/src/common/types/message.rs` | Add `reasoning_content: Option<String>` to `Message` |
| `core/src/common/types/mod.rs` | Register `structured_message` module |
| `core/src/agent/session_state.rs` | Add `structured_messages`, `injectable_context`; remove `conversation: Vec<Message>` |
| `core/src/agent/coordinator.rs` | Use `ContextAssembler`; context_pipeline no longer injects into conversation |
| `core/src/context/pipeline.rs` | Remove system prompt injection; populate `InjectableContext` instead |
| `core/src/context/mod.rs` | Register `assembler` module |
| `core/src/session/types.rs` | `Session.messages`: `Vec<Message>` → `Vec<StructuredMessage>` |
| `core/src/session/manager.rs` | Adapt `PersistentSessionManager` for `StructuredMessage` JSONL |
| `core/src/agent/loop.rs` | Adapt reasoning_content passthrough if needed |

### Unchanged Files

| File | Reason |
|------|--------|
| `core/src/model/types/chat.rs` | `ChatCompletionRequest` unchanged |
| `core/src/model/provider/chat.rs` | `convert_messages()` unchanged |
| `core/src/vfs/**` | Three-tier architecture unchanged |
| `core/src/memory/**` | Memory extraction logic unchanged |
| `core/src/context/retrieval/**` | Retrieval unchanged |
| `server/**` | Upper-layer calls unchanged |
| `gui/**` | Frontend unchanged |
