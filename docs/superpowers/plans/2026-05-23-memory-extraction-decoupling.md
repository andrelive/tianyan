# Memory Extraction Decoupling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple long-term memory extraction from Agent's conversation loop and from the storage module; replace inline per-turnspawn extraction with scheduler-driven periodic scanning using VFS-based state tracking.

**Architecture:** Create a new `memory` module with a pure `MemoryExtractor` (text in, `Vec<MemoryEntry>` out, depends only on `ChatService`). Rewrite `tasks/memory_task.rs` to use VFS-based `_metadata.json` for idempotent session tracking instead of in-memory `HashSet`. Remove `MemoryExtractionService` from `storage/`, `Agent`, and `SummaryService`. Update `TaskContext` to hold `MemoryExtractor` instead.

**Tech Stack:** Rust, async-trait, tokio, serde_json, tianyan-core storage/agent/scheduler/tasks modules

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| **Create** | `core/src/memory/mod.rs` | Module entry, re-export `MemoryExtractor`, `ExtractionConfig`, `DEFAULT_EXTRACTION_PROMPT` |
| **Create** | `core/src/memory/extractor.rs` | Pure extraction: `text → Vec<MemoryEntry>` via `ChatService`; no VFS dependency |
| Delete | `core/src/storage/extractor.rs` | Moved to `memory/extractor.rs` |
| Modify | `core/src/lib.rs` | Add `pub mod memory;` |
| Modify | `core/src/storage/mod.rs` | Remove `mod extractor;` and its re-exports |
| Modify | `core/src/storage/summary/service.rs` | Remove `memory_extractor` field, `with_memory_extractor()`, `with_memory_extractor_from_model()` |
| Modify | `core/src/tasks/memory_task.rs` | Rewrite to use `MemoryExtractor` + VFS `_metadata.json` tracking |
| Modify | `core/src/agent/coordinator.rs` | Remove `memory_extractor` field, 3 spawn blocks, `extract_memories_from_session()` |
| Modify | `core/src/agent/builder.rs` | Remove `memory_extractor` field, `with_memory_extractor()`, construction in `build()` |
| Modify | `core/src/scheduler/task_scheduler.rs` | `TaskContext::memory_extractor`: `Arc<MemoryExtractionService>` → `Arc<MemoryExtractor>` |
| Modify | `server/src/state.rs` | `create_memory_extractor()` returns `Arc<MemoryExtractor>`; remove `SummaryService` extractor wiring |
| Modify | `server/src/lib.rs` | Update imports, `create_memory_extractor()` return type |
| Modify | `core/tests/structural.rs` | Add `test_memory_does_not_depend_on_storage` structural test |

---

### Task 1: Create `memory` module with `MemoryExtractor`

**Files:**
- Create: `core/src/memory/mod.rs`
- Create: `core/src/memory/extractor.rs`
- Modify: `core/src/lib.rs`

**Design note:** `MemoryExtractor` is stateless. Its single public method is `extract(conversation_text: &str) -> Result<Vec<MemoryEntry>>`. It depends only on `Arc<dyn ChatService>`. It does NOT depend on `VirtualFileSystem`. Persistence is the caller's responsibility.

- [ ] **Step 1: Create `core/src/memory/mod.rs`**

```rust
//! 长期记忆提取模块。
//!
//! 本模块提供从会话文本中提取结构化记忆的纯功能，
//! 将提取逻辑与调度、持久化解耦。

mod extractor;

pub use extractor::{ExtractionConfig, MemoryExtractor, DEFAULT_EXTRACTION_PROMPT};
```

- [ ] **Step 2: Create `core/src/memory/extractor.rs`**

Move the extraction logic from `core/src/storage/extractor.rs`, removing all VFS dependencies:

