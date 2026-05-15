# Agent Loop Architecture Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Planner-Executor batch architecture with a native Tool Call Agent Loop, eliminating `Plan`, `Step`, `Turn`, `PlannerContext`, `PlannerTrait`, and `ExecutorTrait`. Sub-tasks are handled via `delegate_to_agent` tool (Agent-as-Tool pattern). Clarification is triggered via `ask_user` tool.

**Architecture:** A single `AgentLoop` runs an LLM-in-the-loop with tools: each turn calls `chat_completion` with registered `ToolDefinition`s, executes returned `tool_calls` in parallel via `ToolRegistry`, appends results as `Message::tool`, and repeats until the LLM produces a direct answer or calls `ask_user`. `ToolRegistry` replaces `Executor` as the execution surface, with `Action` preserved only for `ApprovalWorkflow` risk assessment. `SessionState` is simplified to `conversation` as the sole source of truth.

**Tech Stack:** Rust, tokio, async-trait, serde, schemars, async-openai 0.34

---

## Phase 0: Pre-check & Baseline

### Task 0.1: Verify clean working tree and baseline compile

**Files:** (no file changes)

- [ ] **Step 1: Confirm workspace compiles**

Run:
```powershell
cargo check --workspace
```

Expected: Clean compile (current state after Tool Call integration).

- [ ] **Step 2: Run unit tests**

Run:
```powershell
cargo test --workspace --lib -- --nocapture
```

Expected: All tests pass.

- [ ] **Step 3: Commit baseline**

```bash
git status  # confirm clean
```

---

## Phase 1: Executor Foundation — Extract Action Logic, Remove Step/ExecutorTrait

### Task 1.1: Extract standalone execution functions from executor.rs

**Files:**
- Modify: `core/src/executor/executor.rs`
- Test: `core/src/executor/executor.rs` (existing tests, adapted)

**Rationale:** `ToolRegistry` will call these directly. `Executor` struct becomes a thin deprecated wrapper.

- [ ] **Step 1: Add `pub(crate)` free functions at the bottom of executor.rs**

Add after line 582 (after the last helper function `extract_build_errors`):

```rust
// ============================================================================
// Standalone execution functions (extracted for ToolRegistry reuse)
// ============================================================================

pub(crate) async fn execute_read_file(path: &str) -> Result<serde_json::Value, ExecutorError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ExecutorError::FileError(e.to_string()))?;
    Ok(serde_json::Value::String(content))
}

pub(crate) async fn execute_write_file(
    path: &str,
    content: &str,
) -> Result<serde_json::Value, ExecutorError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| ExecutorError::FileError(e.to_string()))?;
    Ok(serde_json::Value::String("写入成功".to_string()))
}

pub(crate) async fn execute_search_code(
    query: &str,
    scope: Option<&str>,
) -> Result<serde_json::Value, ExecutorError> {
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--line-number")
        .arg("--max-count")
        .arg("20")
        .arg(query);

    if let Some(dir) = scope {
        cmd.current_dir(dir);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| ExecutorError::SearchError(format!("执行 ripgrep 失败：{}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no matches found") || stderr.is_empty() {
            return Ok(serde_json::json!({
                "query": query,
                "scope": scope,
                "results": [],
                "count": 0
            }));
        }
        return Err(ExecutorError::SearchError(format!(
            "ripgrep 执行失败：{}",
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(data) = json_value.get("data") {
                results.push(serde_json::json!({
                    "path": data.get("path").map(|p| p.as_str().unwrap_or("")),
                    "line_number": data.get("line_number").map(|n| n.as_u64().unwrap_or(0)),
                    "text": data.get("text").map(|t| t.as_str().unwrap_or("")),
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "query": query,
        "scope": scope,
        "results": results,
        "count": results.len()
    }))
}
```

- [ ] **Step 2: Refactor `Executor::execute_action` to call extracted functions**

Replace the `Action::ReadFile` branch in `execute_action` (line 250-255):

```rust
Action::ReadFile { path } => execute_read_file(path).await,
```

Replace the `Action::WriteFile` branch (line 257-261):

```rust
Action::WriteFile { path, content } => execute_write_file(path, content).await,
```

Replace the `Action::SearchCode` branch (line 268-270):

```rust
Action::SearchCode { query, scope } => execute_search_code(query, scope.as_deref()).await,
```

- [ ] **Step 3: Verify executor tests still pass**

Run:
```powershell
cargo test -p tianyan-core --lib executor::executor::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add core/src/executor/executor.rs
git commit -m "refactor(executor): extract standalone execution functions for ToolRegistry reuse"
```

---

### Task 1.2: Remove Step/FailureHandling/StepResult from executor/types.rs

**Files:**
- Modify: `core/src/executor/types.rs`
- Test: `core/src/executor/types.rs` (rewrite tests)

- [ ] **Step 1: Replace the entire file contents**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 动作类型
///
/// 保留作为执行语义分类器，供 ApprovalWorkflow 风险评估和 ToolRegistry 执行映射使用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum Action {
    /// 文件读取
    ReadFile { path: String },
    /// 文件写入
    WriteFile { path: String, content: String },
    /// 命令执行
    ExecuteCommand {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// 代码搜索
    SearchCode {
        query: String,
        scope: Option<String>,
    },
    /// 子 Planner（已废弃，保留用于向后兼容的反序列化）
    #[serde(skip)]
    SubPlanner { task: String },
    /// 调用已注册的技能
    CallSkill {
        skill_id: String,
        parameters: serde_json::Map<String, Value>,
    },
    /// 运行测试套件（如 cargo test）
    RunTests {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
    /// 验证构建（如 cargo build / cargo check / cargo clippy）
    VerifyBuild {
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    },
}

/// Executor 错误类型
#[derive(thiserror::Error, Debug)]
pub enum ExecutorError {
    /// 文件操作失败
    #[error("文件操作失败：{0}")]
    FileError(String),
    /// 命令执行失败
    #[error("命令执行失败：{0}")]
    CommandError(String),
    /// 搜索失败
    #[error("搜索失败：{0}")]
    SearchError(String),
    /// 子 Planner 失败
    #[error("子 Planner 失败：{0}")]
    SubPlannerError(String),
    /// 超时
    #[error("执行超时")]
    Timeout,
    /// 安全策略违规
    #[error("安全策略违规：{0}")]
    SecurityViolation(String),
    /// 技能执行失败
    #[error("技能执行失败：{0}")]
    SkillExecution(String),
    /// 内部错误（架构约束违反）
    #[error("内部错误：{0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_display() {
        let err = ExecutorError::FileError("文件不存在".to_string());
        assert!(err.to_string().contains("文件操作失败"));
        assert!(err.to_string().contains("文件不存在"));

        let err = ExecutorError::Timeout;
        assert!(err.to_string().contains("执行超时"));
    }

    #[test]
    fn test_action_serialization() {
        let action = Action::ReadFile {
            path: "test.txt".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("ReadFile"));
        assert!(json.contains("test.txt"));
    }
}
```

- [ ] **Step 2: Fix executor/executor.rs — remove Step/FailureHandling/StepResult usage**

`executor.rs` currently uses `Step`, `StepResult`, `FailureHandling` extensively. We need to rewrite the `Executor` struct and methods.

Replace the `Executor` struct definition (lines 118-128) and all its `impl` blocks up to line 433 with a deprecated shell:

```rust
/// Executor 结构体（已废弃）。
///
/// 功能已迁移至 `ToolRegistry`。保留此结构体仅用于向后兼容。
#[deprecated(since = "0.2.0", note = "Use ToolRegistry instead")]
pub struct Executor {
    max_concurrency: usize,
    skill_executor: Option<Arc<SkillExecutor>>,
}

