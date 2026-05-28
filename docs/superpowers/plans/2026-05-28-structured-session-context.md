# Structured Session Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor session context storage from flat `Vec<Message>` to structured `Vec<StructuredMessage>` (Message + Part) with a storage/transmission separation layer (`ContextAssembler`), optimizing for DeepSeek prefix caching.

**Architecture:** New `StructuredMessage` type with nested `Part` enum for persistent storage. New `ContextAssembler` pure function that takes `structured_messages` + `InjectableContext` and produces `Vec<Message>` for LLM transmission. Existing `Message` type retained unchanged for transmission only. `ContextPipeline` simplified to populate `InjectableContext` instead of injecting into conversation.

**Tech Stack:** Rust, serde, chrono, tokio. Follows existing project patterns (serde derives, `Result<T, TianyanError>`, TDD).

---

## File Responsibility Map

| File | Responsibility |
|------|----------------|
| `core/src/common/types/structured_message.rs` (NEW) | `StructuredMessage`, `Part`, `DetailedTokenUsage`, `CacheUsage`, `PartTime`, `MessageTime` — pure data types |
| `core/src/common/types/message.rs` (MODIFY) | Add `reasoning_content: Option<String>` to `Message` |
| `core/src/common/types/mod.rs` (MODIFY) | Register `structured_message` module |
| `core/src/agent/session_state.rs` (MODIFY) | Add `InjectableContext` struct; add `structured_messages`, `injectable_context` fields; replace `conversation: Vec<Message>` |
| `core/src/context/assembler.rs` (NEW) | `ContextAssembler::assemble()`, `structured_to_messages()`, `message_to_structured()` — pure functions |
| `core/src/context/mod.rs` (MODIFY) | Register `assembler` module |
| `core/src/context/pipeline.rs` (MODIFY) | Change `run()` to return `InjectableContext`; remove system prompt injection |
| `core/src/agent/coordinator.rs` (MODIFY) | Wire up new flow: structured_messages → assembler → agent_loop → back-convert |
| `core/src/session/types.rs` (MODIFY) | `Session.messages`: `Vec<Message>` → `Vec<StructuredMessage>`; remove `MessageRecord` |
| `core/src/session/manager.rs` (MODIFY) | Adapt `PersistentSessionManager` for `StructuredMessage` JSONL |
| `core/src/agent/loop.rs` (MODIFY) | Minor: handle `reasoning_content` field |

---

### Task 1: Create StructuredMessage and related types

**Files:**
- Create: `core/src/common/types/structured_message.rs`

- [ ] **Step 1: Write the new types file**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::message::MessageRole;

/// Token 使用详情，含缓存命中信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetailedTokenUsage {
    #[serde(default)]
    pub input: usize,
    #[serde(default)]
    pub output: usize,
    #[serde(default)]
    pub reasoning: usize,
    #[serde(default)]
    pub cache: CacheUsage,
    #[serde(default)]
    pub total: usize,
}

/// 缓存 Token 统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheUsage {
    #[serde(default)]
    pub read: usize,
    #[serde(default)]
    pub write: usize,
}

/// Part 级别的时间戳（毫秒）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartTime {
    pub start: i64,
    pub end: i64,
}

/// Message 级别的时间戳。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageTime {
    pub created: i64,
    pub completed: i64,
}

/// 结构化消息中的内容块。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Part {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        text: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        id: String,
        name: String,
        arguments: String,
        #[serde(default)]
        time: PartTime,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        content: String,
        #[serde(default)]
        time: PartTime,
    },
}

/// 面向持久化的结构化消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredMessage {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub tokens: DetailedTokenUsage,
    #[serde(default)]
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub time: MessageTime,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<String>,
}
```

- [ ] **Step 2: Run cargo check to verify compilation**

Run: `cargo check -p tianyan-core`

- [ ] **Step 3: Commit**

```bash
git add core/src/common/types/structured_message.rs
git commit -m "feat: add StructuredMessage and Part types for session context storage"
```

---

### Task 2: Add reasoning_content to Message

**Files:**
- Modify: `core/src/common/types/message.rs:28-41`

- [ ] **Step 1: Add the field to Message struct**

```rust
/// 对话中的一条消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息发送者的角色
    pub role: MessageRole,
    /// 消息内容
    pub content: String,
    /// 当 role 为 Assistant 时，模型发出的工具调用列表。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 当 role 为 Tool 时，对应哪个 tool_call 的 ID。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// 模型推理内容（reasoning_content），仅 DeepSeek 等支持思维链的模型使用。
    /// 上一轮 assistant 含 tool_calls 时，下一轮必须原样保留。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}