```rust
//! 记忆提取器 — 纯文本到结构化记忆的转换。
//!
//! 使用 LLM 分析对话文本，返回结构化 MemoryEntry 列表。
//! 不涉及任何持久化操作。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{MemoryCategory, MemoryEntry, Message};
use crate::model::ChatService;

/// 记忆提取配置。
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    /// 用于提取的模型名称。
    pub model: String,
    /// 提取提示词模板。
    pub prompt_template: String,
    /// 最小对话长度才触发提取（字符数）。
    pub min_conversation_length: usize,
    /// 提取的最大记忆数量。
    pub max_extracted_memories: usize,
    /// 最小重要性阈值（低于此值的记忆被过滤）。
    pub min_importance_threshold: f32,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            prompt_template: DEFAULT_EXTRACTION_PROMPT.to_string(),
            min_conversation_length: 50,
            max_extracted_memories: 10,
            min_importance_threshold: 0.3,
        }
    }
}

/// 增强的提取提示词。
///
/// 指导 LLM 从对话中提取结构化记忆，分类存储到不同命名空间。
pub const DEFAULT_EXTRACTION_PROMPT: &str = r#"分析以下对话，提取值得长期保存的结构化记忆。

对话内容：
{conversation}

请提取以下类别的记忆（仅提取重要、持久的信息）：

1. **preferences** - 用户偏好（工作习惯、格式偏好、沟通风格等）
2. **decisions** - 重要决策（用户做出的选择、确认的方案等）
3. **facts** - 事实信息（关于用户、项目、环境的事实）
4. **entities** - 实体信息（提到的人、项目、工具、组织等）
5. **patterns** - 模式（重复出现的工作模式、代码风格等）
6. **successful_cases** - 成功案例（有效的问题解决方法）
7. **failed_cases** - 失败教训（需要避免的错误）

请以 JSON 格式返回，结构如下：
{
  "memories": [
    {
      "id": "唯一标识符（使用小写英文和连字符）",
      "category": "类别名称（preference/decision/fact/entity/pattern/successful_case/failed_case）",
      "content": "记忆的简洁描述",
      "importance": 0.0-1.0,
      "tags": ["标签1", "标签2"]
    }
  ]
}

提取原则：
- 只提取跨会话有价值的信息
- 避免提取临时性、一次性的信息
- 重要性评分：0.9+ 非常关键，0.7-0.9 重要，0.4-0.7 一般，0.4以下不提取
- 每个记忆应该简洁明了，不超过100字
- 最多提取 {max_memories} 条记忆"#;

/// 记忆提取器。
///
/// 使用 LLM 将对话文本转换为结构化记忆。
/// 不持有 VFS 引用，不执行持久化 — 输入是文本，输出是结构化数据。
pub struct MemoryExtractor {
    model_service: Arc<dyn ChatService>,
    config: ExtractionConfig,
}

impl MemoryExtractor {
    /// 创建新的记忆提取器。
    pub fn new(model_service: Arc<dyn ChatService>, config: ExtractionConfig) -> Self {
        Self {
            model_service,
            config,
        }
    }

    /// 从对话文本中提取结构化记忆。
    ///
    /// 不执行持久化，仅返回提取结果。
    /// 调用方负责将结果写入 VFS。
    pub async fn extract(&self, conversation: &str) -> Result<Vec<MemoryEntry>> {
        if conversation.len() < self.config.min_conversation_length {
            return Ok(Vec::new());
        }

        let prompt = self
            .config
            .prompt_template
            .replace("{conversation}", conversation)
            .replace(
                "{max_memories}",
                &self.config.max_extracted_memories.to_string(),
            );

        let response = self
            .model_service
            .chat(&self.config.model, vec![Message::user(prompt)])
            .await?;

        let memories = self.parse_extraction_response(&response)?;

        let filtered: Vec<MemoryEntry> = memories
            .into_iter()
            .filter(|m| m.importance >= self.config.min_importance_threshold)
            .collect();

        tracing::info!(count = filtered.len(), "记忆提取完成");
        Ok(filtered)
    }

    fn parse_extraction_response(&self, response: &str) -> Result<Vec<MemoryEntry>> {
        let cleaned = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let json: serde_json::Value = match serde_json::from_str(cleaned) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, response = %response, "记忆提取响应 JSON 解析失败");
                return Ok(Vec::new());
            }
        };

        let mut memories = Vec::new();

        if let Some(memories_array) = json.get("memories").and_then(|v| v.as_array()) {
            for memory_json in memories_array
                .iter()
                .take(self.config.max_extracted_memories)
            {
                if let Some(entry) = self.parse_single_memory(memory_json) {
                    memories.push(entry);
                }
            }
        } else if let Ok(old_memories) = self.parse_legacy_format(&json) {
            memories.extend(old_memories);
        }

        Ok(memories)
    }

    fn parse_single_memory(&self, json: &serde_json::Value) -> Option<MemoryEntry> {
        let id = json.get("id")?.as_str()?;
        let category_str = json.get("category")?.as_str()?;
        let content = json.get("content")?.as_str()?;
        let importance = json.get("importance")?.as_f64()? as f32;

        let category = match category_str.to_lowercase().as_str() {
            "preference" | "preferences" => MemoryCategory::Preference,
            "decision" | "decisions" => MemoryCategory::Decision,
            "fact" | "facts" => MemoryCategory::Fact,
            "entity" | "entities" => MemoryCategory::Entity,
            "pattern" | "patterns" => MemoryCategory::Pattern,
            "successful_case" | "success" | "successful" => MemoryCategory::SuccessfulCase,
            "failed_case" | "failure" | "failed" => MemoryCategory::FailedCase,
            _ => MemoryCategory::Fact,
        };

        let mut entry = MemoryEntry::new(id, content, category);
        entry.importance = importance.clamp(0.0, 1.0);

        if let Some(tags) = json.get("tags").and_then(|v| v.as_array()) {
            entry.tags = tags
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect();
        }

        Some(entry)
    }

    fn parse_legacy_format(&self, json: &serde_json::Value) -> Result<Vec<MemoryEntry>> {
        let mut memories = Vec::new();

        for (category_str, category) in [
            ("preferences", MemoryCategory::Preference),
            ("decisions", MemoryCategory::Decision),
            ("facts", MemoryCategory::Fact),
        ] {
            if let Some(items) = json.get(category_str).and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    if let Some(content) = item.get("content").and_then(|v| v.as_str()) {
                        let id = item
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("{}-{}", category_str, i));
                        let mut entry = MemoryEntry::new(&id, content, category);
                        if let Some(confidence) = item.get("confidence").and_then(|v| v.as_f64()) {
                            entry.importance = (confidence as f32).clamp(0.0, 1.0);
                        }
                        memories.push(entry);
                    }
                }
            }
        }

        Ok(memories)
    }
}

/// 将记忆格式化为 Markdown。
///
/// 供外部调用方在写入 VFS 前使用。
pub fn format_memory_as_markdown(memory: &MemoryEntry) -> String {
    let mut md = String::new();

    md.push_str(&format!("# 记忆: {}\n\n", memory.id));
    md.push_str(&format!("- **类别**: {}\n", memory.category));
    md.push_str(&format!("- **重要性**: {:.2}\n", memory.importance));
    md.push_str(&format!("- **访问次数**: {}\n", memory.access_count));
    md.push_str(&format!(
        "- **创建时间**: {}\n",
        memory.created_at.to_rfc3339()
    ));
    md.push_str(&format!(
        "- **更新时间**: {}\n",
        memory.updated_at.to_rfc3339()
    ));

    if let Some(ref last_accessed) = memory.last_accessed {
        md.push_str(&format!("- **最后访问**: {}\n", last_accessed.to_rfc3339()));
    }

    if let Some(ref session_id) = memory.source_session {
        md.push_str(&format!("- **来源会话**: {}\n", session_id));
    }

    if !memory.tags.is_empty() {
        md.push_str(&format!("- **标签**: {}\n", memory.tags.join(", ")));
    }

    if !memory.related_memories.is_empty() {
        md.push_str(&format!(
            "- **相关记忆**: {}\n",
            memory.related_memories.join(", ")
        ));
    }

    md.push_str("\n## 内容\n\n");
    md.push_str(&memory.content);
    md.push('\n');

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_memory_as_markdown() {
        let memory = MemoryEntry::new("test-1", "这是一个测试记忆", MemoryCategory::Preference)
            .with_importance(0.8)
            .with_tags(vec!["test".to_string(), "preference".to_string()]);

        let md = format_memory_as_markdown(&memory);
        assert!(md.contains("# 记忆: test-1"));
        assert!(md.contains("**类别**: preference"));
        assert!(md.contains("**重要性**: 0.80"));
        assert!(md.contains("这是一个测试记忆"));
    }
}
```