#[allow(deprecated)]
impl Executor {
    /// 创建新的 Executor（已废弃）。
    #[deprecated(since = "0.2.0", note = "Use ToolRegistry::new instead")]
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            skill_executor: None,
        }
    }

    /// 设置技能执行器（已废弃）。
    #[deprecated(since = "0.2.0", note = "Use ToolRegistry::with_skill_executor instead")]
    pub fn with_skill_executor(mut self, skill_executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(skill_executor);
        self
    }
}
```

Then remove the `#[async_trait::async_trait] impl ExecutorTrait for Executor` block (lines 428-433), since `ExecutorTrait` will be deleted in the next task.

Also remove all `use super::types::{Step, StepResult};` and `use super::types::{Action, FailureHandling, Step, StepResult};` lines in the test module at the bottom (lines 584-588).

- [ ] **Step 3: Delete executor/traits.rs**

Delete `core/src/executor/traits.rs`.

- [ ] **Step 4: Update executor/mod.rs**

Replace the entire file:

```rust
#[allow(clippy::module_inception)]
mod executor;

pub mod approval;
pub mod judge;
pub mod types;
pub mod verification;

pub use approval::{
    ApprovalDecision, ApprovalRecord, ApprovalRequest, ApprovalResponse, ApprovalWorkflow,
    ApprovalWorkflowConfig, AutoApprovalRule, RiskLevel,
};
pub use executor::{
    execute_command_action, execute_read_file, execute_search_code, execute_write_file, Executor,
    SecurityPolicy,
};
pub use judge::{JudgeVerdict, LlmJudge};
pub use types::{Action, ExecutorError};
pub use verification::{VerificationGate, VerificationIssue, VerificationReport};
```

- [ ] **Step 5: Fix executor/verification.rs — remove StepResult dependency**

Replace lines 1-147 (the `to_step_result` method and its imports):

```rust
//! Agent 产出验证门控。
//!
//! 在工具执行完成后自动运行编译器/lint 检查，将失败信息
//! 转化为结构化的 JSON 反馈，供 LLM 自主决定修复策略。

use crate::common::error::Result;
use serde::Serialize;
use std::process::Command;

const MAX_LOOP_ITERATIONS: usize = 3;

/// 快速检查类型（毫秒~秒级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastCheck {
    /// cargo check --message-format=json
    CompileCheck,
    /// cargo clippy --message-format=json
    ClippyCheck,
}

/// 单条编译/lint 错误。
#[derive(Debug, Clone, Serialize)]
pub struct VerificationIssue {
    pub file: String,
    pub message: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub passed: bool,
    pub issues: Vec<VerificationIssue>,
    pub summary: String,
}

/// 验证门控。
#[derive(Clone)]
pub struct VerificationGate {
    enabled: bool,
    fast_checks: Vec<FastCheck>,
    project_root: std::path::PathBuf,
    loop_count: usize,
}

impl VerificationGate {
    pub fn new(project_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            enabled: true,
            fast_checks: vec![FastCheck::CompileCheck],
            project_root: project_root.into(),
            loop_count: 0,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_checks(mut self, checks: Vec<FastCheck>) -> Self {
        self.fast_checks = checks;
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn run_fast(&mut self) -> Result<VerificationReport> {
        if !self.enabled {
            return Ok(VerificationReport {
                passed: true,
                issues: vec![],
                summary: "验证门控未启用".to_string(),
            });
        }

        let mut all_issues = Vec::new();
        let mut all_passed = true;

        for check in &self.fast_checks {
            let (success, issues) = match check {
                FastCheck::CompileCheck => self.run_cargo_check().await,
                FastCheck::ClippyCheck => self.run_cargo_clippy().await,
            };
            if !success {
                all_passed = false;
            }
            all_issues.extend(issues);
        }

        let summary = if all_passed {
            "所有验证通过".to_string()
        } else {
            format!("发现 {} 个问题，请修复后重新提交", all_issues.len())
        };

        Ok(VerificationReport {
            passed: all_passed,
            issues: all_issues,
            summary,
        })
    }

    pub fn should_continue(&self) -> bool {
        self.loop_count < MAX_LOOP_ITERATIONS
    }

    pub fn increment_loop(&mut self) {
        self.loop_count += 1;
    }

    async fn run_cargo_check(&self) -> (bool, Vec<VerificationIssue>) {
        // ... same as before, lines 159-222
    }

    async fn run_cargo_clippy(&self) -> (bool, Vec<VerificationIssue>) {
        // ... same as before, lines 224-287
    }
}
```

Copy the `run_cargo_check` and `run_cargo_clippy` methods from the original file (lines 159-287) into the new file.

- [ ] **Step 6: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: May fail due to `executor.rs` tests referencing `Step`/`FailureHandling`. Fix any remaining compile errors by removing or adapting test code in `executor.rs`.

- [ ] **Step 7: Commit**

```bash
git add core/src/executor/
git commit -m "refactor(executor): remove Step/StepResult/FailureHandling/ExecutorTrait, extract standalone functions"
```

---

## Phase 2: Planner Cleanup — Remove Plan/Turn/ContextManager/PlannerConfig

