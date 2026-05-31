# Compression Marker & Session History Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Agent the sole owner of session state — it loads, executes, persists all messages (including tool calls), and manages compression with persisted markers that survive restarts.

**Architecture:** Agent gains `Arc<dyn SessionManager>` dependency. `process_message` signature changes from `(state, msg)` to `(session_id, msg)`. AgentLoop persists each message in real-time during the loop. Compression produces a `StructuredMessage` with `compression_marker=true` and persists it. Server simplifies to thin routing.

**Tech Stack:** Rust, tokio, serde (JSONL), VFS

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `core/src/common/types/structured_message.rs` | Data model | Add `compression_marker` field |
| `core/src/session/manager.rs` | Session persistence | Add `add_structured_message()`, marker scan on load |
| `core/src/agent/loop.rs` | LLM iteration | Inject `SessionManager`, real-time persist each message |
| `core/src/agent/coordinator.rs` | Agent orchestration | New `(session_id, msg)` signature, self-contained flow |
| `core/src/agent/builder.rs` | Wiring | Add `with_session_manager()` |
| `core/src/context/pipeline.rs` | Context pipeline | Compression returns `StructuredMessage` |
| `server/src/api/chat/services.rs` | HTTP service | Remove `bootstrap_session`, simplify to `agent.process_message(session_id, content)` |
| `server/src/agent_builder.rs` | Wizard stub | Signature sync |

---

### Task 1: Add `compression_marker` to StructuredMessage

**Files:**
- Modify: `core/src/common/types/structured_message.rs`

- [ ] **Step 1: Add field**

Add the field after `finish`:

```rust
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<String>,
    /// 标记该消息为压缩分界点。true 表示该消息是压缩产生的摘要 System Message。
    #[serde(default)]
    pub compression_marker: bool,
}
```

- [ ] **Step 2: Update all StructuredMessage construction sites**

Search for `StructuredMessage {` in the codebase. Each construction needs `compression_marker: false` added. Key sites:

In `core/src/session/manager.rs` (create_session and add_message):
```rust
    finish: None,
    compression_marker: false,   // add this
```

In `core/src/context/assembler.rs` (message_to_structured):
```rust
    finish: None,
    compression_marker: false,   // add this
```

In `core/src/session/manager.rs` (add_message):
```rust
    finish: None,
    compression_marker: false,   // add this
```

In `core/src/agent/coordinator.rs` (all message_to_structured call sites - find by searching `message_to_structured`).

Run: `cargo check --workspace` to find any missed sites via compiler errors.

- [ ] **Step 3: Update tests**

Search `StructuredMessage {` in test code (`core/src/session/types.rs:167`, etc.) and add `compression_marker: false`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: add compression_marker field to StructuredMessage"
```

---

### Task 2: Add `add_structured_message` to SessionManager + Remove `preserve_recent_messages` Logic

**Files:**
- Modify: `core/src/session/manager.rs`

- [ ] **Step 1: Add trait method**

In the `SessionManager` trait, after `add_message`:

```rust
    /// 直接持久化 StructuredMessage（不经过 Message 转换）。
    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()>;

    /// 向会话添加消息（传输格式，内部转为 StructuredMessage）。
    /// 已废弃，新代码应使用 add_structured_message。
    async fn add_message(&self, session_id: &str, message: Message) -> Result<()>;
```

- [ ] **Step 2: Implement in PersistentSessionManager**

Extract the VFS append logic from `add_message` into a shared helper (already exists as `append_message_to_vfs`):

```rust
    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()> {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id))
            .map_err(|e| TianyanError::MemorySystem(format!("无效的 session URI: {}", e)))?;

        // Verify session exists
        if self.vfs.read_content(&uri, crate::ContentLevel::Detail).await.is_err() {
            return Err(TianyanError::MemorySystem(format!(
                "会话未找到：{}",
                session_id
            )));
        }

        self.append_message_to_vfs(&uri, &msg).await?;
        Ok(())
    }