- [ ] **Step 3: Add `pub mod memory;` to `core/src/lib.rs`**

In `core/src/lib.rs`, after line 42 (`pub mod model;`), add:

```rust
pub mod memory;
```

- [ ] **Step 4: Run build check**

```powershell
cargo check -p tianyan-core 2>&1
```

Expected: PASS (no errors from the new module, though unused-import warnings in other files are OK at this stage)

- [ ] **Step 5: Commit**

```bash
git add core/src/memory/mod.rs core/src/memory/extractor.rs core/src/lib.rs
git commit -m "feat: add memory module with pure MemoryExtractor"
```

---

### Task 2: Remove `extractor` from `storage` module

**Files:**
- Modify: `core/src/storage/mod.rs`
- Delete: `core/src/storage/extractor.rs`

- [ ] **Step 1: Remove `mod extractor;` and its re-exports from `core/src/storage/mod.rs`**

Delete line 32:
```
mod extractor;
```

Replace line 46 with only:
```rust
pub use backend::LocalStorageBackend;
```

(Remove `ExtractionConfig, MemoryExtractionService, DEFAULT_EXTRACTION_PROMPT` from the `pub use extractor::` line, keeping only what was from `backend::`)

- [ ] **Step 2: Delete `core/src/storage/extractor.rs`**