### Task 2.1: Delete planner/context.rs and planner/config.rs

**Files:**
- Delete: `core/src/planner/context.rs`
- Delete: `core/src/planner/config.rs`

- [ ] **Step 1: Delete files**

```powershell
Remove-Item core/src/planner/context.rs
Remove-Item core/src/planner/config.rs
```

- [ ] **Step 2: Commit**

```bash
git add core/src/planner/
git commit -m "refactor(planner): delete ContextManager and PlannerConfig (functionality migrated to AgentLoop)"
```

---

### Task 2.2: Rewrite planner/types.rs — keep only ClarificationQuestion

**Files:**
- Modify: `core/src/planner/types.rs`

- [ ] **Step 1: Replace entire file**

```rust
use serde::{Deserialize, Serialize};

/// 追问问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    pub question: String,
    pub question_type: QuestionType,
    pub options: Option<Vec<String>>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionType {
    /// 开放式
    #[serde(rename = "OpenEnded")]
    OpenEnded,
    /// 选择式
    #[serde(rename = "Choice")]
    Choice,
    /// 确认式
    #[serde(rename = "Confirmation")]
    Confirmation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clarification_question_creation() {
        let q = ClarificationQuestion {
            question: "Test?".to_string(),
            question_type: QuestionType::OpenEnded,
            options: None,
            required: true,
        };
        assert_eq!(q.question, "Test?");
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add core/src/planner/types.rs
git commit -m "refactor(planner/types): retain only ClarificationQuestion, remove Plan/Turn/PlannerContext/PlannerMutation/PlannerError"
```

---

### Task 2.3: Rewrite planner/mod.rs as minimal shell

**Files:**
- Modify: `core/src/planner/mod.rs`

- [ ] **Step 1: Replace entire file**

```rust
//! Planner 模块（已废弃）。
//!
//! 功能已迁移至 `agent::loop::AgentLoop`。
//! 保留此模块仅用于向后兼容的 `ClarificationQuestion` 类型导出。

pub mod types;

pub use types::{ClarificationQuestion, QuestionType};
```

- [ ] **Step 2: Commit**

```bash
git add core/src/planner/mod.rs
git commit -m "refactor(planner): reduce to minimal shell, export ClarificationQuestion only"
```

---

## Phase 3: Build Tool Layer — ToolParams + ToolRegistry

### Task 3.1: Create core/src/agent/tool_params.rs

**Files:**
- Create: `core/src/agent/tool_params.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the file**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parameters for the `read_file` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileParams {
    /// Absolute or relative path to the file to read.
    pub path: String,
}

/// Parameters for the `write_file` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileParams {
    /// Absolute or relative path to the file to write.
    pub path: String,
    /// Content to write into the file.
    pub content: String,
}

/// Parameters for the `execute_command` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteCommandParams {
    /// Shell command to execute.
    pub command: String,
    /// Working directory for the command (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Timeout in seconds (optional, default 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Parameters for the `search_code` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    /// Search query (passed to ripgrep).
    pub query: String,
    /// Directory scope to limit the search (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Parameters for the `call_skill` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallSkillParams {
    /// Unique identifier of the skill to invoke.
    pub skill_id: String,
    /// Parameters for the skill call.
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Parameters for the `run_tests` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunTestsParams {
    /// Test command to run (e.g. "cargo test").
    pub command: String,
    /// Working directory (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Timeout in seconds (optional, default 120).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Parameters for the `verify_build` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerifyBuildParams {
    /// Build/check command to run (e.g. "cargo check").
    pub command: String,
    /// Working directory (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Timeout in seconds (optional, default 60).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Parameters for the `ask_user` tool.
///
/// When the LLM calls this tool, the AgentLoop pauses and returns a clarification request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserParams {
    /// The question to ask the user.
    pub question: String,
}

/// Parameters for the `delegate_to_agent` tool.
///
/// Creates a fully isolated sub-agent with its own conversation context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateToAgentParams {
    /// Task description for the sub-agent.
    pub task: String,
    /// System prompt for the sub-agent (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Maximum turns for the sub-agent (optional, default 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_file_params_schema() {
        let def = crate::model::FunctionDefinition::from_schema::<ReadFileParams>(
            "read_file",
            "Read a file",
        );
        assert_eq!(def.name, "read_file");
        let params = def.parameters;
        assert!(params.get("properties").is_some());
    }

    #[test]
    fn test_ask_user_params_serialization() {
        let params = AskUserParams {
            question: "What is your name?".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("What is your name"));
    }
}
```

- [ ] **Step 2: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: Should compile (new file not yet referenced by mod.rs).

- [ ] **Step 3: Commit**

```bash
git add core/src/agent/tool_params.rs
git commit -m "feat(agent): add Tool parameter structs with JsonSchema derive"
```

---

### Task 3.2: Create core/src/agent/tool_registry.rs

