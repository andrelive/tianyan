# 天演 Harness Engineering 演进计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将天演从"功能完备的 Agent 框架"演进为"越用越强的 Harness Engineering 系统"，核心标志是：运行时可追加规则（`agent/learned/`）、失败→免疫的闭环、统一的上下文工程管线。

**Architecture:** 分 5 个 Phase 渐进演进。Phase 1 收敛上下文管理架构（ContextWindow + Pipeline），Phase 2 统一检索管线（Memory/Knowledge 分流注入），Phase 3 补全 Harness Engineering 核心闭环（失败规则追加），Phase 4 建立记忆→规则转化机制，Phase 5 补全架构约束与可观测性。

**Tech Stack:** Rust, Tokio, Axum, VFS (tianyan:// 协议), Qdrant 向量库

---

## 已完成的前置工作

- [x] `default_soul.md` 从编译时 `include_str!` 迁移到 VFS `tianyan://agent/soul`
- [x] 新建 `tianyan://agent/learned/` 目录结构
- [x] `prompt.rs` 删除 `const SYSTEM_PROMPT`，改为参数传入
- [x] `SessionState` 增加 `system_prompt: Option<String>` 缓存字段
- [x] `coordinator.rs` 新增 `load_system_prompt()` 方法
- [x] `process_message` / `process_message_stream` / `handle_clarification_response` 三处调用 `load_system_prompt()`

---

## Phase 1: 上下文架构收敛（ContextWindow + Pipeline）

**目标:** 消除 `SessionState` 的上帝对象问题，将散落的上下文字段收敛为统一结构体，将 coordinator 中的上下文编排逻辑抽入 `ContextPipeline`。

### 当前问题

- `SessionState` 13 个字段跨越 5 个关注域
- `context_uris` / `retrieved_context` / `last_trace` 三个字段表达同一件事
- `prompt.rs` 放在 `agent/` 下语义不匹配
- coordinator 中 40+ 行过程式上下文缝合代码不可复用

### 改动清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `core/src/context/types.rs` | 修改 | 新增 `ContextWindow`、`ContextTokenUsage` 结构体 |
| `core/src/context/assembly.rs` | 新建 | 从 `agent/prompt.rs` 迁入 `build_prompt()` 和相关逻辑 |
| `core/src/context/pipeline.rs` | 新建 | 上下文工程管线，编入 system_prompt 加载、检索、压缩 |
| `core/src/context/mod.rs` | 修改 | 注册新子模块 |
| `core/src/agent/session_state.rs` | 修改 | 用 `context_window: Option<ContextWindow>` 替代散落字段 |
| `core/src/agent/prompt.rs` | 修改 | 删除 `build_prompt_with_history`，保留 `parse_llm_output` |
| `core/src/agent/coordinator.rs` | 修改 | 用 `context_pipeline.run()` 替代过程式缝合 |
| `core/src/agent/mod.rs` | 修改 | 更新 re-export |
| `core/src/planner/mod.rs` | 修改 | 适配 `build_prompt_context` 的签名变化 |

### 目标结构

```
tianyan://agent/soul         ← 核心提示词
tianyan://agent/learned/     ← 失败教训规则
         ↓ load_system_prompt()
┌──────────────────────────────────┐
│ ContextWindow                    │
│  system_prompt: String           │  ← 合并 soul + learned/*.Abstract
│  summary: Option<String>         │  ← 压缩摘要
│  retrieved: Vec<RetrievalResult> │  ← 统一检索结果（含 URI/内容/分数/层级/类别）
│  token_usage: ContextTokenUsage  │
└──────────────┬───────────────────┘
               │ assemble_prompt()
               ▼
          LLM Prompt 字符串
```

### Task 1.1: 新增 `ContextWindow` 和 `ContextTokenUsage`

**Files:**
- Modify: `core/src/context/types.rs`

- [ ] **Step 1: 在 `context/types.rs` 文件末尾追加新类型定义**

```rust
/// 统一上下文窗口，聚合所有注入 prompt 的上下文。
#[derive(Debug, Clone)]
pub struct ContextWindow {
    /// 系统提示词（从 VFS Agent 命名空间加载）
    pub system_prompt: String,
    /// 压缩后的对话摘要
    pub summary: Option<String>,
    /// 检索结果（含 URI、内容、分数、层级、token 数、类别）
    pub retrieved: Vec<RetrievalResult>,
    /// Token 使用统计
    pub token_usage: ContextTokenUsage,
}

impl ContextWindow {
    /// 创建空的上下文窗口。
    pub fn new(system_prompt: String) -> Self {
        Self {
            system_prompt,
            summary: None,
            retrieved: Vec::new(),
            token_usage: ContextTokenUsage::default(),
        }
    }

    /// 计算总 token 数。
    pub fn total_tokens(&self) -> usize {
        self.token_usage.total()
    }
}

/// 上下文窗口的 token 使用统计。
#[derive(Debug, Clone, Default)]
pub struct ContextTokenUsage {
    pub system_prompt_tokens: usize,
    pub summary_tokens: usize,
    pub retrieved_tokens: usize,
}

impl ContextTokenUsage {
    pub fn total(&self) -> usize {
        self.system_prompt_tokens + self.summary_tokens + self.retrieved_tokens
    }
}
```

- [ ] **Step 2: 在 `context/types.rs` 顶部增加 `use` 导入**

```rust
use crate::context::retrieval::RetrievalResult;
```

（若产生循环引用，改为保留 `RetrievalResult` 在 `context/types.rs` 中定义，原 `retrieval/retriever.rs` 从 `crate::context::types::RetrievalResult` 导入）

- [ ] **Step 3: 运行 `cargo check -p tianyan-core` 确认编译**

---

### Task 1.2: 新建 `context/assembly.rs`

**Files:**
- Create: `core/src/context/assembly.rs`
- Modify: `core/src/context/mod.rs`

- [ ] **Step 1: 创建 `core/src/context/assembly.rs`**

```rust
//! 上下文组装模块。
//!
//! 将 ContextWindow 与 SessionState 中的动态数据（对话历史、执行历史）
//! 组装为最终发送给 LLM 的 prompt 字符串。

use crate::common::types::Message;
use crate::context::types::ContextWindow;
use crate::planner::types::Turn;

/// 从 ContextWindow 和会话状态中的动态数据构建最终 prompt。
pub fn assemble_prompt(
    window: &ContextWindow,
    conversation: &[Message],
    turns: &[Turn],
    current_input: &str,
) -> String {
    let mut prompt = String::new();

    // 系统提示词
    prompt.push_str(&window.system_prompt);
    prompt.push_str("\n\n---\n\n");

    // 对话历史
    if !conversation.is_empty() {
        prompt.push_str("## 对话历史\n\n");
        for msg in conversation {
            let role = match msg.role {
                crate::common::types::MessageRole::User => "用户",
                crate::common::types::MessageRole::Assistant => "助手",
                crate::common::types::MessageRole::System => "系统",
                crate::common::types::MessageRole::Tool => "工具",
            };
            prompt.push_str(&format!("{}: {}\n\n", role, msg.content));
        }
        prompt.push_str("---\n\n");
    }

    // 执行历史
    if !turns.is_empty() {
        prompt.push_str("## 历史执行记录\n\n");
        for turn in turns {
            prompt.push_str(&format!("### 轮次 {}\n", turn.turn_id));
            for result in &turn.results {
                prompt.push_str(&format!(
                    "步骤 {}: {} => {}\n",
                    result.step_id,
                    if result.success { "成功" } else { "失败" },
                    result.output
                ));
            }
            prompt.push('\n');
        }
        prompt.push_str("---\n\n");
    }

    // 压缩摘要
    if let Some(ref summary) = window.summary {
        prompt.push_str("## 历史对话摘要\n");
        prompt.push_str(summary);
        prompt.push_str("\n\n");
    }

    // 检索到的上下文（按类别分组）
    if !window.retrieved.is_empty() {
        let (memories, knowledge): (Vec<_>, Vec<_>) = window
            .retrieved
            .iter()
            .partition(|r| r.category == "memory");

        if !memories.is_empty() {
            prompt.push_str("## 相关历史记忆\n\n");
            for (i, m) in memories.iter().enumerate() {
                if let Some(ref content) = m.content {
                    prompt.push_str(&format!("- {}\n", content));
                    let _ = i;
                }
            }
            prompt.push('\n');
        }

        if !knowledge.is_empty() {
            prompt.push_str("## 检索到的相关知识\n\n");
            for (i, k) in knowledge.iter().enumerate() {
                if let Some(ref content) = k.content {
                    prompt.push_str(&format!("{}. {}\n", i + 1, content));
                }
            }
            prompt.push('\n');
        }
    }

    // 当前输入
    prompt.push_str("## 当前输入\n\n");
    prompt.push_str(current_input);
    prompt.push_str("\n\n## 你的执行计划\n");

    prompt
}
```

- [ ] **Step 2: 在 `context/mod.rs` 中注册新子模块**

```rust
pub mod assembly;
pub mod pipeline;
```

并增加 re-export：

```rust
pub use assembly::assemble_prompt;
```

- [ ] **Step 3: 运行 `cargo check -p tianyan-core` 确认编译**

---

### Task 1.3: 新建 `context/pipeline.rs` —— 上下文工程管线

**Files:**
- Create: `core/src/context/pipeline.rs`
- Modify: `core/src/context/mod.rs`

- [ ] **Step 1: 创建 `core/src/context/pipeline.rs`**

```rust
//! 上下文工程管线。
//!
//! 将上下文装配的完整流程封装为可复用、可测试的管线：
//! load_system_prompt → search → compress → build ContextWindow

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, Message, TianyanUri};
use crate::context::compression::ContextCompressor;
use crate::context::retrieval::{DualLayerRetriever, RetrievalResult};
use crate::context::types::{ContextTokenUsage, ContextWindow};
use crate::planner::types::Turn;
use crate::storage::VirtualFileSystem;

/// 上下文工程管线。
pub struct ContextPipeline {
    vfs: Arc<dyn VirtualFileSystem>,
    retriever: Arc<DualLayerRetriever>,
    compressor: ContextCompressor,
    default_top_k: usize,
}

impl ContextPipeline {
    /// 创建新的上下文管线。
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        retriever: Arc<DualLayerRetriever>,
        compressor: ContextCompressor,
        default_top_k: usize,
    ) -> Self {
        Self {
            vfs,
            retriever,
            compressor,
            default_top_k,
        }
    }

    /// 执行完整上下文管线。
    ///
    /// - `query` - 用户查询（用于检索）
    /// - `conversation` - 当前对话历史（可变引用，压缩可能修改）
    /// - `turns` - 执行历史
    pub async fn run(
        &self,
        query: &str,
        conversation: &mut Vec<Message>,
        turns: &[Turn],
    ) -> Result<ContextWindow> {
        let system_prompt = self.load_system_prompt().await?;
        let retrieved = self.search(query).await?;
        let summary = self.compress_if_needed(conversation).await?;

        let mut token_usage = ContextTokenUsage::default();
        token_usage.system_prompt_tokens = estimate_tokens(&system_prompt);
        token_usage.retrieved_tokens = retrieved.iter().map(|r| r.token_count).sum();

        if let Some(ref s) = summary {
            token_usage.summary_tokens = estimate_tokens(s);
        }

        Ok(ContextWindow {
            system_prompt,
            summary,
            retrieved,
            token_usage,
        })
    }

    /// 从 VFS 加载系统提示词（soul + learned rules）。
    async fn load_system_prompt(&self) -> Result<String> {
        let mut prompt = String::new();

        let soul_uri = TianyanUri::new(ContextNamespace::Agent, vec!["soul".to_string()]);
        match self.vfs.read_content(&soul_uri, ContentLevel::Detail).await {
            Ok(content) => {
                prompt.push_str(&content);
                prompt.push_str("\n\n");
            }
            Err(e) => {
                tracing::warn!(error = %e, uri = %soul_uri, "加载核心提示词失败");
            }
        }

        let learned_uri = TianyanUri::new(ContextNamespace::Agent, vec!["learned".to_string()]);
        match self.vfs.list(&learned_uri).await {
            Ok(entries) => {
                if !entries.is_empty() {
                    prompt.push_str("---\n");
                    prompt.push_str("## 你已经学到的经验教训\n\n");
                    for entry in &entries {
                        if let Ok(content) = self
                            .vfs
                            .read_content(entry.uri(), ContentLevel::Abstract)
                            .await
                        {
                            if !content.trim().is_empty() {
                                prompt.push_str(&format!("- {}\n", content.trim()));
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }

        Ok(prompt)
    }

    /// 统一检索（Memory + Knowledge 一次搜索，结果按类别区分）。
    async fn search(&self, query: &str) -> Result<Vec<RetrievalResult>> {
        self.retriever.retrieve(query, self.default_top_k).await
    }

    /// 如需压缩则执行压缩，返回摘要文本。
    async fn compress_if_needed(&self, conversation: &mut Vec<Message>) -> Result<Option<String>> {
        if !self.compressor.should_compress(conversation) {
            return Ok(None);
        }

        let result = self.compressor.compress(conversation).await?;
        let summary = if result.summary.is_empty() {
            None
        } else {
            Some(result.summary)
        };

        *conversation = result.messages;
        tracing::info!(
            compressed = result.compressed_count,
            original_tokens = result.original_tokens,
            compressed_tokens = result.compressed_tokens,
            "上下文压缩完成"
        );

        Ok(summary)
    }
}

fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}
```

- [ ] **Step 2: 在 `context/mod.rs` 中注册**

```rust
pub use pipeline::ContextPipeline;
```

- [ ] **Step 3: 运行 `cargo check -p tianyan-core` 确认编译**

---

### Task 1.4: 瘦身 `SessionState`

**Files:**
- Modify: `core/src/agent/session_state.rs`

- [ ] **Step 1: 用 `context_window` 替代 5 个散落字段**

删除以下字段：
- `context_uris: Vec<TianyanUri>`
- `last_trace: Option<RetrievalTrace>`
- `retrieved_context: Vec<String>`
- `compressed_summary: Option<String>`
- `system_prompt: Option<String>`

新增：
```rust
/// 当前会话的上下文窗口。
pub context_window: Option<ContextWindow>,
```

- [ ] **Step 2: 更新 `new()` 方法**

删除被移除字段的初始化，增加：
```rust
context_window: None,
```

- [ ] **Step 3: 更新 `build_prompt_context()` 方法**

改为使用 `ContextWindow`：

```rust
pub fn build_prompt_context(&self, current_input: &str) -> String {
    let window = self.context_window.as_ref();
    let system_prompt = window.map(|w| w.system_prompt.as_str()).unwrap_or("");
    let conversation = &self.conversation;
    let turns = self.execution_context.get_turns();

    crate::context::assembly::assemble_prompt_with_raw(
        system_prompt,
        window.and_then(|w| w.summary.as_deref()),
        window.map(|w| w.retrieved.as_slice()).unwrap_or(&[]),
        conversation,
        turns,
        current_input,
    )
}
```

或者更简单：如果 `ContextWindow` 已就绪，直接委托给 `assemble_prompt`：

```rust
pub fn build_prompt_context(&self, current_input: &str) -> String {
    if let Some(ref window) = self.context_window {
        crate::context::assembly::assemble_prompt(
            window,
            &self.conversation,
            self.execution_context.get_turns(),
            current_input,
        )
    } else {
        // 回退：无上下文窗口时最小化 prompt
        format!("## 当前输入\n\n{}\n\n## 你的执行计划\n", current_input)
    }
}
```

- [ ] **Step 4: 清理 `use` 导入**

删除不再需要的 `use crate::context::RetrievalTrace;`

- [ ] **Step 5: 更新测试中的字段引用**

在 `tests` 模块中，确保 `test_session_state_new()` 等测试中不引用被删除的字段。

- [ ] **Step 6: 运行 `cargo test -p tianyan-core --lib -- agent::session` 确认通过**

---

### Task 1.5: 适配 coordinator 使用 ContextPipeline

**Files:**
- Modify: `core/src/agent/coordinator.rs`

- [ ] **Step 1: 修改 `Agent` 结构体**

删除 `retriever: Arc<DualLayerRetriever>` 和 `compressor: ContextCompressor` 字段，替换为：
```rust
context_pipeline: ContextPipeline,
```

- [ ] **Step 2: 修改 `Agent::new()` 构造函数**

```rust
let context_pipeline = ContextPipeline::new(
    vfs.clone(),
    retriever.clone(),
    ContextCompressor::new(
        model_service.clone(),
        CompressionConfig {
            preserve_recent_messages: 6,
            ..CompressionConfig::default()
        },
    ),
    config.default_top_k,
);
```

- [ ] **Step 3: 删除 `inject_relevant_memories`、`retrieve_context`、`load_system_prompt` 三个方法**

管线已封装这些逻辑。也可暂时保留标记为 `#[allow(dead_code)]` 待后续清理。

- [ ] **Step 4: 重写 `process_message`**

```rust
async fn process_message(
    &self,
    state: &mut SessionState,
    message: &str,
) -> Result<AgentResponse> {
    let start = Instant::now();
    let message = message.to_string();

    state.add_user_message(&message);

    // 运行上下文管线
    match self.context_pipeline
        .run(&message, &mut state.conversation, state.execution_context.get_turns())
        .await
    {
        Ok(window) => {
            state.context_window = Some(window);
        }
        Err(e) => {
            tracing::warn!(error = %e, "上下文管线执行失败");
        }
    }

    let result = self.planner.lock().await.run(message.clone(), state).await;
    // ... 后续处理保持不变
}
```

- [ ] **Step 5: 同步修改 `process_message_stream` 和 `handle_clarification_response`**

同样用管线替换过程式缝合。

- [ ] **Step 6: 运行 `cargo check -p tianyan-core` 确认编译**

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: 引入 ContextWindow 和 ContextPipeline 统一上下文管理"
```

---

## Phase 2: 统一检索管线（Memory/Knowledge 分流注入）

**目标:** 合并两次检索为一次，按 namespace 分流到 prompt 的不同区域，消除冗余检索和死数据字段。

### 当前问题

- `inject_relevant_memories()` 限制 namespace 为 Memory 做一次搜索
- `retrieve_context()` 搜全部 namespace 做第二次搜索
- Memory 和 Knowledge 结果在 prompt 中混在一起，无法区分对待
- `context_uris` 字段写了再无人读

### 改动清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `core/src/context/assembly.rs` | 确认 | 已实现 Memory/Knowledge 分区渲染（Phase 1 已完成） |
| `core/src/context/pipeline.rs` | 确认 | 已实现 `search()` 统一一次检索（Phase 1 已完成） |
| `core/src/context/retrieval/retriever.rs` | 修改 | 可选：为 Memory namespace 增加 score bias（+15%） |

### Task 2.1: 为 Memory namespace 增加检索优先级偏置

**Files:**
- Modify: `core/src/context/retrieval/retriever.rs`

- [ ] **Step 1: 在 `fused_search` 方法中，RRF 融合后增加 Memory bias**

```rust
// 在 fused_search 返回前，RRF 融合后：
for result in &mut results {
    if result.uri.namespace() == ContextNamespace::Memory {
        result.score *= 1.15;
    }
}
results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
```

- [ ] **Step 2: 运行 `cargo test -p tianyan-core --lib -- context::retrieval` 确认通过**

- [ ] **Step 3: Commit**

```bash
git add core/src/context/retrieval/retriever.rs
git commit -m "feat: Memory namespace 检索结果获得 15% 分数偏置"
```

---

## Phase 3: Harness Engineering 核心闭环

**目标:** 实现"智能体失败 → 追加规则到 `agent/learned/` → 同一失败永不发生第二次"的免疫系统。

### 当前状态

- `agent/learned/` 目录已创建 ✓
- `load_system_prompt()` 已加载 learned 规则 ✓
- **缺失**：追加规则的机制

### Task 3.1: 新增 `RuleRecorder` — 运行时追加规则到 `agent/learned/`

**Files:**
- Create: `core/src/context/rule_recorder.rs`
- Modify: `core/src/context/mod.rs`

- [ ] **Step 1: 创建 `core/src/context/rule_recorder.rs`**

```rust
//! 规则记录器。
//!
//! Harness Engineering 核心：每次智能体失败时，追加一条规则到 agent/learned/，
//! 确保同一失败永不发生第二次。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::storage::VirtualFileSystem;

/// 规则记录器。
pub struct RuleRecorder {
    vfs: Arc<dyn VirtualFileSystem>,
}

impl RuleRecorder {
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs }
    }

    /// 追加一条学习规则。
    ///
    /// - `abstract_text` - 规则摘要（~100 tokens），注入 system_prompt
    /// - `detail_text` - 规则详情（含溯源信息），存储在 Overview 层
    /// - `source_session` - 触发此规则的会话 ID
    pub async fn record(
        &self,
        abstract_text: &str,
        detail_text: &str,
        source_session: &str,
    ) -> Result<()> {
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let rule_id = format!("rule-{}", ts);

        let uri = TianyanUri::new(
            ContextNamespace::Agent,
            vec!["learned".to_string(), rule_id],
        );

        self.vfs.create_file(&uri).await?;

        // Abstract 层：注入 prompt 的简短摘要
        let abstract_content = format!("{} (来源会话: {})", abstract_text, source_session);
        self.vfs.write_abstract(&uri, &abstract_content).await?;

        // Detail 层：完整规则
        self.vfs.write_content(&uri, detail_text).await?;

        tracing::info!(
            rule_id = %uri.to_string(),
            session_id = %source_session,
            "已追加学习规则"
        );

        Ok(())
    }
}
```

- [ ] **Step 2: 在 `context/mod.rs` 中注册**

```rust
pub mod rule_recorder;
pub use rule_recorder::RuleRecorder;
```

- [ ] **Step 3: 运行 `cargo check -p tianyan-core` 确认编译**

---

### Task 3.2: 在 coordinator 中接入 RuleRecorder

**Files:**
- Modify: `core/src/agent/coordinator.rs`

- [ ] **Step 1: 在 `Agent` 结构体中增加 `rule_recorder` 字段**

```rust
rule_recorder: RuleRecorder,
```

- [ ] **Step 2: 在 `Agent::new()` 中初始化**

```rust
let rule_recorder = RuleRecorder::new(vfs.clone());
```

- [ ] **Step 3: 在 `process_message` 的 Planner 错误分支中触发规则录制**

Planner 返回错误后、生成 AgentResponse 之前，调用：

```rust
Err(e) => {
    // 记录失败模式到 learned 规则
    let abstract_text = format!("Planner 执行失败: 需要避免导致 {} 的操作模式", e);
    let detail = format!(
        "# 规则: 避免 Planner 执行失败\n\n\
         **来源会话**: {}\n\
         **错误信息**: {}\n\
         **建议**: 分析失败原因，在规划阶段增加对应的前置检查步骤\n",
        state.session_id, e
    );
    let _ = self.rule_recorder
        .record(&abstract_text, &detail, &state.session_id)
        .await;

    state.cleanup();
    AgentResponse::error(format!("Planner 执行失败：{}", e))
}
```

- [ ] **Step 4: 在 Executor 步骤失败时也同样触发**

在 `process_message` 的步骤失败日志处增加规则录制逻辑。

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: 实现 RuleRecorder — 失败→规则追加的 Harness Engineering 闭环"
```