```powershell
Remove-Item -LiteralPath "core\src\storage\extractor.rs"
```

- [ ] **Step 3: Run build check to find all broken imports**

```powershell
cargo check --workspace 2>&1
```

Expected: FAIL — compilation errors in files that still import from `crate::storage::` or `tianyan::storage::` for `MemoryExtractionService`, `ExtractionConfig`, `DEFAULT_EXTRACTION_PROMPT`. This is expected; subsequent tasks will fix these.

- [ ] **Step 4: Commit**

```bash
git add -u core/src/storage/mod.rs
git rm core/src/storage/extractor.rs
git commit -m "refactor: remove extractor from storage module"
```

---

### Task 3: Update `TaskContext` in scheduler to use `MemoryExtractor`

**Files:**
- Modify: `core/src/scheduler/task_scheduler.rs`

- [ ] **Step 1: Update imports and field type in `core/src/scheduler/task_scheduler.rs`**

Replace line 12:
```rust
use crate::storage::{MemoryExtractionService, SummaryEngine, VirtualFileSystem};
```

With:
```rust
use crate::memory::MemoryExtractor;
use crate::storage::{SummaryEngine, VirtualFileSystem};
```

Replace line 68-69 (field declaration):
```rust
    /// 记忆提取器。
    pub memory_extractor: Arc<MemoryExtractionService>,
```

With:
```rust
    /// 记忆提取器。
    pub memory_extractor: Arc<MemoryExtractor>,
```

Replace line 79 (parameter):
```rust
        memory_extractor: Arc<MemoryExtractionService>,
```

With:
```rust
        memory_extractor: Arc<MemoryExtractor>,
```

- [ ] **Step 2: Run build check**

```powershell
cargo check -p tianyan-core 2>&1
```