```

- [ ] **Step 3: Add marker scan to load_session_from_vfs**

After the JSONL parsing loop (after `session.messages.len() > MAX_SESSION_MESSAGES` check), add marker-based truncation:

```rust
        // 从后向前扫描 compression_marker，只保留 marker 及之后的消息
        if let Some(marker_pos) = session.messages.iter().rposition(|m| m.compression_marker) {
            let skipped = marker_pos;
            if skipped > 0 {
                session.messages = session.messages.split_off(skipped);
                tracing::info!(
                    skipped_messages = skipped,
                    session_id = %id,
                    "按 compression_marker 截断会话至工作集"
                );
            }
        }
```

This replaces the existing `MAX_SESSION_MESSAGES` truncation — the marker-based logic takes precedence. Keep MAX_SESSION_MESSAGES as a safety limit *after* marker truncation.

- [ ] **Step 4: Remove `preserve_recent_messages` split logic from ContextCompressor**

In `core/src/context/compression/mod.rs`, `compress()` method (line 197):
- Remove the early return `if messages.len() <= self.config.preserve_recent_messages`
- Remove `split_point`, `early_messages`, `recent_messages` variables
- `compress()` now takes ALL messages passed to it and summarizes them in full
- The caller (`prepare_context`) is responsible for only passing messages after the last marker

Updated `compress()`:
```rust
    pub async fn compress(&mut self, messages: &[Message]) -> Result<CompressionResult> {
        if messages.is_empty() {
            return Ok(CompressionResult { messages: vec![], compressed_count: 0, summary: String::new(), ... });
        }

        let original_tokens = self.estimator.estimate_messages(messages);

        let (compressed_early, summary) = match self.config.strategy {
            CompressionStrategy::Summarize => {
                self.compress_by_summarization(messages).await?
            }
            CompressionStrategy::Select => self.compress_by_selection(messages).await?,
            CompressionStrategy::Hybrid => self.compress_hybrid(messages).await?,
        };

        let compressed_tokens = self.estimator.estimate_messages(&compressed_early);

        Ok(CompressionResult {
            messages: compressed_early,
            compressed_count: messages.len(),
            summary,
            original_tokens,
            compressed_tokens,
        })
    }
```

- [ ] **Step 5: Commit**

```bash
git add core/src/session/manager.rs core/src/context/compression/mod.rs
git commit -m "feat: add add_structured_message to SessionManager; marker scan on load; simplify compress"
```

---

### Task 3: Inject SessionManager into AgentLoop, persist in real-time

**Files:**
- Modify: `core/src/agent/loop.rs`

- [ ] **Step 1: Add imports**

```rust
use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::types::{Message, MessageRole};
use crate::context::ContextAssembler;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use crate::session::SessionManager;                              // new
use crate::common::error::Result as TianyanResult;                // new
```

- [ ] **Step 2: Add session_manager field and update new()**

```rust
pub struct AgentLoop {
    model_service: Arc<dyn ChatService>,
    tool_registry: ToolRegistry,
    session_manager: Arc<dyn SessionManager>,  // new
    config: AgentLoopConfig,
}

impl AgentLoop {
    pub fn new(
        model_service: Arc<dyn ChatService>,
        tool_registry: ToolRegistry,
        session_manager: Arc<dyn SessionManager>,  // new parameter
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            model_service,
            tool_registry,
            session_manager,    // new
            config,
        }
    }