---

### Task 3.3: 增加自我验证工具 — `RunTests` 和 `VerifyBuild`

**Files:**
- Create: `core/src/skills/verification.rs`（或扩展现有 `executor.rs` 增加 Action 类型）
- Modify: `core/src/executor/executor.rs`

- [ ] **Step 1: 新增 `Action::RunTests` 变体**

在 `core/src/planner/types.rs` 中：

```rust
Action::RunTests {
    /// 测试命令（如 "cargo test --lib"）
    command: String,
    /// 工作目录
    cwd: Option<String>,
    /// 超时秒数
    timeout_secs: Option<u64>,
}
```

- [ ] **Step 2: 在 Executor 中实现 `RunTests` 的执行逻辑**

执行命令，解析测试输出，返回结构化结果：
```rust
struct TestResult {
    passed: usize,
    failed: usize,
    failures: Vec<String>,
}
```

- [ ] **Step 3: 同步更新 `default_soul.md`（`agent/soul`）**

告知 Planner 可以使用 `RunTests` 和 `VerifyBuild` Action。

- [ ] **Step 4: 运行 `cargo check -p tianyan-core` 确认编译**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: 新增 RunTests / VerifyBuild 自我验证 Action"
```

---

## Phase 4: 记忆 → 规则转化机制

**目标:** 建立 `FailedCase` 和 `Pattern` 记忆自动提炼为 Learned Rule 的转化路径。

### 核心设计

```
MemoryExtractionService → 提取原始记忆 (FailedCase, Pattern)
        ↓