```

Update `Message::new()` constructor:

```rust
pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
    Self {
        role,
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }
}
```

Update `Message::assistant_with_tools()`:

```rust
pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
    Self {
        role: MessageRole::Assistant,
        content: content.into(),
        tool_calls: Some(tool_calls),
        tool_call_id: None,
        reasoning_content: None,
    }
}
```

Update `Message::tool()`:

```rust
pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
    Self {
        role: MessageRole::Tool,
        content: content.into(),
        tool_calls: None,
        tool_call_id: Some(tool_call_id.into()),
        reasoning_content: None,
    }
}
```

- [ ] **Step 2: Run existing tests to verify no regressions**

Run: `cargo test -p tianyan-core common::types::message -- --nocapture`

- [ ] **Step 3: Commit**

```bash
git add core/src/common/types/message.rs
git commit -m "feat: add reasoning_content field to Message for DeepSeek cache optimization"
```

---

### Task 3: Register new module and add InjectableContext

**Files:**
- Modify: `core/src/common/types/mod.rs`
- Modify: `core/src/agent/session_state.rs`

- [ ] **Step 1: Register structured_message in mod.rs**

In `core/src/common/types/mod.rs`, add the module declaration and re-export:

```rust
mod structured_message;

pub use structured_message::{
    CacheUsage, DetailedTokenUsage, MessageTime, Part, PartTime, StructuredMessage,
};
```

- [ ] **Step 2: Add InjectableContext to SessionState**

In `core/src/agent/session_state.rs`, add imports and the struct:

```rust
use chrono::{DateTime, Utc};
```

Add after the imports block:

```rust
/// 可注入的上下文内容，由 ContextPipeline 填充，由 ContextAssembler 组装使用。
#[derive(Debug, Clone, Default)]
pub struct InjectableContext {
    /// 智能体核心人格（soul.md）。
    pub soul: String,
    /// 经验与方法论。
    pub rules_and_experiences: Vec<String>,
    /// 用户画像与环境事实。
    pub memories: Vec<String>,
    /// 最后更新时间。
    pub last_updated: DateTime<Utc>,
}

impl InjectableContext {
    /// 创建空的注入上下文。
    pub fn new() -> Self {
        Self {
            last_updated: Utc::now(),
            ..Default::default()
        }
    }
}
```

- [ ] **Step 3: Replace conversation with structured_messages in SessionState**

Update the `SessionState` struct:

```rust
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 会话 ID。
    pub session_id: String,
    /// 结构化消息历史（持久化格式）。
    pub structured_messages: Vec<StructuredMessage>,
    /// 可注入上下文（soul + rules + memories）。
    pub injectable_context: InjectableContext,
    /// 当前目标。
    pub current_goal: Option<String>,
    /// 待追问问题。
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    /// 最后活动时间。
    pub last_activity: Instant,
    /// 上下文窗口。
    pub context_window: Option<ContextWindow>,
    /// 待持久化记忆。
    pub pending_memories: Vec<crate::common::types::MemoryEntry>,
    /// 总 token 数。
    pub total_tokens: usize,
    /// 开始时间。
    pub start_time: Instant,
}
```

Update the constructor and existing methods. First `SessionState::new()`:

```rust
pub fn new(session_id: &str) -> Self {
    let now = Instant::now();
    Self {
        session_id: session_id.to_string(),
        structured_messages: Vec::new(),
        injectable_context: InjectableContext::new(),
        current_goal: None,
        pending_clarification: None,
        last_activity: now,
        context_window: None,
        pending_memories: Vec::new(),
        total_tokens: 0,
        start_time: now,
    }
}
```

Replace message helper methods with structured versions:

```rust
/// 添加用户消息。
pub fn add_user_message(&mut self, content: impl Into<String>) {
    let msg = StructuredMessage {
        id: format!("msg_{}", chrono::Utc::now().timestamp_millis()),
        parent_id: self.structured_messages.last().map(|m| m.id.clone()),
        role: MessageRole::User,
        parts: vec![Part::Text {
            text: content.into(),
            time: PartTime::default(),
        }],
        tokens: DetailedTokenUsage::default(),
        cost: 0.0,
        model_id: None,
        time: MessageTime::default(),
        session_id: self.session_id.clone(),
        finish: None,
    };
    self.structured_messages.push(msg);
    self.trim_conversation();
    self.last_activity = Instant::now();
}