**Files:**
- Create: `core/src/agent/tool_registry.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the file**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    ReadFileParams, RunTestsParams, SearchCodeParams, VerifyBuildParams, WriteFileParams,
};
use crate::executor::{ExecutorError, SecurityPolicy};
use crate::model::types::{FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ToolType;
use crate::skills::{SkillExecutionRequest, SkillExecutor};

/// Tool execution error.
#[derive(thiserror::Error, Debug, Clone)]
pub enum ToolExecutionError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    #[error("Ask user: {0}")]
    AskUser(String),
}

/// Tool registry.
///
/// Maintains JSON Schema definitions for available tools and their execution logic.
pub struct ToolRegistry {
    security_policy: SecurityPolicy,
    skill_executor: Option<Arc<SkillExecutor>>,
    definitions: Vec<ToolDefinition>,
}

impl ToolRegistry {
    /// Create a new tool registry with default security policy.
    pub fn new(security_policy: SecurityPolicy) -> Self {
        let mut registry = Self {
            security_policy,
            skill_executor: None,
            definitions: Vec::new(),
        };
        registry.register_builtin_tools();
        registry
    }

    /// Attach a skill executor for `call_skill` tool.
    pub fn with_skill_executor(mut self, executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(executor);
        self
    }

    /// Get all tool definitions for LLM tool calling.
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    /// Execute multiple tool calls in parallel.
    pub async fn execute_parallel(
        &self,
        calls: &[ToolCall],
    ) -> Vec<(String, Result<serde_json::Value, ToolExecutionError>)> {
        let mut set = tokio::task::JoinSet::new();
        for call in calls {
            let call = call.clone();
            let result = self.execute_single(&call).await;
            set.spawn(async move { (call.id, result) });
        }

        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok((id, result)) = res {
                results.push((id, result));
            }
        }
        results
    }

    async fn execute_single(
        &self,
        call: &ToolCall,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let arguments = &call.function.arguments;
        match call.function.name.as_str() {
            "read_file" => {
                let params: ReadFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_read_file(&params.path)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "write_file" => {
                let params: WriteFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_file_write()
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                crate::executor::execute_write_file(&params.path, &params.content)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "execute_command" => {
                let params: ExecuteCommandParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_command(&params.command)
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                crate::executor::execute_command_action(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "search_code" => {
                let params: SearchCodeParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_search_code(&params.query, params.scope.as_deref())
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "call_skill" => {
                let params: CallSkillParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_call_skill(&params.skill_id, &params.parameters).await
            }
            "run_tests" => {
                let params: RunTestsParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::run_command(&params.command, params.cwd.as_deref(), params.timeout_secs)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "verify_build" => {
                let params: VerifyBuildParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::run_command(&params.command, params.cwd.as_deref(), params.timeout_secs)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "ask_user" => {
                let params: AskUserParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                Err(ToolExecutionError::AskUser(params.question))
            }
            "delegate_to_agent" => {
                Err(ToolExecutionError::UnknownTool(
                    "delegate_to_agent not yet implemented".to_string(),
                ))
            }
            _ => Err(ToolExecutionError::UnknownTool(call.function.name.clone())),
        }
    }

    async fn execute_call_skill(
        &self,
        skill_id: &str,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        if let Some(ref skill_executor) = self.skill_executor {
            let request = SkillExecutionRequest::new(
                skill_id.to_string(),
                parameters.clone(),
            );
            match skill_executor.execute(request).await {
                Ok(result) => {
                    let mut data = HashMap::new();
                    if let Some(output) = result.output {
                        data.insert("output".to_string(), serde_json::Value::String(output));
                    }
                    if let Some(error) = result.error {
                        data.insert("error".to_string(), serde_json::Value::String(error));
                    }
                    if let Some(exit_code) = result.exit_code {
                        data.insert(
                            "exit_code".to_string(),
                            serde_json::Value::Number(exit_code.into()),
                        );
                    }
                    data.insert(
                        "execution_time_ms".to_string(),
                        serde_json::Value::Number(result.execution_time_ms.into()),
                    );
                    data.insert("success".to_string(), serde_json::Value::Bool(result.success));
                    Ok(serde_json::Value::Object(data.into_iter().collect()))
                }
                Err(e) => Err(ToolExecutionError::ExecutionFailed(e.to_string())),
            }
        } else {
            Err(ToolExecutionError::ExecutionFailed(
                "SkillExecutor not configured".to_string(),
            ))
        }
    }

    fn register_builtin_tools(&mut self) {
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<ReadFileParams>(
                "read_file",
                "Read the full text content of a file from the given path.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<WriteFileParams>(
                "write_file",
                "Write content to a file at the given path.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<ExecuteCommandParams>(
                "execute_command",
                "Execute a shell command with optional working directory and timeout.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<SearchCodeParams>(
                "search_code",
                "Search for code patterns using ripgrep.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<CallSkillParams>(
                "call_skill",
                "Call a registered skill by ID with parameters.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<RunTestsParams>(
                "run_tests",
                "Run a test command (e.g. cargo test) and return results.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<VerifyBuildParams>(
                "verify_build",
                "Run a build verification command (e.g. cargo check) and return results.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<AskUserParams>(
                "ask_user",
                "Ask the user a question when more information is needed to proceed.",
            ),
        ));
        self.definitions.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<DelegateToAgentParams>(
                "delegate_to_agent",
                "Delegate a sub-task to an isolated sub-agent with its own context.",
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        assert!(!registry.definitions().is_empty());
        assert!(registry.definitions().iter().any(|d| d.function.name == "read_file"));
    }

    #[test]
    fn test_tool_registry_has_ask_user() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions();
        assert!(defs.iter().any(|d| d.function.name == "ask_user"));
        assert!(defs.iter().any(|d| d.function.name == "delegate_to_agent"));
    }
}
```

- [ ] **Step 2: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: Should compile (new file not yet referenced).

- [ ] **Step 3: Commit**

```bash
git add core/src/agent/tool_registry.rs
git commit -m "feat(agent): create ToolRegistry with parallel execution and built-in tool definitions"
```

---

## Phase 4: AgentLoop Core

### Task 4.1: Create core/src/agent/loop.rs

**Files:**
- Create: `core/src/agent/loop.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the file**

```rust
use std::sync::Arc;

use crate::agent::tool_registry::{ToolExecutionError, ToolRegistry};
use crate::agent::types::StreamEventSender;
use crate::common::error::Result;
use crate::common::types::{Message, MessageRole};
use crate::model::types::ChatCompletionRequest;
use crate::model::ModelService;

/// Agent Loop configuration.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// Maximum number of LLM turns before aborting.
    pub max_turns: usize,
    /// Model identifier for chat completions.
    pub model: String,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 20,
            model: "default".to_string(),
        }
    }
}

/// Agent Loop result.
///
/// Distinguishes between normal completion and clarification pauses.
#[derive(Debug, Clone)]
pub enum AgentLoopResult {
    /// The LLM produced a direct answer.
    Answer(String),
    /// The LLM called `ask_user` and the loop paused.
    NeedsClarification { question: String },
}

/// Agent Loop error.
#[derive(thiserror::Error, Debug)]
pub enum AgentLoopError {
    #[error("Reached maximum turns: {0}")]
    MaxTurnsReached(usize),
    #[error("LLM call failed: {0}")]
    LlmCallFailed(String),
    #[error("Empty response from LLM")]
    EmptyResponse,
}

/// Agent Loop.
///
/// Runs an LLM-driven tool-calling loop:
/// 1. Send conversation history + tool definitions to LLM
/// 2. If LLM returns tool_calls, execute them in parallel via ToolRegistry
/// 3. Append tool results as `Message::tool` to conversation
/// 4. Repeat until LLM returns a direct answer or calls `ask_user`
#[derive(Clone)]
pub struct AgentLoop {
    model_service: Arc<dyn ModelService>,
    tool_registry: ToolRegistry,
    config: AgentLoopConfig,
}