RuleSuggester → 检测同类记忆 ≥2 次 → 建议提炼为规则
        ↓
确认后写入 agent/learned/rule-xxx
        ↓
原记忆标记 [已被规则覆盖]，降低注入优先级
```

### Task 4.1: 新增 `RuleSuggester`

**Files:**
- Create: `core/src/context/rule_suggester.rs`
- Modify: `core/src/context/mod.rs`

- [ ] **Step 1: 创建 `core/src/context/rule_suggester.rs`**

```rust
//! 规则建议器。
//!
//! 监控 Memory 命名空间中的 FailedCase 和 Pattern，
//! 当同类失败 ≥ 阈值时建议提炼为 Learned Rule。

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::model::ModelService;
use crate::storage::VirtualFileSystem;

pub struct RuleSuggester {
    vfs: Arc<dyn VirtualFileSystem>,
    model_service: Arc<dyn ModelService>,
    min_occurrences: usize,
}

impl RuleSuggester {
    pub fn new(vfs: Arc<dyn VirtualFileSystem>, model_service: Arc<dyn ModelService>) -> Self {
        Self {
            vfs,
            model_service,
            min_occurrences: 2,
        }
    }

    /// 扫描 FailedCase 和 Pattern 记忆，检测可提炼的模式。
    pub async fn scan(&self) -> Result<Vec<RuleSuggestion>> {
        let mut suggestions = Vec::new();

        // 收集 agent/patterns/ 下的 Pattern 记忆
        let pattern_uri = TianyanUri::new(
            ContextNamespace::Agent,
            vec!["patterns".to_string()],
        );
        self.collect_suggestions(&pattern_uri, "pattern", &mut suggestions).await?;

        // 收集 memory/cases/failed_tasks/ 下的 FailedCase 记忆
        let failed_uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["cases".to_string(), "failed_tasks".to_string()],
        );
        self.collect_suggestions(&failed_uri, "failed_case", &mut suggestions).await?;