```

- [ ] **Step 3: Update run() signature to accept session_id and parent_id**

```rust
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
        session_id: &str,               // new
        parent_id: Option<&str>,         // new
    ) -> Result<AgentLoopResult, AgentLoopError> {
```

- [ ] **Step 4: Add persistence after each message push**

After line 110 (`messages.push(Message { ... });` — the assistant message push), add:

```rust
            // Persist assistant message
            let sm = ContextAssembler::message_to_structured(
                &Message {
                    role: MessageRole::Assistant,
                    content: assistant_msg.content.clone(),
                    tool_calls: assistant_msg.tool_calls.clone(),
                    tool_call_id: None,
                    reasoning_content: assistant_msg.reasoning_content.clone(),
                },
                session_id,
                &current_parent_id,
            );
            if let Err(e) = self.session_manager.add_structured_message(session_id, sm).await {
                tracing::warn!(error = %e, "持久化 assistant 消息失败");
            }
```

After line 135 (`messages.push(Message::tool(...))` — the tool result push), add:

```rust
                    // Persist tool result
                    let tool_sm = ContextAssembler::message_to_structured(
                        &Message::tool(&call_id, &content),
                        session_id,
                        &current_parent_id,
                    );
                    if let Err(e) = self.session_manager.add_structured_message(session_id, tool_sm).await {
                        tracing::warn!(error = %e, "持久化 tool result 消息失败");
                    }
```

Also maintain a `current_parent_id` variable: initialize with the passed `parent_id` param, update it with each new message's id after persist.

- [ ] **Step 5: Also persist the final answer (no tool calls path)**

After `return Ok(AgentLoopResult::Answer(assistant_msg.content));` (line 142), add persistence before the return:

```rust
            } else {
                let sm = ContextAssembler::message_to_structured(
                    &Message::assistant(&assistant_msg.content),
                    session_id,
                    &current_parent_id,
                );
                if let Err(e) = self.session_manager.add_structured_message(session_id, sm).await {
                    tracing::warn!(error = %e, "持久化 assistant answer 失败");
                }
                return Ok(AgentLoopResult::Answer(assistant_msg.content));
            }
```

- [ ] **Step 6: Commit**

```bash
git add core/src/agent/loop.rs
git commit -m "feat: AgentLoop persists messages in real-time during loop via SessionManager"
```

---

### Task 4: Refactor Agent::process_message to self-contained flow

**Files:**
- Modify: `core/src/agent/coordinator.rs`

- [ ] **Step 1: Update imports**

```rust
use crate::session::SessionManager;    // new
use crate::session::Session;           // new
```

Remove `use crate::agent::session_state::SessionState;` (it's already imported implicitly via submodule, but keep explicit if needed).

- [ ] **Step 2: Add session_manager to Agent struct**

```rust
pub struct Agent {
    config: AgentConfig,
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    context_pipeline: ContextPipeline,
    harness: AgentHarness,
    skills: AgentSkills,
    session_manager: Arc<dyn SessionManager>,  // new
    state: Arc<RwLock<AgentState>>,
    verification_gate: VerificationGate,
    llm_judge: Option<LlmJudge>,
    agent_loop: AgentLoop,
}
```

- [ ] **Step 3: Update Agent::new() signature**

Add `session_manager: Arc<dyn SessionManager>` parameter:

```rust
    pub fn new(
        config: AgentConfig,
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        context_pipeline: ContextPipeline,
        harness: AgentHarness,
        skills: AgentSkills,
        session_manager: Arc<dyn SessionManager>,  // new
        verification_gate: VerificationGate,
        llm_judge: Option<LlmJudge>,
        agent_loop: AgentLoop,
    ) -> Self {
        Self {
            // ... existing fields ...
            session_manager,   // new
            // ...
        }
    }
```

- [ ] **Step 4: Update Clone impl**

```rust
            session_manager: self.session_manager.clone(),