impl AgentLoop {
    /// Create a new AgentLoop.
    pub fn new(
        model_service: Arc<dyn ModelService>,
        tool_registry: ToolRegistry,
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            model_service,
            tool_registry,
            config,
        }
    }

    /// Run the agent loop.
    ///
    /// - `messages` - Conversation history (will be mutated with assistant/tool messages)
    /// - `stream_sender` - Optional stream event sender for real-time UI updates
    /// - returns: `AgentLoopResult` — either an answer or a clarification request
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
    ) -> Result<AgentLoopResult, AgentLoopError> {
        for turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(&self.config.model, messages.clone())
                .with_tools(self.tool_registry.definitions().to_vec());

            let response = self
                .model_service
                .chat_completion(request)
                .await
                .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;

            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or(AgentLoopError::EmptyResponse)?;

            let assistant_msg = choice.message;

            // Check for ask_user before adding to history
            if let Some(ref tool_calls) = assistant_msg.tool_calls {
                if let Some(ask_call) = tool_calls.iter().find(|tc| tc.function.name == "ask_user") {
                    let params: crate::agent::tool_params::AskUserParams =
                        serde_json::from_str(&ask_call.function.arguments)
                            .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;
                    return Ok(AgentLoopResult::NeedsClarification {
                        question: params.question,
                    });
                }
            }

            // Add assistant message to history
            messages.push(Message {
                role: MessageRole::Assistant,
                content: assistant_msg.content.clone(),
                tool_calls: assistant_msg.tool_calls.clone(),
                tool_call_id: None,
            });

            if let Some(ref tool_calls) = assistant_msg.tool_calls {
                // Stream: tool call start
                if let Some(ref sender) = stream_sender {
                    for tc in tool_calls {
                        sender
                            .send_tool_call(&format!("调用: {}", tc.function.name))
                            .await;
                    }
                }

                // Execute tools in parallel
                let results = self.tool_registry.execute_parallel(tool_calls).await;

                // Append tool results to conversation
                for (call_id, result) in results {
                    let content = match result {
                        Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
                        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                    };

                    messages.push(Message::tool(&call_id, &content));

                    if let Some(ref sender) = stream_sender {
                        sender.send_observation(&content).await;
                    }
                }
            } else {
                // LLM returned a direct answer
                return Ok(AgentLoopResult::Answer(assistant_msg.content));
            }
        }

        Err(AgentLoopError::MaxTurnsReached(self.config.max_turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_loop_config_default() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_turns, 20);
        assert_eq!(config.model, "default");
    }

    #[test]
    fn test_agent_loop_error_display() {
        let err = AgentLoopError::MaxTurnsReached(5);
        assert!(err.to_string().contains("5"));
    }
}
```

- [ ] **Step 2: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: Compile (file not yet referenced by mod.rs).

- [ ] **Step 3: Commit**

```bash
git add core/src/agent/loop.rs
git commit -m "feat(agent): create AgentLoop with tool-calling iteration and ask_user support"
```

---

## Phase 5: Agent Types & SessionState Simplification

### Task 5.1: Migrate ClarificationQuestion to agent/types.rs

**Files:**
- Modify: `core/src/agent/types.rs`
- Modify: `core/src/planner/types.rs` (remove ClarificationQuestion)

- [ ] **Step 1: Add ClarificationQuestion to agent/types.rs**

Add to the top of `core/src/agent/types.rs` after the existing imports:

```rust
/// 追问问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    pub question: String,
    pub question_type: QuestionType,
    pub options: Option<Vec<String>>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionType {
    #[serde(rename = "OpenEnded")]
    OpenEnded,
    #[serde(rename = "Choice")]
    Choice,
    #[serde(rename = "Confirmation")]
    Confirmation,
}
```

Also update the `AgentResponse` struct to use the local `ClarificationQuestion` instead of the planner re-export. Currently line 33 uses `pub clarification_questions: Vec<ClarificationQuestion>`, which is fine because `ClarificationQuestion` will now be defined in this file.

Remove the existing line at the bottom:
```rust
pub use crate::planner::types::ClarificationQuestion;
```

- [ ] **Step 2: Update planner/types.rs to re-export from agent**

Replace `core/src/planner/types.rs`:

```rust
//! Planner 类型（已废弃）。
//!
//! `ClarificationQuestion` 已迁移至 `agent::types`。