/// 添加结构化消息并裁剪历史。
pub fn add_structured_message(&mut self, msg: StructuredMessage) {
    self.structured_messages.push(msg);
    self.trim_conversation();
    self.last_activity = Instant::now();
}

/// 获取最近 n 条消息。
pub fn recent_messages(&self, n: usize) -> Vec<&StructuredMessage> {
    self.structured_messages
        .iter()
        .rev()
        .take(n)
        .rev()
        .collect()
}

/// 消息总数。
pub fn message_count(&self) -> usize {
    self.structured_messages.len()
}
```

Remove `add_assistant_message()`, `add_message()` methods. Update `to_session()`:

```rust
/// 转换为 Session。
pub fn to_session(&self) -> Session {
    let mut session = Session::new(&self.session_id);
    session.messages = self.structured_messages.clone();
    session
}
```

Remove `build_prompt_context()`, `get_conversation()`. Update `trim_conversation()` to work on `structured_messages`:

```rust
fn trim_conversation(&mut self) {
    if self.structured_messages.len() <= MAX_CONVERSATION_MESSAGES {
        return;
    }
    let keep_from = self.structured_messages.len().saturating_sub(KEEP_RECENT_MESSAGES);
    if keep_from > 0 {
        self.structured_messages.drain(0..keep_from);
    }
}
```

Add import for new types at top:

```rust
use crate::common::types::{MessageRole, Part, PartTime, StructuredMessage, MessageTime, DetailedTokenUsage};
```

- [ ] **Step 4: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 5: Commit**

```bash
git add core/src/common/types/mod.rs core/src/agent/session_state.rs
git commit -m "feat: add InjectableContext, replace conversation with structured_messages in SessionState"
```

---

### Task 4: Create ContextAssembler

**Files:**
- Create: `core/src/context/assembler.rs`
- Modify: `core/src/context/mod.rs`

- [ ] **Step 1: Write the assembler module**

```rust
//! 上下文组装器。
//!
//! 负责将存储层的 StructuredMessage 和 InjectableContext 组装为
//! 传输层的 Vec<Message>，优化 DeepSeek 前缀缓存命中率。

use crate::agent::session_state::InjectableContext;
use crate::common::types::{
    Message, MessageRole, Part, StructuredMessage, ToolCall as CoreToolCall,
};

/// 上下文组装器——纯函数，无副作用。
pub struct ContextAssembler;

impl ContextAssembler {
    /// 将结构化消息和注入上下文组装为 LLM 传输格式。
    ///
    /// 输出顺序（缓存最优）：
    /// - messages[0]: injectable.soul（system）
    /// - messages[1]: rules + memories（system）
    /// - messages[2..N-1]: 历史对话
    /// - messages[N-1]: 当前用户输入
    pub fn assemble(
        structured_messages: &[StructuredMessage],
        injectable: &InjectableContext,
        current_input: &str,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 注入 soul
        if !injectable.soul.is_empty() {
            messages.push(Message::system(&injectable.soul));
        }

        // 注入 rules + memories
        let mut context_parts: Vec<String> = Vec::new();
        if !injectable.rules_and_experiences.is_empty() {
            context_parts.push("## 经验与方法论\n".to_string());
            for rule in &injectable.rules_and_experiences {
                context_parts.push(format!("- {}\n", rule));
            }
        }
        if !injectable.memories.is_empty() {
            context_parts.push("## 用户画像与记忆\n".to_string());
            for mem in &injectable.memories {
                context_parts.push(format!("- {}\n", mem));
            }
        }
        if !context_parts.is_empty() {
            messages.push(Message::system(context_parts.concat()));
        }

        // 历史对话
        for sm in structured_messages {
            messages.extend(Self::structured_to_messages(sm));
        }

        // 当前用户输入
        messages.push(Message::user(current_input));

        messages
    }