Expected: Errors only in files not yet updated (tasks/memory_task.rs, agent/*, server/*). The scheduler module itself should compile.

- [ ] **Step 3: Commit**

```bash
git add core/src/scheduler/task_scheduler.rs
git commit -m "refactor: TaskContext uses MemoryExtractor instead of MemoryExtractionService"
```

---

### Task 4: Rewrite `MemoryTask` to use `MemoryExtractor` + VFS `_metadata.json` tracking

**Files:**
- Modify: `core/src/tasks/memory_task.rs`

**Design note:** The current `MemoryTask` tracks processed sessions in an in-memory `HashSet<RwLock<HashSet<String>>>` which is lost on restart. The new design stores `_metadata.json` alongside each session entry in VFS. On scan, read this file to get `last_extracted_message_count`, compare with current message count from session content, and extract only the delta.

`_metadata.json` format:
```json
{
  "last_extracted_message_count": 42,
  "last_extracted_at": "2026-05-23T10:30:00Z"
}
```

Location: `tianyan://session/{id}/_metadata` (child URI of session directory).

- [ ] **Step 1: Write the entire replacement for `core/src/tasks/memory_task.rs`**

```rust
//! 记忆提取任务。
//!
//! 扫描 VFS 中的会话，对含有新消息的会话调用 MemoryExtractor 提取长期记忆。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, MemoryEntry, TianyanUri};
use crate::memory::{format_memory_as_markdown, MemoryExtractor};
use crate::scheduler::{TaskContext, TaskHandler, TaskResult};

/// 会话的记忆提取状态，存储在会话目录下的 `_metadata` 文件中。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionExtractionState {
    /// 上次提取时处理到的消息行数。
    last_extracted_message_count: usize,
    /// 上次提取时间（ISO 8601）。
    last_extracted_at: String,
}

impl SessionExtractionState {
    fn new(message_count: usize) -> Self {
        Self {
            last_extracted_message_count: message_count,
            last_extracted_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 记忆提取任务。
///
/// 定时扫描 Session 命名空间下的所有会话，对比 VFS 中持久化的
/// `_metadata` 文件判断是否有新消息需要提取。
pub struct MemoryTask {
    /// 每个扫描周期处理上限，避免单次扫描耗时过长。
    max_process_per_cycle: usize,
}

impl MemoryTask {
    /// 创建新的记忆任务。
    pub fn new() -> Self {
        Self {
            max_process_per_cycle: 50,
        }
    }

    /// 从 `_metadata` 子文件中读取上次提取状态。
    async fn read_state(&self, ctx: &TaskContext, session_uri: &TianyanUri) -> Option<SessionExtractionState> {
        let state_uri = session_uri.append("_metadata");
        match ctx.vfs.read_content(&state_uri, ContentLevel::Detail).await {
            Ok(json) => match serde_json::from_str::<SessionExtractionState>(&json) {
                Ok(state) => Some(state),
                Err(e) => {
                    tracing::warn!(
                        session_uri = %session_uri,
                        error = %e,
                        "解析 _metadata 文件失败，将全量提取"
                    );
                    None
                }
            },
            Err(_) => None,
        }
    }

    /// 写入 `_metadata` 状态文件。
    async fn write_state(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
        state: &SessionExtractionState,
    ) -> Result<()> {
        let state_uri = session_uri.append("_metadata");
        let json = serde_json::to_string(state)
            .map_err(|e| crate::common::error::TianyanError::Serialization(e.to_string()))?;
        if !ctx.vfs.exists(&state_uri).await? {
            ctx.vfs.create_file(&state_uri).await?;
        }
        ctx.vfs.write_content(&state_uri, &json).await?;
        Ok(())
    }

    /// 统计 session JSONL 文件中的消息行数。
    async fn count_session_messages(&self, ctx: &TaskContext, session_uri: &TianyanUri) -> Result<usize> {
        let content = ctx.vfs.read_content(session_uri, ContentLevel::Detail).await?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// 扫描需要提取记忆的会话。
    async fn scan_sessions(
        &self,
        ctx: &TaskContext,
    ) -> Result<Vec<TianyanUri>> {
        let mut candidates = Vec::new();
        let root_uri = TianyanUri::new(ContextNamespace::Session, vec![]);

        let entries = match ctx.vfs.list(&root_uri).await {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };

        for entry in entries {
            if !entry.is_directory() {
                continue;
            }
            let session_uri = entry.uri().clone();

            let current_count = match self.count_session_messages(ctx, &session_uri).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(session_uri = %session_uri, error = %e, "统计消息数失败");
                    continue;
                }
            };

            if current_count == 0 {
                continue;
            }

            let needs_extraction = match self.read_state(ctx, &session_uri).await {
                Some(state) => current_count > state.last_extracted_message_count,
                None => true,
            };

            if needs_extraction {
                candidates.push(session_uri);
                if candidates.len() >= self.max_process_per_cycle {
                    break;
                }
            }
        }

        Ok(candidates)
    }

    /// 存储单条记忆到 VFS。
    async fn store_memory(
        &self,
        ctx: &TaskContext,
        memory: &MemoryEntry,
    ) -> Result<()> {
        let uri = &memory.uri;

        if let Some(parent) = uri.parent() {
            if !ctx.vfs.exists(&parent).await? {
                ctx.vfs.create_directory(&parent).await?;
            }
        }

        let content = format_memory_as_markdown(memory);
        ctx.vfs.write_content(uri, &content).await?;

        let abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {}",
            memory.content, memory.importance, memory.category
        );
        ctx.vfs.write_abstract(uri, &abstract_content).await?;

        tracing::debug!(memory_id = %memory.id, uri = %uri, "记忆已持久化");
        Ok(())
    }

    /// 处理单个会话：提取并持久化记忆。
    async fn process_session(
        &self,
        ctx: &TaskContext,
        session_uri: &TianyanUri,
    ) -> Result<usize> {
        let conversation = ctx.vfs.read_content(session_uri, ContentLevel::Detail).await?;

        let memories = ctx.memory_extractor.extract(&conversation).await?;

        let mut stored_count = 0;
        for mut memory in memories {
            memory.source_session = Some(session_uri.to_string());
            if let Err(e) = self.store_memory(ctx, &memory).await {
                tracing::warn!(memory_id = %memory.id, error = %e, "存储记忆失败");
                continue;
            }
            stored_count += 1;
        }

        let message_count = conversation.lines().filter(|l| !l.trim().is_empty()).count();
        let state = SessionExtractionState::new(message_count);
        self.write_state(ctx, session_uri, &state).await?;

        tracing::info!(
            session_uri = %session_uri,
            stored_count = stored_count,
            message_count = message_count,
            "记忆提取完成"
        );

        Ok(stored_count)
    }
}

impl Default for MemoryTask {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskHandler for MemoryTask {
    async fn execute(&self, ctx: &TaskContext) -> TaskResult {
        tracing::info!("开始执行记忆提取任务...");

        let sessions = match self.scan_sessions(ctx).await {
            Ok(uris) => uris,
            Err(e) => {
                return TaskResult::failed(format!("扫描会话失败：{}", e));
            }
        };

        if sessions.is_empty() {
            tracing::debug!("没有需要提取记忆的会话");
            return TaskResult::success(0);
        }

        tracing::info!("发现 {} 个会话需要提取记忆", sessions.len());

        let mut total_memories = 0;
        let mut processed_count = 0;

        for session_uri in sessions {
            match self.process_session(ctx, &session_uri).await {
                Ok(count) => {
                    total_memories += count;
                    processed_count += 1;
                }
                Err(e) => {
                    tracing::warn!(session_uri = %session_uri, error = %e, "处理会话失败");
                }
            }
        }

        tracing::info!(
            "记忆提取任务完成，处理了 {} 个会话，提取 {} 条记忆",
            processed_count,
            total_memories
        );

        TaskResult::success(total_memories)
    }

    fn name(&self) -> &str {
        "memory_extraction"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_task_new() {
        let task = MemoryTask::new();
        assert_eq!(task.name(), "memory_extraction");
    }

    #[test]
    fn test_memory_task_default() {
        let task: MemoryTask = Default::default();
        assert_eq!(task.name(), "memory_extraction");
    }

    #[test]
    fn test_extraction_state_roundtrip() {
        let state = SessionExtractionState::new(42);
        let json = serde_json::to_string(&state).unwrap();
        let parsed: SessionExtractionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_extracted_message_count, 42);
        assert!(!parsed.last_extracted_at.is_empty());
    }
}
```

- [ ] **Step 2: Verify `chrono` is in workspace dependencies**

Check `Cargo.toml` for `chrono` in `[workspace.dependencies]`. If not present, add it. If present, ensure `core/Cargo.toml` has `chrono = { workspace = true }`.

- [ ] **Step 3: Run build check**

```powershell
cargo check -p tianyan-core 2>&1
```

Expected: The tasks module should compile. Errors from agent/* and server/* are expected (not yet updated).

- [ ] **Step 4: Run unit tests for the memory task**

```powershell
cargo test -p tianyan-core --lib -- memory_task --nocapture 2>&1
```

Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add core/src/tasks/memory_task.rs
git commit -m "refactor: MemoryTask uses MemoryExtractor + VFS _metadata tracking"
```

---

### Task 5: Remove `MemoryExtractionService` from Agent

**Files:**
- Modify: `core/src/agent/coordinator.rs`
- Modify: `core/src/agent/builder.rs`

- [ ] **Step 1: Remove from `core/src/agent/coordinator.rs`**

**Remove import (line 135):**
Delete:
```rust
use crate::storage::{MemoryExtractionService, VirtualFileSystem};
```
Add:
```rust
use crate::storage::VirtualFileSystem;
```

**Remove field from `Agent` struct (line 175):**
Delete `memory_extractor: Option<Arc<MemoryExtractionService>>,` from the struct definition.

**Remove from `Agent::new()` parameter (line 192) and assignment (line 205):**
Delete `memory_extractor: Option<Arc<MemoryExtractionService>>,` from the parameter list.
Delete `memory_extractor,` from the struct literal.

**Delete `extract_memories_from_session` method (lines 328-353):**

Delete the entire method including the doc comment:
```rust
    /// 在会话结束后提取和保存记忆。
    async fn extract_memories_from_session(&self, state: &SessionState) -> Result<()> {
        ...
        Ok(())
    }
```

**Delete the 3 spawn blocks:**
- Lines 304-321 (in `process_message`)
- Lines 493-510 (in `handle_clarification_response`)  
- Lines 591-608 (in `process_message_stream`)

Each block looks like:
```rust
        if response.is_complete && !response.needs_clarification {
            let agent_clone = self.clone();
            let state_clone = state.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = agent_clone
                    .extract_memories_from_session(&state_clone)
                    .await
                {
                    tracing::warn!(error = %e, "记忆提取后台任务失败");
                }
                agent_clone
                    .scan_and_promote_rules(&state_clone.session_id)
                    .await;
                if let Err(e) = agent_clone.learn_skills_from_session(&state_clone).await {
                    tracing::warn!(error = %e, "技能学习后台任务失败");
                }
            });
            self.background_tasks.lock().await.push(handle);
        }
```

Replace each with (keeping `scan_and_promote_rules` and `learn_skills_from_session`):
```rust
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
```

- [ ] **Step 2: Remove from `core/src/agent/builder.rs`**

**Remove import (lines 22-24):**
Delete:
```rust
use crate::storage::{
    ExtractionConfig, MemoryExtractionService, VirtualFileSystem, DEFAULT_EXTRACTION_PROMPT,
};
```
Add:
```rust
use crate::storage::VirtualFileSystem;
```

**Remove field (line 36):**
Delete `memory_extractor: Option<Arc<MemoryExtractionService>>,`

**Remove assignment from `Self` in `new()` (line 48):**
Delete `memory_extractor: None,`

**Remove `with_memory_extractor` method (lines 82-85):**
Delete:
```rust
    pub fn with_memory_extractor(mut self, extractor: Arc<MemoryExtractionService>) -> Self {
        self.memory_extractor = Some(extractor);
        self
    }
```

**Remove construction block from `build()` (lines 173-189):**
Delete:
```rust
        let memory_extractor = self.memory_extractor.or_else(|| {
            if self.config.enable_memory {
                let config = ExtractionConfig {
                    model: "gpt-4".to_string(),
                    prompt_template: DEFAULT_EXTRACTION_PROMPT.to_string(),
                    min_conversation_length: 50,
                    max_extracted_memories: 10,
                    min_importance_threshold: 0.3,
                };
                let extractor =
                    MemoryExtractionService::new(model_service.clone(), vfs.clone(), config);
                Some(Arc::new(extractor))
            } else {
                None
            }
        });
```

**Update `Agent::new()` call (lines 191-202):**
Remove `memory_extractor,` from the arguments.

- [ ] **Step 3: Run build check**

```powershell
cargo check -p tianyan-core 2>&1
```

Expected: PASS (no errors in core). Errors only in server/* are OK.

- [ ] **Step 4: Run unit tests for agent**

```powershell
cargo test -p tianyan-core --lib -- agent --nocapture 2>&1
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add core/src/agent/coordinator.rs core/src/agent/builder.rs
git commit -m "refactor: remove MemoryExtractionService from Agent"
```

---

### Task 6: Remove `MemoryExtractionService` from `SummaryService`

**Files:**
- Modify: `core/src/storage/summary/service.rs`

- [ ] **Step 1: Update imports in `core/src/storage/summary/service.rs`**

Replace lines 14-17:
```rust
use crate::storage::{
    ExtractionConfig, MemoryExtractionService, SummaryEngine, VirtualFileSystem,
    ABSTRACT_TOKEN_LIMIT,
};
```

With:
```rust
use crate::storage::{SummaryEngine, VirtualFileSystem, ABSTRACT_TOKEN_LIMIT};
```

- [ ] **Step 2: Remove `memory_extractor` field from `SummaryService` struct (line 127)**

Delete:
```rust
    /// 记忆提取器（可选，用于从会话中提取记忆）。
    pub memory_extractor: Option<Arc<MemoryExtractionService>>,
```

- [ ] **Step 3: Remove from `SummaryService::new()` (line 156)**

Delete:
```rust
            memory_extractor: None,
```

- [ ] **Step 4: Remove `with_memory_extractor` method (lines 171-174)**

Delete the entire method block.

- [ ] **Step 5: Remove `with_memory_extractor_from_model` method (lines 183-196)**

Delete the entire method block.

- [ ] **Step 6: Remove memory extraction call from `process_uri` (lines 573-589)**

In `SummaryService::process_uri`, delete:
```rust
        // 如果是 Session 且配置了记忆提取器，则异步提取记忆
        if let Some(ref extractor) = self.memory_extractor {
            if uri.namespace() == ContextNamespace::Session {
                // 异步提取记忆，不阻塞主流程
                let extractor = extractor.clone();
                let session_uri = uri.clone();
                tokio::spawn(async move {
                    match extractor.extract_from_session(&session_uri).await {
                        Ok(memories) => {
                            tracing::info!("从会话提取 {} 条记忆：{}", memories.len(), session_uri);
                        }
                        Err(e) => {
                            tracing::warn!("从会话提取记忆失败：{} - {}", session_uri, e);
                        }
                    }
                });
            }
        }
```

- [ ] **Step 7: Run build check**

```powershell
cargo check -p tianyan-core 2>&1
```

Expected: PASS (no errors in core).

- [ ] **Step 8: Commit**

```bash
git add core/src/storage/summary/service.rs
git commit -m "refactor: remove MemoryExtractionService from SummaryService"
```

---

### Task 7: Update server crate

**Files:**
- Modify: `server/src/state.rs`
- Modify: `server/src/lib.rs`

- [ ] **Step 1: Update `server/src/state.rs`**

Replace line 22 (import):
```rust
use tianyan::storage::{
    DEFAULT_EXTRACTION_PROMPT, ExtractionConfig, MemoryExtractionService, SummaryEngine,
    SummaryService, VirtualFileSystemImpl,
};
```

With:
```rust
use tianyan::memory::{ExtractionConfig, MemoryExtractor};
use tianyan::storage::{SummaryEngine, SummaryService, VirtualFileSystemImpl};
```

Update `create_memory_extractor` method (lines 290-315):

```rust
    /// 创建记忆提取器
    ///
    /// # Returns
    /// * `TianyanResult<Arc<MemoryExtractor>>` - 记忆提取器实例
    pub fn create_memory_extractor(
        &self,
    ) -> TianyanResult<Arc<MemoryExtractor>> {
        let config = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.config.read().await.clone() })
        });

        let model_services = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { create_model_services(&config).await })
        })
        .map_err(|e| TianyanError::ModelService(format!("模型服务创建失败：{}", e)))?;

        let extractor = MemoryExtractor::new(
            model_services.chat,
            ExtractionConfig::default(),
        );
        Ok(Arc::new(extractor))
    }
```

Remove line 308 `prompt_template: DEFAULT_EXTRACTION_PROMPT.to_string(),` — now handled by `ExtractionConfig::default()`.

- [ ] **Step 2: Update `server/src/lib.rs`**

Find and update the import of `MemoryExtractionService` (if any direct import exists). If imported via `tianyan::storage::`, change to `tianyan::memory::MemoryExtractor`.

The `start_server` function (line 233) calls `state.create_memory_extractor()?` which now returns `Arc<MemoryExtractor>`. The downstream usage in `TaskContext::new(...)` expects `Arc<MemoryExtractor>` (already updated in Task 3). No other changes needed in `lib.rs`.

- [ ] **Step 3: Run full workspace build check**

```powershell
cargo check --workspace 2>&1
```

Expected: PASS — all compilation errors resolved.

- [ ] **Step 4: Run full lint pass**

```powershell
cargo fmt --all -- --check 2>&1
cargo clippy --workspace -- -D warnings 2>&1
```

Expected: PASS

- [ ] **Step 5: Run unit tests**

```powershell
cargo test --workspace --lib -- --nocapture 2>&1
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add server/src/state.rs server/src/lib.rs
git commit -m "refactor: update server to use MemoryExtractor from memory module"
```

---

### Task 8: Add structural test and final cleanup

**Files:**
- Modify: `core/tests/structural.rs`

- [ ] **Step 1: Add architectural integrity test for new `memory` module**

Add to `core/tests/structural.rs` after line 97:

```rust
/// memory 模块不得依赖 storage 模块 — 提取逻辑应独立于存储层。
#[test]
fn test_memory_does_not_depend_on_storage() {
    let violations = grep_in_dir("use crate::storage", &format!("{}/memory", CORE_SRC));
    assert!(
        violations.is_empty(),
        "memory 模块不得依赖 storage 模块:\n{}",
        violations.join("\n")
    );
}
```

- [ ] **Step 2: Run structural tests**

```powershell
cargo test -p tianyan-core --test structural -- --nocapture 2>&1
```

Expected: All structural tests PASS, including new `test_memory_does_not_depend_on_storage`.

- [ ] **Step 3: Run full test suite**

```powershell
cargo test --workspace -- --nocapture 2>&1
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add core/tests/structural.rs
git commit -m "test: add structural test for memory module independence"
```

---

## Verification Checklist

After all tasks, verify:

```powershell
# Full check
cargo check --workspace
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace --lib -- --nocapture
cargo test --workspace --test '*' -- --nocapture
```

Expected: All PASS.

~~~

## Self-Review

1. **Spec coverage:**
   - Create `memory` module with pure `MemoryExtractor` → Task 1
   - Remove from `storage/` → Task 2
   - Update `TaskContext` → Task 3
   - Rewrite `MemoryTask` with VFS `_metadata.json` tracking → Task 4
   - Remove from Agent → Task 5
   - Remove from SummaryService → Task 6
   - Update server → Task 7
   - Structural test → Task 8

2. **Placeholder scan:** No "TBD", "TODO", or vague instructions found.

3. **Type consistency:**
   - `MemoryExtractor` defined in Task 1, consumed in Tasks 3, 4, 7. Signature: `fn new(model_service: Arc<dyn ChatService>, config: ExtractionConfig) -> Self` and `async fn extract(&self, conversation: &str) -> Result<Vec<MemoryEntry>>`. Consistent throughout.
   - `ExtractionConfig::default()` defined in Task 1, used in Task 7. Fields match.
   - `SessionExtractionState` used only in Task 4 (internal to MemoryTask). Consistent.
   - `format_memory_as_markdown()` defined in Task 1, used in Task 4. Consistent.
   - `_metadata` URI pattern used consistently in Task 4.
