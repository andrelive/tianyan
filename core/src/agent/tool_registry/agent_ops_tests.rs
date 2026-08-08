//! Agent 交互类工具执行器测试：execute_command / call_skill / ask_user /
//! self_check / delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::common::types::{FunctionCall, TokenUsage, ToolCall, ToolCallType};
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

/// 构造返回工具调用的 ChatCompletionResponse（content 为空）。
fn chat_response_with_tools(calls: Vec<ToolCall>) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "test".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: Message::assistant_with_tools("", calls),
            finish_reason: Some("tool_calls".to_string()),
        }],
        usage: TokenUsage::default(),
    }
}

fn delegate_call(id: &str, task: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        call_type: ToolCallType::Function,
        function: FunctionCall {
            name: "delegate_to_agent".to_string(),
            arguments: format!(r#"{{"task":"{task}"}}"#),
        },
    }
}

#[tokio::test]
async fn test_delegate_nested_delegation_executes_and_depth_released() {
    // 嵌套委托链路：外层 → 子 Agent → 内层委托（深度 2，允许），
    // 完成后委托深度应恢复为 0（guard 释放）。
    use mockall::Sequence;

    let mut mock = MockChatService::new();
    let mut seq = Sequence::new();
    // 第 1 轮：外层子循环收到嵌套委托请求
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            Ok(chat_response_with_tools(vec![delegate_call(
                "c1",
                "inner task",
            )]))
        });
    // 第 2 轮：内层委托子循环直接返回答案
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(chat_response("inner done")));
    // 第 3 轮：外层子循环收到工具结果后返回最终答案
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(chat_response("outer done")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"outer task"}"#)
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "outer done");
    assert_eq!(
        registry.delegation_depth.load(Ordering::SeqCst),
        0,
        "委托结束后深度应恢复为 0（guard 释放）"
    );
}

#[tokio::test]
async fn test_delegate_depth_limit_rejected() {
    // 深度到达上限时，新委托被拒绝，且深度回滚（不泄漏计数）。
    let mock = MockChatService::new();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");
    // 手动将深度推至上限（模拟已有 MAX 层委托在途）
    registry
        .delegation_depth
        .store(MAX_DELEGATION_DEPTH, Ordering::SeqCst);

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"too deep"}"#)
        .await;
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("委托深度超过上限"),
        "超限应被拒绝，实际: {err}"
    );
    assert_eq!(
        registry.delegation_depth.load(Ordering::SeqCst),
        MAX_DELEGATION_DEPTH,
        "拒绝后深度应回滚到上限值"
    );
}

#[tokio::test]
async fn test_delegate_max_turns_param_bounds_loop() {
    // max_turns=2：mock 始终返回工具调用（无最终答案），2 轮后返回超限消息。
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().times(2).returning(|_| {
        Ok(chat_response_with_tools(vec![ToolCall {
            id: "c1".to_string(),
            call_type: ToolCallType::Function,
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"x"}"#.to_string(),
            },
        }]))
    });
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"loop forever","max_turns":2}"#)
        .await
        .unwrap();
    assert!(
        result["result"]
            .as_str()
            .unwrap()
            .contains("reached max turns (2)"),
        "应报告到达 max_turns"
    );
}

#[tokio::test]
async fn test_delegate_timeout_param_accepted() {
    // timeout_secs 参数被接受：正常完成不受影响，深度恢复为 0。
    // （mock 无法真实挂起——mockall async 方法闭包同步返回，超时中断路径
    // 由 tokio::time::timeout 保证，此处覆盖参数解析与正常路径。）
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(1)
        .returning(|_| Ok(chat_response("quick answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"quick","timeout_secs":30}"#)
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "quick answer");
    assert_eq!(
        registry.delegation_depth.load(Ordering::SeqCst),
        0,
        "正常完成后深度应恢复为 0"
    );
}