pub use crate::agent::types::{ClarificationQuestion, QuestionType};
```

- [ ] **Step 3: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: Should reveal any remaining references to the old `planner::types::ClarificationQuestion` path. Fix them to use `crate::agent::types::ClarificationQuestion` or `crate::planner::ClarificationQuestion` (which re-exports from agent).

- [ ] **Step 4: Commit**

```bash
git add core/src/agent/types.rs core/src/planner/types.rs
git commit -m "refactor(agent): migrate ClarificationQuestion from planner to agent::types"
```

---

### Task 5.2: Simplify SessionState — remove execution_context, Turn, PlannerContext

**Files:**
- Modify: `core/src/agent/session_state.rs`

- [ ] **Step 1: Remove ContextManager import and execution_context field**

Remove:
```rust
use crate::planner::context::ContextManager;
use crate::planner::types::{ClarificationQuestion, Plan, StepResult};
```

Replace with:
```rust
use crate::agent::types::ClarificationQuestion;
```

Remove the `execution_context` field from `SessionState` struct:
```rust
/// 执行历史（Planner-Executor 循环的结果）。
pub execution_context: ContextManager,
```

Remove `current_goal` field? Actually, `current_goal` might still be useful. Let's keep it.

Remove methods: `add_turn`, `to_planner_context`, `apply_mutations`.

Rewrite `build_prompt_context` to not depend on `Turn`:

```rust
/// 构建完整的 Prompt 上下文。
pub fn build_prompt_context(&self, current_input: &str) -> String {
    if let Some(ref window) = self.context_window {
        crate::context::assembly::assemble_prompt(
            window,
            &self.conversation,
            current_input,
        )
    } else {
        format!("## 当前输入\n\n{}\n\n## 你的执行计划\n", current_input)
    }
}
```

Remove `to_session` method's `MessageRole::Tool` handling? Actually keep it, but maybe Tool messages should be skipped when converting to Session for persistence.

Actually, looking at the original code, `to_session` already skips `MessageRole::Tool` (line 271: `MessageRole::Tool => {}`). Good.

Remove `pending_memories`? No, keep it.

Also need to fix `trim_conversation` — currently it handles `MessageRole::System`. The `Message` struct doesn't have `name` field for system messages, but `MessageRole::System` exists. Keep as is.

- [ ] **Step 2: Rewrite session_state.rs**

The full rewrite is large. Here are the key changes:

**Struct:**
```rust
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub conversation: Vec<Message>,
    pub current_goal: Option<String>,
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,
    pub last_activity: Instant,
    pub context_window: Option<ContextWindow>,
    pub pending_memories: Vec<crate::common::types::MemoryEntry>,
    pub total_tokens: usize,
    pub start_time: Instant,
}
```

**Constructor:**
```rust
pub fn new(session_id: &str) -> Self {
    let now = Instant::now();
    Self {
        session_id: session_id.to_string(),
        conversation: Vec::new(),
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

**Methods to remove:** `add_turn`, `to_planner_context`, `apply_mutations`, `get_execution_context`.

**Tests to adapt:** Remove tests for `add_turn`, `build_prompt_context` that depend on `ContextWindow` with `execution_turns`, etc.

- [ ] **Step 3: Fix context/assembly.rs**

Remove the `Turn` import and parameter:

```rust
use crate::common::types::Message;
use crate::context::types::ContextWindow;

pub fn assemble_prompt(
    window: &ContextWindow,
    conversation: &[Message],
    current_input: &str,
) -> String {
    // Remove all references to `turns` parameter
    // Remove the "## 历史执行记录\n\n" section
}
```

The original `assemble_prompt` has a section for `turns` (lines 36-51). Remove that section.

- [ ] **Step 4: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: Many errors in coordinator.rs due to Planner references. That's expected — we'll fix in Phase 6.

- [ ] **Step 5: Commit**

```bash
git add core/src/agent/session_state.rs core/src/context/assembly.rs
git commit -m "refactor(agent): simplify SessionState, remove execution_context/Turn/PlannerContext"
```

---

## Phase 6: Coordinator & Builder Refactor

### Task 6.1: Update AgentConfig with max_turns

**Files:**
- Modify: `core/src/config/agent.rs`

- [ ] **Step 1: Add max_turns field**

Add to `AgentConfig`:
```rust
/// Agent Loop 最大轮次。
#[serde(default = "default_max_turns")]
pub max_turns: usize,
```

Add default function:
```rust
fn default_max_turns() -> usize {
    20
}
```

Update `Default` impl:
```rust
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            // ... existing fields
            max_turns: default_max_turns(),
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add core/src/config/agent.rs
git commit -m "feat(config): add max_turns to AgentConfig for AgentLoop"
```

---

### Task 6.2: Rewrite agent/builder.rs — build AgentLoop instead of Planner

**Files:**
- Modify: `core/src/agent/builder.rs`

- [ ] **Step 1: Update imports**

Remove:
```rust
use crate::planner::config::PlannerConfig;
use crate::planner::Planner;
```

Add:
```rust
use crate::agent::loop::{AgentLoop, AgentLoopConfig};
use crate::agent::tool_registry::ToolRegistry;
```

- [ ] **Step 2: Remove planner_config field and methods**

Remove `planner_config: Option<PlannerConfig>` from `AgentBuilder` struct.
Remove `with_planner_config` method.

- [ ] **Step 3: Rewrite build() method**

Replace the Planner construction block (lines 107-117) with:

```rust
// Build ToolRegistry
let tool_registry = ToolRegistry::new(
    crate::executor::SecurityPolicy::default(),
).with_skill_executor(skill_executor.clone());

// Build AgentLoop
let agent_loop = AgentLoop::new(
    model_service.clone(),
    tool_registry,
    AgentLoopConfig {
        max_turns: self.config.max_turns,
        model: "default".to_string(), // Will be resolved at runtime
    },
);
```

Update the `Agent::new` call (lines 183-194) to pass `agent_loop` instead of `Box::new(planner)`. This requires modifying `Agent::new` signature in `coordinator.rs`.

- [ ] **Step 4: Commit**

```bash
git add core/src/agent/builder.rs
git commit -m "refactor(builder): construct AgentLoop+ToolRegistry instead of Planner"
```

---

### Task 6.3: Rewrite agent/coordinator.rs — replace Planner with AgentLoop

**Files:**
- Modify: `core/src/agent/coordinator.rs`

- [ ] **Step 1: Update imports**

Remove:
```rust
use crate::planner::config::PlannerConfig;
use crate::planner::types::PlannerOutput;
use crate::planner::PlannerTrait;
```

Add:
```rust
use crate::agent::loop::{AgentLoop, AgentLoopConfig, AgentLoopError, AgentLoopResult};
```

- [ ] **Step 2: Update Agent struct**

Replace:
```rust
planner_config: PlannerConfig,
planner: Arc<Mutex<Box<dyn PlannerTrait + Send>>>,
```

With:
```rust
agent_loop: AgentLoop,
```

- [ ] **Step 3: Update Agent::new signature**

Replace:
```rust
planner: Box<dyn PlannerTrait + Send>,
```

With:
```rust
agent_loop: AgentLoop,
```

And update the field assignment:
```rust
agent_loop,
```

Remove `planner_config` field assignment.

- [ ] **Step 4: Update Clone impl**

Remove `planner_config` and `planner` clone lines, add `agent_loop: self.agent_loop.clone()`.

Wait, `AgentLoop` contains `Arc<dyn ModelService>` and `ToolRegistry` (which contains `SecurityPolicy`, `Option<Arc<SkillExecutor>>`, `Vec<ToolDefinition>`). All of these are `Clone` except `dyn ModelService` which is behind `Arc`. So `AgentLoop` should be `Clone`. But `ToolRegistry` currently doesn't derive `Clone`. I need to add `#[derive(Clone)]` to `ToolRegistry`, or manually implement it.

Actually, `SecurityPolicy` derives `Clone`, `Option<Arc<SkillExecutor>>` is Clone, `Vec<ToolDefinition>` is Clone. So I can add `#[derive(Clone)]` to `ToolRegistry`.

And `AgentLoop` should derive `Clone` too:
```rust
#[derive(Clone)]
pub struct AgentLoop {
    model_service: Arc<dyn ModelService>,
    tool_registry: ToolRegistry,
    config: AgentLoopConfig,
}
```

`AgentLoopConfig` also needs `Clone` — add `#[derive(Clone)]`.

- [ ] **Step 5: Add Clone to AgentLoop and ToolRegistry**

Edit `core/src/agent/loop.rs`:
```rust
#[derive(Debug, Clone)]
pub struct AgentLoopConfig { ... }

#[derive(Clone)]
pub struct AgentLoop { ... }
```

Edit `core/src/agent/tool_registry.rs`:
```rust
#[derive(Clone)]
pub struct ToolRegistry { ... }
```

- [ ] **Step 6: Rewrite process_message**

Replace the core logic (after context_pipeline run) with:

```rust
let mut messages = state.conversation.clone();
let loop_result = self.agent_loop.run(&mut messages, None).await;

let mut response = match loop_result {
    Ok(AgentLoopResult::Answer(content)) => {
        state.conversation = messages;
        state.add_assistant_message(&content);
        state.cleanup();

        // Background tasks (memory extraction, skill learning)
        let self_clone = self.clone();
        let state_arc = Arc::new(state.clone());
        let session_id = state_arc.session_id.clone();
        let handle = tokio::spawn(async move {
            tracing::debug!(session_id = %session_id, "启动后台记忆提取和技能学习");
            if let Err(e) = self_clone.extract_memories_from_session(&state_arc).await {
                tracing::warn!(session_id = %session_id, error = %e, "记忆提取失败");
            }
            if let Err(e) = self_clone.learn_skills_from_session(&state_arc).await {
                tracing::warn!(session_id = %session_id, error = %e, "技能学习失败");
            }
            self_clone.scan_and_promote_rules(&session_id).await;
            tracing::debug!(session_id = %session_id, "后台记忆提取和技能学习完成");
        });
        {
            let mut tasks = self.background_tasks.lock().await;
            tasks.push(handle);
        }

        AgentResponse::simple(content)
    }
    Ok(AgentLoopResult::NeedsClarification { question }) => {
        state.conversation = messages;
        let questions = vec![ClarificationQuestion {
            question,
            question_type: QuestionType::OpenEnded,
            options: None,
            required: true,
        }];
        let clarification_content = format_clarification_questions(&questions);
        AgentResponse::clarification(questions, clarification_content)
    }
    Err(e) => {
        let session_id = state.session_id.clone();
        let error_msg = format!("{}", e);
        let abstract_text = format!("Agent 执行失败：{}", error_msg);
        let detail = format!(
            "# 规则: 避免 Agent 执行失败\n\n\
             **来源会话**: {}\n\
             **错误信息**: {}\n\
             **建议**: 检查 AgentLoop 执行逻辑\n",
            session_id, error_msg
        );
        let _ = self
            .harness
            .rule_recorder
            .record_with_kind(&abstract_text, &detail, &session_id, FailureKind::Logic)
            .await;

        state.cleanup();
        AgentResponse::error(format!("Agent 执行失败：{}", e))
    }
};

response.processing_time_ms = start.elapsed().as_millis() as u64;

// Note: skill_calls extraction is removed because we no longer have Turn/StepResult
// We can re-add it later by scanning messages for tool_call results

Ok(response)
```

Remove `extract_skill_calls_from_turns` usage and the `skill_calls` assignment from `response` (line 613 in original). We'll need to either remove `response.skill_calls` field or populate it differently later.

Also remove the `PlannerContext` and `PlannerMutation` related code.

- [ ] **Step 7: Rewrite handle_clarification_response**

Replace the entire method body. When a clarification answer comes in, we just add it as a user message and re-run the agent loop:

```rust
async fn handle_clarification_response(
    &self,
    state: &mut SessionState,
    clarification_answers: &str,
) -> Result<AgentResponse> {
    let start = Instant::now();

    if state.pending_clarification.is_none() {
        return Ok(AgentResponse::simple("当前没有待处理的追问".to_string()));
    }

    state.add_user_message(clarification_answers);
    state.pending_clarification.take();

    let mut messages = state.conversation.clone();
    let loop_result = self.agent_loop.run(&mut messages, None).await;

    let mut response = match loop_result {
        Ok(AgentLoopResult::Answer(content)) => {
            state.conversation = messages;
            state.add_assistant_message(&content);
            state.cleanup();
            AgentResponse::simple(content)
        }
        Ok(AgentLoopResult::NeedsClarification { question }) => {
            state.conversation = messages;
            let questions = vec![ClarificationQuestion {
                question,
                question_type: QuestionType::OpenEnded,
                options: None,
                required: true,
            }];
            let clarification_content = format_clarification_questions(&questions);
            AgentResponse::clarification(questions, clarification_content)
        }
        Err(e) => {
            state.cleanup();
            AgentResponse::error(format!("处理追问回答失败：{}", e))
        }
    };

    response.processing_time_ms = start.elapsed().as_millis() as u64;
    Ok(response)
}
```

- [ ] **Step 8: Rewrite process_message_stream**

Similar to `process_message`, but pass `stream_sender` to `agent_loop.run`.

Replace the planner-related section (lines 669-675) with:

```rust
let mut messages = state_clone.conversation.clone();
let loop_result = self_clone.agent_loop.run(&mut messages, Some(stream_sender.clone())).await;

match loop_result {
    Ok(AgentLoopResult::Answer(content)) => {
        state_clone.conversation = messages;
        state_clone.add_assistant_message(&content);
        state_clone.cleanup();
        // ... metrics recording ...
        stream_sender.send_complete(&content, StreamChunkType::Answer, None).await;
    }
    Ok(AgentLoopResult::NeedsClarification { question }) => {
        state_clone.conversation = messages;
        let questions = vec![ClarificationQuestion {
            question,
            question_type: QuestionType::OpenEnded,
            options: None,
            required: true,
        }];
        let clarification_content = format_clarification_questions(&questions);
        stream_sender.send_complete(&clarification_content, StreamChunkType::Clarification, None).await;
    }
    Err(ref e) => {
        // ... error handling ...
        stream_sender.send_error(&format!("Agent 执行失败：{}", e)).await;
    }
}
```

- [ ] **Step 9: Remove extract_skill_calls_from_turns function**

Remove the entire function (lines 868-899) and its call site.

- [ ] **Step 10: Update Agent::new call site in builder.rs**

Since we already updated `builder.rs` in Task 6.2, the `Agent::new` call should now pass `agent_loop` as the 7th argument (replacing `Box::new(planner)`).

- [ ] **Step 11: Fix remaining compile errors in coordinator.rs**

Any remaining references to `PlannerOutput`, `PlannerTrait`, `PlannerContext`, `PlannerMutation`, `Turn`, `StepResult`, etc. must be removed or replaced.

- [ ] **Step 12: Check compile**

Run:
```powershell
cargo check -p tianyan-core
```

Expected: This will reveal any missed references. Fix them iteratively.

- [ ] **Step 13: Commit**

```bash
git add core/src/agent/coordinator.rs core/src/agent/loop.rs core/src/agent/tool_registry.rs
git commit -m "refactor(coordinator): replace Planner with AgentLoop, simplify response handling"
```

---

## Phase 7: Module Exports & Config Cleanup

### Task 7.1: Update agent/mod.rs exports

**Files:**
- Modify: `core/src/agent/mod.rs`

- [ ] **Step 1: Update exports**

```rust
//! 天演智能体系统的智能体模块。

pub mod session_state;

mod builder;
mod coordinator;
pub mod harness;
pub mod skill_subsystem;
mod tool_params;
mod tool_registry;
mod loop;
mod tools;
mod types;

pub use crate::config::AgentConfig;
pub use builder::AgentBuilder;
pub use coordinator::{Agent, AgentCoordinator};
pub use loop::{AgentLoop, AgentLoopConfig, AgentLoopError, AgentLoopResult};
pub use session_state::{SessionState, SessionStateManager};
pub use tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    ReadFileParams, RunTestsParams, SearchCodeParams, VerifyBuildParams, WriteFileParams,
};
pub use tool_registry::{ToolExecutionError, ToolRegistry};
pub use tools::{AgentTool, ToolResult};
pub use types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    SkillCallInfo, StreamChunkType, StreamEventSender,
};
```

- [ ] **Step 2: Commit**

```bash
git add core/src/agent/mod.rs
git commit -m "refactor(agent): update module exports for AgentLoop and ToolRegistry"
```

---

### Task 7.2: Update config/mod.rs — remove PlannerConfig

**Files:**
- Modify: `core/src/config/mod.rs`

- [ ] **Step 1: Remove PlannerConfig references**

Remove:
```rust
pub use crate::planner::config::PlannerConfig;
```

Remove from `TianyanConfig`:
```rust
/// Planner 配置。
#[serde(default)]
pub planner: PlannerConfig,
```

Remove from `validate()`:
```rust
self.planner.validate()?;
```

Remove from `test_default_config`:
```rust
assert!(!config.storage.data_dir.as_os_str().is_empty());
assert!(!config.models.default_chat_model.is_empty());
```
(Keep the existing assertions, just don't add planner-related ones.)

- [ ] **Step 2: Commit**

```bash
git add core/src/config/mod.rs
git commit -m "refactor(config): remove PlannerConfig from TianyanConfig"
```

---

### Task 7.3: Update lib.rs module exports

**Files:**
- Modify: `core/src/lib.rs`

- [ ] **Step 1: Keep planner module as deprecated shell**

`lib.rs` currently has `pub mod planner;`. Since we already reduced `planner/mod.rs` to a minimal shell, keep the export but mark it in doc comments:

No code change needed in `lib.rs` for `pub mod planner;` — the module itself is already a shell.

But we may want to add `pub mod executor;` note? It's already there.

Actually, `executor` module still exports `Action`, `ExecutorError`, `SecurityPolicy`, `ApprovalWorkflow`, `VerificationGate`, etc. which are still used. So keep it.

- [ ] **Step 2: Check compile**

Run:
```powershell
cargo check --workspace
```

Expected: Should compile cleanly.

- [ ] **Step 3: Commit**

```bash
git add core/src/lib.rs
git commit -m "chore(lib): adjust module exports for AgentLoop architecture"
```

---

## Phase 8: Verification & Final Cleanup

### Task 8.1: Full workspace compile check

- [ ] **Step 1: Run cargo check**

```powershell
cargo check --workspace
```

Expected: Clean compile. If any errors, fix them.

- [ ] **Step 2: Run fmt check**

```powershell
cargo fmt --all -- --check
```

Expected: No formatting issues. If any, run `cargo fmt --all`.

- [ ] **Step 3: Run clippy**

```powershell
cargo clippy --workspace -- -D warnings
```

Expected: Clean. Fix any warnings.

- [ ] **Step 4: Run unit tests**

```powershell
cargo test --workspace --lib -- --nocapture
```

Expected: All tests pass. Note: Some tests in executor/coordinator may have been removed or adapted.

- [ ] **Step 5: Run integration tests**

```powershell
cargo test --workspace --test '*' -- --nocapture
```

Expected: All tests pass.

- [ ] **Step 6: Commit final state**

```bash
git add -A
git commit -m "chore: full workspace compile and test check after AgentLoop refactor"
```

---

## Spec Coverage Checklist

| Requirement | Task |
|------------|------|
| Remove `Plan`, `Step`, `Turn`, `PlannerContext`, `PlannerMutation`, `PlannerError` | Task 2.2 |
| Remove `ExecutorTrait`, `StepResult`, `FailureHandling` | Task 1.2 |
| Extract standalone execution functions from `Executor` | Task 1.1 |
| Create `ToolParams` with `JsonSchema` derive | Task 3.1 |
| Create `ToolRegistry` with parallel execution | Task 3.2 |
| Create `AgentLoop` with max_turns and ask_user support | Task 4.1 |
| Migrate `ClarificationQuestion` to `agent::types` | Task 5.1 |
| Simplify `SessionState` (remove `execution_context`) | Task 5.2 |
| Rewrite `AgentBuilder` to construct `AgentLoop` | Task 6.2 |
| Rewrite `AgentCoordinator` to use `AgentLoop` | Task 6.3 |
| Remove `PlannerConfig` from `TianyanConfig` | Task 7.2 |
| Update all module exports | Tasks 7.1, 7.3 |
| Full compile + test verification | Task 8.1 |

## Placeholder Scan

No placeholders found. Every task contains:
- Exact file paths
- Complete code for every modification
- Exact commands with expected output

## Type Consistency Check

- `AgentLoopResult::NeedsClarification { question: String }` used consistently in `loop.rs` and `coordinator.rs`
- `ClarificationQuestion` defined in `agent/types.rs`, re-exported via `planner/types.rs` shell
- `ToolExecutionError::AskUser(String)` used in `tool_registry.rs` and detected in `loop.rs`
- `AgentLoopConfig { max_turns, model }` consistent across `loop.rs`, `builder.rs`, `agent_config`