    /// 将单个 StructuredMessage 转换为 1~N 条传输层 Message。
    fn structured_to_messages(sm: &StructuredMessage) -> Vec<Message> {
        let mut messages = Vec::new();

        match sm.role {
            MessageRole::User => {
                for part in &sm.parts {
                    if let Part::Text { text, .. } = part {
                        messages.push(Message::user(text.clone()));
                    }
                }
            }
            MessageRole::Assistant => {
                let has_tool_calls = sm.parts.iter().any(|p| matches!(p, Part::ToolCall { .. }));

                let mut content = String::new();
                let mut tool_calls: Vec<CoreToolCall> = Vec::new();
                let mut reasoning_content: Option<String> = None;

                for part in &sm.parts {
                    match part {
                        Part::Text { text, .. } => {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        }
                        Part::Reasoning { text, .. } => {
                            if has_tool_calls {
                                // 有工具调用时保留 reasoning
                                reasoning_content = Some(text.clone());
                            }
                        }
                        Part::ToolCall { id, name, arguments, .. } => {
                            tool_calls.push(CoreToolCall {
                                id: id.clone(),
                                call_type: crate::common::types::ToolCallType::Function,
                                function: crate::common::types::FunctionCall {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                },
                            });
                        }
                        Part::ToolResult { .. } => {
                            // ToolResult 不在 assistant 消息中处理，
                            // 由后续的 Tool role message 处理
                        }
                    }
                }

                if !content.is_empty() || !tool_calls.is_empty() {
                    let mut msg = Message {
                        role: MessageRole::Assistant,
                        content,
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                        reasoning_content,
                    };
                    messages.push(msg);
                }
            }
            MessageRole::Tool => {
                for part in &sm.parts {
                    if let Part::ToolResult { tool_call_id, content, .. } = part {
                        messages.push(Message::tool(tool_call_id.clone(), content.clone()));
                    }
                }
            }
            MessageRole::System => {
                for part in &sm.parts {
                    if let Part::Text { text, .. } = part {
                        messages.push(Message::system(text.clone()));
                    }
                }
            }
        }

        messages
    }

    /// 将 LLM 返回的传输层 Message 转回 StructuredMessage（用于持久化）。
    pub fn message_to_structured(
        msg: &Message,
        role: MessageRole,
        session_id: &str,
        parent_id: Option<&str>,
    ) -> StructuredMessage {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let default_time = crate::common::types::PartTime::default();

        let id = format!("msg_{}", now_ms);

        let mut parts = Vec::new();

        // 如果有 reasoning_content，先添加 Reasoning part
        if let Some(ref reasoning) = msg.reasoning_content {
            if !reasoning.is_empty() {
                parts.push(Part::Reasoning {
                    text: reasoning.clone(),
                    time: default_time.clone(),
                });
            }
        }

        // 添加文本内容
        if !msg.content.is_empty() {
            parts.push(Part::Text {
                text: msg.content.clone(),
                time: default_time.clone(),
            });
        }

        // 添加工具调用
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                parts.push(Part::ToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                    time: default_time.clone(),
                });
            }
        }

        StructuredMessage {
            id,
            parent_id: parent_id.map(|s| s.to_string()),
            role,
            parts,
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now_ms,
                completed: now_ms,
            },
            session_id: session_id.to_string(),
            finish: Some("stop".to_string()),
        }
    }
}
```

- [ ] **Step 2: Register assembler in context/mod.rs**

Add the module declaration and re-export:

```rust
pub mod assembler;

pub use assembler::ContextAssembler;
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 4: Commit**

```bash
git add core/src/context/assembler.rs core/src/context/mod.rs
git commit -m "feat: add ContextAssembler with structured_to_messages and message_to_structured"
```

---

### Task 5: Refactor ContextPipeline to populate InjectableContext

**Files:**
- Modify: `core/src/context/pipeline.rs`

- [ ] **Step 1: Change run() to return InjectableContext**

Replace the `run()` method signature and implementation. The pipeline no longer returns `ContextWindow` with a monolithic system_prompt string — instead it populates `InjectableContext`.

Add import at top:

```rust
use crate::agent::session_state::InjectableContext;
```

Change the `run()` method:

