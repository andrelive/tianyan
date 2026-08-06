//! Agent 交互类工具执行器测试：execute_command / call_skill / ask_user /
//! self_check / delegate_to_agent。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::common::types::TokenUsage;
use crate::config::SafetyMode;
use crate::executor::SecurityPolicy;
use crate::model::types::{ChatChoice, ChatCompletionResponse};
use crate::model::MockChatService;
use crate::observability::AgentMetrics;
use crate::skills::{
    ExecutionContext, Skill, SkillExecutionResult, SkillExecutor, SkillHandler, SkillRegistry,
};

use super::*;

/// 默认严格策略：允许任意路径，但阻止常见破坏性命令。
fn default_strict_policy() -> SecurityPolicy {
    SecurityPolicy {
        safety_mode: SafetyMode::Strict,
        trash_directory: std::env::temp_dir(),
        allowed_commands: None,
        blocked_commands: vec![
            "rm".to_string(),
            "del".to_string(),
            "format".to_string(),
            "shutdown".to_string(),
            "taskkill".to_string(),
        ],
        allowed_directories: Vec::new(),
        blocked_directories: Vec::new(),
        allow_file_write: true,
        max_command_timeout_secs: 30,
        max_file_size: 1024 * 1024,
        block_interpreters: true,
    }
}

// ── execute_command ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_execute_command_success_captures_output() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"echo hello"}"#)
        .await
        .unwrap();
    assert!(result["stdout"].as_str().unwrap().contains("hello"));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_execute_command_rejects_blocked_command() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"rm foo.txt"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_command_rejects_shell_metacharacters() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"echo a && echo b"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_command_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_execute_command(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

// ── call_skill ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_call_skill_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_call_skill(r#"{"skill_id":"echo"}"#).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("SkillExecutor not configured"));
}

#[tokio::test]
async fn test_call_skill_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_call_skill(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_call_skill_success_with_mock_handler() {
    let mut skill_registry = SkillRegistry::new();
    skill_registry.register_with_handler(
        Skill::new("echo", "Echo", "Echo a message"),
        Arc::new(EchoSkillHandler),
    );
    let skill_executor = Arc::new(SkillExecutor::with_defaults(Arc::new(
        tokio::sync::RwLock::new(skill_registry),
    )));
    let registry = ToolRegistry::new(default_strict_policy()).with_skill_executor(skill_executor);

    let result = registry
        .execute_call_skill(r#"{"skill_id":"echo","parameters":{}}"#)
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["output"].as_str().unwrap(), "echo output");
}

/// 技能执行器的 Mock 处理器：返回固定输出。
struct EchoSkillHandler;

#[async_trait]
impl SkillHandler for EchoSkillHandler {
    async fn execute(
        &self,
        _params: HashMap<String, Value>,
        _context: ExecutionContext,
    ) -> crate::common::error::Result<SkillExecutionResult> {
        Ok(SkillExecutionResult::success("echo output"))
    }

    fn skill_id(&self) -> &str {
        "echo"
    }
}

// ── ask_user ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ask_user_returns_clarification() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_ask_user(r#"{"question":"Which file do you mean?"}"#)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("需要追问"));
    assert!(err.contains("Which file do you mean?"));
}

#[tokio::test]
async fn test_ask_user_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_ask_user(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

// ── self_check ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_self_check_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_self_check().await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("AgentMetrics not configured"));
}

#[tokio::test]
async fn test_self_check_success_with_metrics() {
    let registry = ToolRegistry::new(default_strict_policy()).with_metrics(AgentMetrics::new());
    let result = registry.execute_self_check().await.unwrap();
    assert!(result.is_object());
    assert_eq!(result["harness_version"], json!(1));
    assert_eq!(result["executions"].as_u64(), Some(0));
}

// ── delegate_to_agent ────────────────────────────────────────────────────

#[tokio::test]
async fn test_delegate_to_agent_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_delegate_to_agent(r#"{"task":"do something"}"#)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("ModelService not configured"));
}

#[tokio::test]
async fn test_delegate_to_agent_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_delegate_to_agent(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_delegate_to_agent_success() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| Ok(chat_response("final answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"summarize the notes"}"#)
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "final answer");
    assert_eq!(result["total_tokens"].as_u64(), Some(0));
}

/// 构造返回固定文本的 ChatCompletionResponse。
fn chat_response(content: &str) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "test".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: Message::assistant(content),
            finish_reason: Some("stop".to_string()),
        }],
        usage: TokenUsage::default(),
    }
}