        Ok(suggestions)
    }

    async fn collect_suggestions(
        &self,
        uri: &TianyanUri,
        category: &str,
        suggestions: &mut Vec<RuleSuggestion>,
    ) -> Result<()> {
        // 读取所有记忆，用 LLM 聚类，找出同主题 ≥ min_occurrences 的
        // 简化版：只计数，不做语义聚类
        if !self.vfs.exists(uri).await? {
            return Ok(());
        }

        let entries = self.vfs.list(uri).await?;
        if entries.len() >= self.min_occurrences {
            // 收集所有记忆的内容
            let mut contents = Vec::new();
            for entry in &entries {
                if let Ok(content) = self.vfs.read_content(entry.uri(), ContentLevel::Abstract).await {
                    contents.push(content);
                }
            }

            if contents.len() >= self.min_occurrences {
                suggestions.push(RuleSuggestion {
                    source_category: category.to_string(),
                    source_count: contents.len(),
                    source_contents: contents,
                });
            }
        }

        Ok(())
    }
}

pub struct RuleSuggestion {
    pub source_category: String,
    pub source_count: usize,
    pub source_contents: Vec<String>,
}
```

- [ ] **Step 2: 在 `context/mod.rs` 中注册**

- [ ] **Step 3: 在 coordinator 后台任务中增加规则建议扫描**

```rust
// spawn 后台任务定期调用 rule_suggester.scan()
```

- [ ] **Step 4: Commit**

---

## Phase 5: 架构约束与可观测性

**目标:** 补全 Harness Engineering 文档中识别的最严重缺失：机械架构约束、智能体可读的可观测性、Garbage Collection。

### Task 5.1: 智能体可读的可观测性 API

**Files:**
- Create: `core/src/observability/mod.rs`

- [ ] **Step 1: 暴露查询 API 供智能体使用**

```rust
/// 智能体可查询的可观测性接口。
pub trait AgentObservability {
    /// 查询"我上次做类似任务时的 token 消耗"
    async fn query_token_history(&self, task_pattern: &str) -> Vec<TokenRecord>;
    /// 查询"过去 N 次 cargo build 的成功率"
    async fn query_build_success_rate(&self, n: usize) -> f32;
    /// 查询"最常见的失败步骤类型"
    async fn query_common_failures(&self) -> Vec<FailureStats>;
}
```

- [ ] **Step 2: 提供对应的 CallSkill 技能（如 `query_failure_history`）**

### Task 5.2: Garbage Collection 基础

**Files:**
- Create: `core/src/tasks/gc_task.rs`

- [ ] **Step 1: 创建定期扫描任务**

扫描 `agent/learned/` 中的规则是否仍与当前代码一致（至少标记最后验证时间）。

- [ ] **Step 2: 集成到 Scheduler 中作为定期任务**

---

## 演进概览

| Phase | 核心交付 | 对 Harness 文档的对应 | 依赖 |
|-------|---------|---------------------|------|
| **1** | ContextWindow + Pipeline | §4.1 上下文工程收敛 | 无 |
| **2** | 统一检索 + Memory bias | §4.4 数据层治理（部分） | Phase 1 |
| **3** | RuleRecorder + 自我验证工具 | §2 失败驱动的环境增强（核心）、§5 Sensors | Phase 1 |
| **4** | RuleSuggester 转化逻辑 | §2.2 隐式提示自动化 | Phase 3 |
| **5** | 可观测性 + GC + 架构约束 | §4.2 约束、§4.3 垃圾回收、§6 原则 2 | Phase 1 |

## 关键设计原则

1. **每次 commit 保持可编译、测试通过** — 不积压破坏性改动
2. **Phase 之间正交** — 每个 Phase 可独立交付和验证
3. **优先基础设施，后补业务逻辑** — Phase 1-2 是纯架构优化，Phase 3+ 是增值功能
4. **统一检索入口** — 始终只有一次向量搜索
5. **渐进式披露** — 规则只注入 Abstract 层，系统提示词保持精炼