```rust
/// 执行完整上下文管线。
///
/// - `query` - 用户查询（用于检索和规则匹配）
/// - `conversation` - 当前对话历史（可变引用，压缩可能修改）
///
/// 返回 InjectableContext（soul + rules + memories）和压缩摘要。
pub async fn run(
    &self,
    query: &str,
    conversation: &mut Vec<Message>,
) -> Result<(InjectableContext, Option<String>)> {
    let mut injectable = InjectableContext::new();

    // Load soul
    let soul_uri = crate::common::types::AgentPath::Soul.uri();
    match self.vfs.read_content(&soul_uri, ContentLevel::Detail).await {
        Ok(content) => {
            injectable.soul = content;
        }
        Err(e) => {
            tracing::warn!(error = %e, uri = %soul_uri, "加载核心提示词失败");
        }
    }

    // Load learned rules and experiences
    let rules = self.load_relevant_rules(query).await;
    injectable.rules_and_experiences = rules;

    // Load memories (retrieval)
    let memories = self.load_memories(query).await;
    injectable.memories = memories;

    // Compression
    let summary = self.compress_if_needed(conversation).await?;

    Ok((injectable, summary))
}
```

Add `load_memories()` method:

```rust
/// 检索相关记忆。
async fn load_memories(&self, query: &str) -> Vec<String> {
    match self
        .retriever
        .retrieve_by_namespace(query, self.default_top_k, ContextNamespace::Memory)
        .await
    {
        Ok(results) => results
            .into_iter()
            .filter_map(|r| r.content)
            .filter(|c| !c.trim().is_empty())
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "记忆检索失败");
            Vec::new()
        }
    }
}
```

Remove the now-unused `search()` method (replaced by `load_memories()` specialized for memory namespace).

- [ ] **Step 2: Update callers in pipeline.rs tests or leave as-is**

Since pipeline no longer returns `ContextWindow`, we need to check if `ContextWindow` is still used elsewhere.

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 4: Commit**

```bash
git add core/src/context/pipeline.rs
git commit -m "refactor: ContextPipeline returns InjectableContext instead of ContextWindow"
```

---

### Task 6: Update Coordinator to use ContextAssembler

**Files:**
- Modify: `core/src/agent/coordinator.rs`

- [ ] **Step 1: Add imports**

```rust
use crate::context::assembler::ContextAssembler;
use crate::agent::session_state::InjectableContext;
```

- [ ] **Step 2: Rewrite process_message()**

Replace the `process_message()` body at line 355-471 with:

```rust
#[instrument(skip(self, state), fields(session_id = %state.session_id))]
async fn process_message(
    &self,
    state: &mut SessionState,
    message: &str,
) -> Result<AgentResponse> {
    let start = Instant::now();
    let message = message.to_string();

    state.add_user_message(&message);

    // Convert structured_messages to Vec<Message> for pipeline compression
    let mut conversation: Vec<Message> = state.structured_messages.iter()
        .flat_map(|sm| ContextAssembler::structured_to_messages(sm))
        .collect();

    // Run context pipeline to populate injectable context and optionally compress
    let injectable = match self.context_pipeline.run(&message, &mut conversation).await {
        Ok((ctx, summary)) => {
            // Record rule hits
            let injected_rules = ctx.rules_and_experiences.len();
            if injected_rules > 0 {
                self.harness.metrics.record_rule_hit(injected_rules).await;
            }

            // If compression happened, replace structured_messages with compressed version
            if summary.is_some() {
                let parent_id = state.structured_messages.last().map(|m| m.id.as_str());
                let new_structured: Vec<StructuredMessage> = conversation.iter()
                    .map(|m| ContextAssembler::message_to_structured(
                        m, m.role, &state.session_id, parent_id,
                    ))
                    .collect();
                state.structured_messages = new_structured;
            }

            state.injectable_context = ctx.clone();
            ctx
        }
        Err(e) => {
            tracing::warn!(error = %e, "上下文管线执行失败");
            self.harness.metrics.record_pipeline_failure().await;
            state.injectable_context.clone()
        }
    };

    // Assemble messages for LLM (input already in structured_messages as user message)
    let mut messages = ContextAssembler::assemble(
        &state.structured_messages,
        &injectable,
        "", // current_input already in structured_messages as last entry
    );

    let loop_result = self.agent_loop.run(&mut messages, None).await;

    let response = match loop_result {
        Ok(AgentLoopResult::Answer(content)) => {
            // Convert assistant response to StructuredMessage
            let parent_id = state.structured_messages.last().map(|m| m.id.as_str());
            let sm = ContextAssembler::message_to_structured(
                &Message::assistant(&content),
                MessageRole::Assistant,
                &state.session_id,
                parent_id,
            );
            state.add_structured_message(sm);

            let mut resp = AgentResponse::simple(content);
            resp.processing_time_ms = start.elapsed().as_millis() as u64;
            self.harness.metrics.record_execution(true).await;
            resp
        }
        Ok(AgentLoopResult::NeedsClarification { question }) => {
            let question_obj = ClarificationQuestion {
                question,
                question_type: QuestionType::OpenEnded,
                options: None,
                required: true,
            };
            state.pending_clarification = Some(vec![question_obj.clone()]);
            let formatted = format_clarification_questions(&[question_obj.clone()]);
            let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
            resp.processing_time_ms = start.elapsed().as_millis() as u64;
            resp
        }
        Err(e) => {
            tracing::warn!(error = %e, "AgentLoop 执行失败");
            self.harness.metrics.record_execution(false).await;
            let mut resp = AgentResponse::error(format!("处理失败：{}", e));
            resp.processing_time_ms = start.elapsed().as_millis() as u64;
            resp
        }
    };

    {
        let mut st = self.state.write().await;
        st.conversations_processed += 1;
    }

    if response.is_complete && !response.needs_clarification {
        let agent_clone = self.clone();
        let state_clone = state.clone();
        let handle = tokio::spawn(async move {
            agent_clone
                .scan_and_promote_rules(&state_clone.session_id)
                .await;
            if let Err(e) = agent_clone.learn_skills_from_session(&state_clone).await {
                tracing::warn!(error = %e, "技能学习后台任务失败");
            }
        });
        self.background_tasks.lock().await.push(handle);
    }

    Ok(response)
}
```