```

- [ ] **Step 5: Update AgentCoordinator trait method signatures**

```rust
    async fn process_message(
        &self,
        session_id: &str,       // was: state: Arc<RwLock<SessionState>>
        message: &str,
    ) -> Result<AgentResponse>;

    async fn process_message_stream(
        &self,
        session_id: &str,       // was: state: Arc<RwLock<SessionState>>
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    async fn handle_clarification(
        &self,
        session_id: &str,       // was: state: Arc<RwLock<SessionState>>
        answers: &str,
    ) -> Result<AgentResponse>;
```

- [ ] **Step 6: Rewrite process_message implementation**

```rust
    async fn process_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();
        let message = message.to_string();

        // 1. Load session from persistence
        let session = self.session_manager.get_session(session_id).await?
            .unwrap_or_else(|| Session::new(session_id));

        // 2. Build SessionState from loaded messages
        let state = Arc::new(RwLock::new(SessionState::new(session_id)));
        {
            let mut s = state.write().await;
            for sm in &session.messages {
                s.add_structured_message(sm.clone());
            }
        }

        // 3. Persist current user message
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let user_sm = ContextAssembler::message_to_structured(
            &Message::user(&message),
            session_id,
            parent_id.as_deref(),
        );
        self.session_manager.add_structured_message(session_id, user_sm).await?;

        // 4. Prepare context
        let messages = self.prepare_context(&state, &message).await;

        // 5. Run agent loop (internal persistence)
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let mut messages = messages;  // was already owned, just re-bind
        let loop_result = self.agent_loop.run(
            &mut messages,
            None,
            session_id,
            parent_id.as_deref(),
        ).await;

        // 6. Handle loop result (no more persistence needed — loop did it)
        let response = match loop_result {
            Ok(AgentLoopResult::Answer(content)) => {
                // state update (in-memory only, persistence done in loop)
                {
                    let parent_id = {
                        let s = state.read().await;
                        s.structured_messages.last().map(|m| m.id.clone())
                    };
                    let sm = ContextAssembler::message_to_structured(
                        &Message::assistant(&content),
                        session_id,
                        parent_id.as_deref(),
                    );
                    state.write().await.add_structured_message(sm);
                }

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
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
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

        // 7. Compression: check if needed, produce StructuredMessage with marker
        self.maybe_compress_and_persist(&state, session_id).await;

        // 8. Update agent metrics
        {
            let mut st = self.state.write().await;
            st.conversations_processed += 1;
        }

        Ok(response)
    }
```

- [ ] **Step 7: Add maybe_compress_and_persist helper method**

```rust
    /// 判断是否需要压缩，如果需要则生成摘要 StructuredMessage 并持久化。
    async fn maybe_compress_and_persist(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) {
        let (needs_compress, messages_since_marker) = {
            let s = state.read().await;
            // Find last compression marker position
            let marker_pos = s.structured_messages.iter()
                .rposition(|m| m.compression_marker);
            let start_idx = marker_pos.unwrap_or(0);
            let msgs: Vec<StructuredMessage> = s.structured_messages[start_idx..].to_vec();
            let need = msgs.len() >= 6; // threshold: at least 6 messages since last marker
            (need, msgs)
        };

        if !needs_compress {
            return;
        }

        // Build Vec<Message> from messages since marker
        let conversation: Vec<Message> = messages_since_marker
            .iter()
            .flat_map(|sm| ContextAssembler::structured_to_messages(sm))
            .collect();

        // Run compressor
        let mut compressor = self.context_pipeline.get_compressor();
        if !compressor.should_compress(&conversation) {
            return;
        }

        match compressor.compress(&conversation).await {
            Ok(result) if !result.summary.is_empty() => {
                let summary_sm = StructuredMessage {
                    id: format!("cmp_{}", chrono::Utc::now().timestamp_millis()),
                    parent_id: None,
                    role: MessageRole::System,
                    parts: vec![Part::Text {
                        text: format!("[对话摘要] 以下是对历史对话的摘要：\n{}\n[摘要结束]", result.summary),
                        time: PartTime::default(),
                    }],
                    tokens: DetailedTokenUsage::default(),
                    cost: 0.0,
                    model_id: None,
                    time: MessageTime {
                        created: chrono::Utc::now().timestamp_millis(),
                        completed: chrono::Utc::now().timestamp_millis(),
                    },
                    session_id: session_id.to_string(),
                    finish: None,
                    compression_marker: true,
                };

                if let Err(e) = self.session_manager
                    .add_structured_message(session_id, summary_sm.clone())
                    .await
                {
                    tracing::warn!(error = %e, "持久化压缩摘要失败");
                } else {
                    state.write().await.add_structured_message(summary_sm);
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "压缩执行失败");
            }
        }
    }
```

Note: This requires adding a `get_compressor()` method to `ContextPipeline` that returns a clone of the ContextCompressor from inside the `Arc<TokioMutex<>>`:

In `core/src/context/pipeline.rs`:
```rust
    pub fn get_compressor(&self) -> ContextCompressor {
        // Must be inside a tokio context to call .lock().await
        // Since we need this synchronously, we'll restructure...
    }
```

Actually, `tokio::sync::Mutex::lock()` is async. We can't call it synchronously. Better approach: add a method `compress_if_needed` directly on `ContextPipeline` that takes the messages and returns `Option<StructuredMessage>`:

In `core/src/context/pipeline.rs`:
```rust
    /// 检查并执行压缩，返回带 compression_marker 的摘要 StructuredMessage（如果需要）。
    pub async fn compress_if_needed_for(
        &self,
        messages: &[Message],
        session_id: &str,
    ) -> Option<StructuredMessage> {
        let mut conversation = messages.to_vec();
        let summary = self.compress_if_needed(&mut conversation).await.ok()??;
        // summary is Option<String>, unwrap the outer Result then the inner Option
        // Actually compress_if_needed returns Result<Option<String>>
        // Let me reconsider...
    }
```

Let me simplify. `compress_if_needed` already returns `Result<Option<String>>`. We can call it directly and build the `StructuredMessage`:

```rust
    /// 压缩后生成带 compression_marker 的 StructuredMessage。
    pub async fn compress_for_session(
        &self,
        messages: &[Message],
        session_id: &str,
    ) -> Option<StructuredMessage> {
        let mut conversation = messages.to_vec();
        let summary = self.compress_if_needed(&mut conversation).await.ok()??;
        
        if summary.is_empty() {
            return None;
        }

        Some(StructuredMessage {
            id: format!("cmp_{}", chrono::Utc::now().timestamp_millis()),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text: format!("[对话摘要] 以下是对历史对话的摘要：\n{}\n[摘要结束]", summary),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: chrono::Utc::now().timestamp_millis(),
                completed: chrono::Utc::now().timestamp_millis(),
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: true,
        })
    }
```

Must add `use chrono;` and `use crate::common::types::{Part, PartTime, MessageTime, DetailedTokenUsage, MessageRole};` to pipeline.rs imports.

- [ ] **Step 8: Update prepare_context — remove inline compression logic**

`prepare_context` no longer handles compression. Remove the compression block (the `{ let s = state.read().await; let mut conversation... }` block). `prepare_context` now only:
1. add_user_message
2. load injectable (with cache)
3. assemble

- [ ] **Step 9: Commit**

```bash
git add core/src/agent/coordinator.rs core/src/context/pipeline.rs
git commit -m "feat: refactor Agent.process_message to self-contained session flow with compression"
```

---

### Task 5: Update AgentBuilder to wire SessionManager

**Files:**
- Modify: `core/src/agent/builder.rs`

- [ ] **Step 1: Add field and setter**

```rust
    session_manager: Option<Arc<dyn SessionManager>>,   // new field

    pub fn with_session_manager(mut self, sm: Arc<dyn SessionManager>) -> Self {
        self.session_manager = Some(sm);
        self
    }
```

- [ ] **Step 2: Add import**

```rust
use crate::session::SessionManager;
```

- [ ] **Step 3: Wire in build()**

After `let agent_loop = AgentLoop::new(...)`:

```rust
        let session_manager = self
            .session_manager
            .ok_or_else(|| TianyanError::Internal("需要 SessionManager".to_string()))?;

        let agent_loop = AgentLoop::new(
            model_service.clone(),
            tool_registry,
            session_manager.clone(),   // new parameter
            AgentLoopConfig {
                max_turns: self.config.max_turns,
                model: "default".to_string(),
            },
        );
```

And add `session_manager` to the `Agent::new(...)` call:

```rust
        Ok(Agent::new(
            self.config,
            model_service,
            vfs,
            context_pipeline,
            harness,
            skills,
            session_manager,        // new
            verification_gate,
            llm_judge,
            agent_loop,
        ))
```

- [ ] **Step 4: Commit**

```bash
git add core/src/agent/builder.rs
git commit -m "feat: wire SessionManager through AgentBuilder to Agent and AgentLoop"
```

---

### Task 6: Update server ChatService

**Files:**
- Modify: `server/src/api/chat/services.rs`

- [ ] **Step 1: Simplify process_message**

Remove the `bootstrap_session` call. The method becomes:

```rust
    pub async fn process_message(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let last_message = request.messages.last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let response = self.agent.process_message(session_id, &last_message).await?;

        // Note: no more manual add_message — Agent persists internally
        // Note: no more bootstrap_session — Agent loads internally

        let chat_response = ChatResponse {
            id: format!("chatcmpl-{}", short_uuid()),
            session_id: session_id.clone(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response.content,
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            usage: TokenUsage {
                prompt_tokens: response.token_usage.prompt_tokens as u32,
                completion_tokens: response.token_usage.completion_tokens as u32,
                total_tokens: response.token_usage.total_tokens as u32,
            },
        };

        Ok(chat_response)
    }
```

- [ ] **Step 2: Simplify process_message_stream**

```rust
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> anyhow::Result<()> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let last_message = request.messages.last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let mut stream = self.agent.process_message_stream(session_id, &last_message).await?;

        // ... stream consumption unchanged ...
    }
```

Remove all `self.session_manager.add_message()` calls in the stream handler — Agent persists internally.

- [ ] **Step 3: Remove bootstrap_session method entirely**

Delete the ~30-line `bootstrap_session` method.

- [ ] **Step 4: Remove unused imports**

Remove `use tianyan::agent::SessionState;` if no longer needed. Remove `use tokio::sync::RwLock;` if no longer needed.

- [ ] **Step 5: Handle regenerate_message and edit_message**

These methods currently construct `ChatRequest` with history and call `self.process_message(chat_request)`. With the new signature, `process_message` takes `session_id`. The `regenerate_message` and `edit_message` methods need updating:

For `regenerate_message`: instead of building full history and calling `process_message(chat_request)`, call `process_message` directly with just the session_id. The Agent will load existing session history.

Actually, regenerate and edit need special handling — they modify the session before calling. This is beyond the scope of this task. For now, mark these as `// TODO: update for new agent signature` and keep them compiling. They will need a separate follow-up.

- [ ] **Step 6: Commit**

```bash
git add server/src/api/chat/services.rs
git commit -m "refactor: simplify ChatService — remove bootstrap_session, agent handles persistence"
```

---

### Task 7: Update WizardModeAgent stub

**Files:**
- Modify: `server/src/agent_builder.rs`

- [ ] **Step 1: Sync trait signatures**

```rust
    async fn process_message(
        &self,
        _session_id: &str,      // was: _state: Arc<RwLock<SessionState>>
        _message: &str,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::ModelService(...))
    }

    async fn process_message_stream(
        &self,
        _session_id: &str,
        _message: &str,
    ) -> TianyanResult<mpsc::Receiver<TianyanResult<AgentStreamChunk>>> {
        Err(TianyanError::ModelService(...))
    }

    async fn handle_clarification(
        &self,
        _session_id: &str,
        _answers: &str,
    ) -> TianyanResult<AgentResponse> {
        Err(TianyanError::ModelService(...))
    }
```

- [ ] **Step 2: Commit**

```bash
git add server/src/agent_builder.rs
git commit -m "fix: sync WizardModeAgent stubs with new AgentCoordinator signature"
```

---

### Task 8: Build verification

- [ ] **Step 1: cargo check --workspace**

```bash
cargo check --workspace
```

Expected: compilation passes. Fix any remaining compiler errors (missing `compression_marker: false` in construction sites, missing imports, wrong types).

- [ ] **Step 2: cargo test --workspace**

```bash
cargo test --workspace --lib
```

Expected: all tests pass. Update any tests that instantiate `StructuredMessage` without the new field, `AgentLoop::new` without `session_manager`, etc.

- [ ] **Step 3: Commit**

```bash
git commit -m "fix: compilation and test fixes for compression marker refactor"
```