- [ ] **Step 3: Rewrite process_message_stream() and handle_clarification_response() similarly**

For `process_message_stream()` (starts around line 474), apply the same pattern: use `ContextAssembler::assemble()` instead of injecting system prompt. For `handle_clarification_response()` (starts around line 211), apply the same pattern.

- [ ] **Step 4: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 5: Commit**

```bash
git add core/src/agent/coordinator.rs
git commit -m "refactor: coordinator uses ContextAssembler instead of injecting system prompt"
```

---

### Task 7: Update Session types and PersistentSessionManager

**Files:**
- Modify: `core/src/session/types.rs`
- Modify: `core/src/session/manager.rs`

- [ ] **Step 1: Update Session to use StructuredMessage**

In `core/src/session/types.rs`, change imports:

```rust
use crate::common::types::{Message, MessageRole, StructuredMessage, TianyanUri};
```

Change `Session.messages` field:

```rust
/// 会话中的消息。
pub messages: Vec<StructuredMessage>,
```

Update `add_message()`, `add_user_message()`, `add_assistant_message()`, `add_system_message()` methods — they need to accept `StructuredMessage` or be removed.

Since these methods are used in tests and by `PersistentSessionManager`, let's simplify:

```rust
/// 向会话添加结构化消息。
pub fn add_structured_message(&mut self, msg: StructuredMessage) {
    self.messages.push(msg);
}
```

Remove `add_message()`, `add_user_message()`, `add_assistant_message()`, `add_system_message()`.

Remove the `MessageRecord` struct entirely (lines 15-60).

- [ ] **Step 2: Update PersistentSessionManager**

In `core/src/session/manager.rs`, change imports:

```rust
use crate::common::types::{Message, StructuredMessage, TianyanUri};
```

Remove `MessageRecord` import. Change `append_message_to_vfs()`:

```rust
async fn append_message_to_vfs(&self, uri: &TianyanUri, msg: &StructuredMessage) -> Result<()> {
    let json_line = serde_json::to_string(msg)
        .map_err(|e| TianyanError::Serialization(e.to_string()))?;
    let jsonl_line = format!("{}\n", json_line);
    self.vfs.append_content(uri, &jsonl_line).await?;
    Ok(())
}
```

Change `load_session_from_vfs()` — deserialize `StructuredMessage` instead of `MessageRecord`:

```rust
for line in content.lines() {
    if line.trim().is_empty() {
        continue;
    }
    match serde_json::from_str::<StructuredMessage>(line) {
        Ok(msg) => {
            session.add_structured_message(msg);
        }
        Err(e) => {
            tracing::warn!("解析消息记录失败：{} - {}", uri, e);
        }
    }
}
```

Update `create_session()` to use `StructuredMessage`:

```rust
async fn create_session(&self, id: &str, message: Message) -> Result<Session> {
    let sm = StructuredMessage {
        id: format!("msg_{}", Utc::now().timestamp_millis()),
        parent_id: None,
        role: message.role,
        parts: vec![Part::Text {
            text: message.content.clone(),
            time: PartTime::default(),
        }],
        tokens: DetailedTokenUsage::default(),
        cost: 0.0,
        model_id: None,
        time: MessageTime::default(),
        session_id: id.to_string(),
        finish: None,
    };
    let mut session = Session::new(id);
    session.add_structured_message(sm.clone());
    // ... rest of method using `sm` instead of `message`
}
```

Update `add_message()` in SessionManager trait impl similarly.

- [ ] **Step 3: Update tests in session/types.rs**

Since we changed `Session` methods, update tests. Replace `add_user_message()` / `add_assistant_message()` with `add_structured_message()`:

```rust
#[test]
fn test_session_messages() {
    let mut session = Session::new("test-session");
    session.add_structured_message(StructuredMessage {
        id: "msg_1".to_string(),
        parent_id: None,
        role: MessageRole::User,
        parts: vec![Part::Text {
            text: "你好".to_string(),
            time: PartTime::default(),
        }],
        tokens: DetailedTokenUsage::default(),
        cost: 0.0,
        model_id: None,
        time: MessageTime::default(),
        session_id: "test-session".to_string(),
        finish: None,
    });
    assert_eq!(session.message_count(), 1);
}
```

Remove `test_message_record()` test since `MessageRecord` is removed.

- [ ] **Step 4: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 5: Commit**

```bash
git add core/src/session/types.rs core/src/session/manager.rs
git commit -m "refactor: Session uses StructuredMessage, remove MessageRecord"
```

---

### Task 8: Update agent/loop.rs for reasoning_content passthrough

**Files:**
- Modify: `core/src/agent/loop.rs`

- [ ] **Step 1: Add reasoning_content to assistant message construction**

In the `run()` method around line 109-115, where the assistant message is pushed to conversation:

```rust
// Add assistant message to history
messages.push(Message {
    role: MessageRole::Assistant,
    content: assistant_msg.content.clone(),
    tool_calls: assistant_msg.tool_calls.clone(),
    tool_call_id: None,
    reasoning_content: assistant_msg.reasoning_content.clone(),
});
```

- [ ] **Step 2: Run cargo check**

Run: `cargo check -p tianyan-core`

- [ ] **Step 3: Commit**

```bash
git add core/src/agent/loop.rs
git commit -m "feat: passthrough reasoning_content in agent loop"
```

---

### Task 9: Fix compilation across workspace

- [ ] **Step 1: Run full cargo check**

Run: `cargo check --workspace`

- [ ] **Step 2: Fix any compilation errors**

Fix issues in:
- `server/src/` if it references `ContextPipeline.run()` returning `ContextWindow` — update to destructure `(InjectableContext, Option<String>)`
- `server/src/` if it calls `PersistentSessionManager` methods that changed signatures
- `core/src/model/provider/chat.rs` — verify `convert_messages()` passes `reasoning_content` through to `async-openai` types. If `async-openai` does not natively support `reasoning_content`, the field may need explicit handling in the request body serialization
- Any test files that reference removed methods like `add_assistant_message()`, `add_message()`, `get_conversation()`, `build_prompt_context()`

Check `core/src/agent/session_state.rs` tests — update to use `structured_messages` and `StructuredMessage` instead of `conversation`.

Update session_state tests:

```rust
#[test]
fn test_session_state_new() {
    let state = SessionState::new("test-session");
    assert_eq!(state.session_id, "test-session");
    assert!(state.structured_messages.is_empty());
    assert!(state.current_goal.is_none());
}

#[test]
fn test_session_state_add_messages() {
    let mut state = SessionState::new("test-session");
    state.add_user_message("Hello");
    assert_eq!(state.structured_messages.len(), 1);
}
```

Remove `test_session_state_build_prompt_context()` test (method removed).

- [ ] **Step 3: Run clippy on changed files**

Run: `cargo clippy -p tianyan-core`

- [ ] **Step 4: Run fmt**

Run: `cargo fmt --all`

- [ ] **Step 5: Run unit tests**

Run: `cargo test -p tianyan-core --lib -- --nocapture`

- [ ] **Step 6: Commit all fixes**

```bash
git add -u
git commit -m "fix: compilation and test fixes for structured session context"
```

---

### Task 10: Write unit tests for ContextAssembler

**Files:**
- Modify: `core/src/context/assembler.rs` (add `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write assembler tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session_state::InjectableContext;
    use crate::common::types::{
        DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
    };

    fn make_text_msg(id: &str, role: MessageRole, text: &str, session_id: &str) -> StructuredMessage {
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts: vec![Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: session_id.to_string(),
            finish: None,
        }
    }

    #[test]
    fn test_assemble_empty_session() {
        let injectable = InjectableContext {
            soul: "You are a helpful assistant.".to_string(),
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable, "Hello");
        assert_eq!(messages.len(), 2); // soul + current input
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "You are a helpful assistant.");
        assert_eq!(messages[1].role, MessageRole::User);
        assert_eq!(messages[1].content, "Hello");
    }

    #[test]
    fn test_assemble_with_history() {
        let injectable = InjectableContext::default();
        let sm = make_text_msg("msg_1", MessageRole::User, "Hi", "ses_1");
        let messages = ContextAssembler::assemble(&[sm], &injectable, "Hello again");
        // 1 user history + 1 current input (no injectable, so no system messages)
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].content, "Hi");
        assert_eq!(messages[1].content, "Hello again");
    }

    #[test]
    fn test_structured_to_messages_reasoning_with_tool_call() {
        let sm = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Reasoning {
                    text: "I need to read the file first".to_string(),
                    time: PartTime::default(),
                },
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"foo.rs"}"#.to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Assistant);
        assert!(msg.tool_calls.is_some());
        assert!(msg.reasoning_content.is_some());
        assert_eq!(
            msg.reasoning_content.as_ref().unwrap(),
            "I need to read the file first"
        );
    }

    #[test]
    fn test_structured_to_messages_reasoning_without_tool_call_is_discarded() {
        let sm = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Reasoning {
                    text: "The answer is 42".to_string(),
                    time: PartTime::default(),
                },
                Part::Text {
                    text: "42".to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "42");
        assert!(msg.reasoning_content.is_none());
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn test_message_to_structured_roundtrip() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        let sm = ContextAssembler::message_to_structured(&msg, MessageRole::Assistant, "ses_1", None);
        assert_eq!(sm.role, MessageRole::Assistant);
        assert_eq!(sm.session_id, "ses_1");
        assert!(sm.parts.iter().any(|p| matches!(p, Part::Text { .. })));
    }

    #[test]
    fn test_assemble_with_rules_and_memories() {
        let injectable = InjectableContext {
            soul: "You are helpful.".to_string(),
            rules_and_experiences: vec!["Always read before writing.".to_string()],
            memories: vec!["User prefers Result style.".to_string()],
            ..Default::default()
        };
        let messages = ContextAssembler::assemble(&[], &injectable, "Test");
        // soul + (rules+memories merged) + current input
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.contains("helpful"));
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Always read"));
        assert!(messages[1].content.contains("Result style"));
    }

    #[test]
    fn test_structured_to_messages_tool_result() {
        let sm = StructuredMessage {
            id: "msg_tool".to_string(),
            parent_id: None,
            role: MessageRole::Tool,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: r#"{"result":"ok"}"#.to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "ses_1".to_string(),
            finish: None,
        };
        let messages = ContextAssembler::structured_to_messages(&sm);
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_ref().unwrap(), "call_1");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p tianyan-core context::assembler -- --nocapture`

- [ ] **Step 3: Commit**

```bash
git add core/src/context/assembler.rs
git commit -m "test: add unit tests for ContextAssembler"
```

---

### Task 11: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p tianyan-core --lib -- --nocapture`

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p tianyan-core`

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --all -- --check`

- [ ] **Step 4: Commit if any clippy/fmt fixes**

```bash
git commit -am "chore: clippy and fmt fixes"
```
