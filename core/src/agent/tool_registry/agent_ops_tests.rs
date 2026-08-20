//! Agent 交互类工具执行器测试：execute_command / call_skill / ask_user /
//! self_check / delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{AgentRole, RoleRegistry};
use crate::common::types::{FunctionCall, TokenUsage, ToolCall, ToolCallType};
use crate::config::AgentRolesConfig;
use crate::config::SafetyMode;
use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
use crate::executor::SecurityPolicy;
use crate::model::types::{ChatChoice, ChatCompletionResponse};
use crate::model::MockChatService;
use crate::observability::AgentMetrics;
use crate::skills::{
    ExecutionContext, Skill, SkillExecutionResult, SkillExecutor, SkillHandler, SkillRegistry,
};
use crate::vfs::VirtualFileSystem;

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
        allow_all_operations: false,
    }
}

// ── execute_command ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_execute_command_success_captures_output() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"echo hello"}"#, "test-session", false)
        .await
        .unwrap();
    assert!(result["stdout"].as_str().unwrap().contains("hello"));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_execute_command_rejects_blocked_command() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"rm foo.txt"}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_command_rejects_shell_metacharacters() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{"command":"echo a && echo b"}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_command_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_execute_command(r#"{}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_subagent_command_denied_without_hang() {
    // 回归保护：子任务（subagent=true）审批不交互——即使全局
    // wait_for_approval=true（主循环会挂起等面板），子任务也必须立即
    // 拒绝（timeout 包裹验证不挂起），由主 agent 在主对话确认后重试。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        unattended_mode: false,
        wait_for_approval: true,
        ..Default::default()
    }));
    let registry =
        ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow.clone());

    let fut =
        registry.execute_execute_command(r#"{"command":"cargo --version"}"#, "session-1", true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), fut)
        .await
        .expect("子任务审批必须立即返回，不得挂起等待");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("安全违规"), "子任务未授权操作应拒绝: {err}");
    assert!(
        err.contains("子任务操作需要主任务授权"),
        "拒绝原因应携带主任务授权标记: {err}"
    );
    // 审批请求不应进入待处理队列（不挂起）
    assert!(
        workflow.get_pending_approvals().await.is_empty(),
        "子任务审批不应产生挂起请求"
    );
}

#[tokio::test]
async fn test_subagent_command_approved_via_shared_fingerprint() {
    // 回归保护：主 agent 已确认过的操作（指纹共享），子任务直接执行——
    // 子 agent 是主 agent 意图的执行器，已授权范围内的操作零交互。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        unattended_mode: false,
        wait_for_approval: true,
        ..Default::default()
    }));
    // 主循环先确认过该操作（指纹记录）
    let action = Action::ExecuteCommand {
        command: "cargo --version".to_string(),
        cwd: None,
        timeout_secs: None,
    };
    workflow.record_user_confirmation(&action).await;

    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);

    let result = registry
        .execute_execute_command(r#"{"command":"cargo --version"}"#, "session-1", true)
        .await
        .unwrap();
    assert_eq!(result["exit_code"].as_i64(), Some(0));
    assert!(
        result["stdout"].as_str().unwrap().contains("cargo"),
        "子任务应执行已确认操作: {result:?}"
    );
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
    // 子代理上下文：返回指导性结果（携带原问题），而非错误字符串
    let value = result.expect("子代理 ask_user 应返回指导性结果");
    assert_eq!(value["status"], "delegated_agent_cannot_ask");
    assert_eq!(value["question"], "Which file do you mean?");
    assert!(
        value["hint"].as_str().unwrap().contains("无法向用户追问"),
        "应包含无法追问的提示"
    );
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
        .execute_delegate_to_agent(r#"{"task":"do something"}"#, "session-1")
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
    let result = registry
        .execute_delegate_to_agent(r#"{}"#, "session-1")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_delegate_to_agent_success() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| Ok(submit_response("final answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"summarize the notes"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "final answer");
    assert_eq!(result["submitted"].as_bool(), Some(true));
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

/// 构造子代理显式提交结果的响应（submit_result 工具调用）。
fn submit_response(result: &str) -> ChatCompletionResponse {
    chat_response_with_tools(vec![ToolCall {
        id: "s1".to_string(),
        call_type: ToolCallType::Function,
        function: FunctionCall {
            name: "submit_result".to_string(),
            arguments: format!(r#"{{"result":"{result}"}}"#),
        },
    }])
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
    // 第 2 轮：内层子循环显式提交结果
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(submit_response("inner done")));
    // 第 3 轮：外层子循环收到工具结果后显式提交最终结果
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(submit_response("outer done")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"outer task"}"#, "session-1")
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
        .execute_delegate_to_agent(r#"{"task":"too deep"}"#, "session-1")
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
        .execute_delegate_to_agent(r#"{"task":"loop forever","max_turns":2}"#, "session-1")
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
        .returning(|_| Ok(submit_response("quick answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"quick","timeout_secs":30}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "quick answer");
    assert_eq!(
        registry.delegation_depth.load(Ordering::SeqCst),
        0,
        "正常完成后深度应恢复为 0"
    );
}

#[tokio::test]
async fn test_delegate_plain_text_not_accepted_as_final() {
    // 未调用 submit_result 的纯文本输出不被接受为最终结果：
    // 第一轮输出文本 → 循环继续（注入提示）；第二轮调用 submit_result 才结束。
    use mockall::Sequence;
    let mut mock = MockChatService::new();
    let mut seq = Sequence::new();
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| Ok(chat_response("我先说说我的看法")));
    mock.expect_chat_completion()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| {
            // 第二轮请求应携带"未接受"提示（引导调用 submit_result）
            req.messages
                .iter()
                .any(|m| m.content.contains("没有被接受为最终结果"))
        })
        .returning(|_| Ok(submit_response("最终报告")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"审查"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "最终报告");
    assert_eq!(result["submitted"].as_bool(), Some(true));
}

// ── 角色化委托（delegate role / model）───────────────────────────────────

/// 构造覆盖 researcher 内置角色的注册表（仅保留注入的字段）。
fn role_registry_with(model: Option<&str>, tools: Option<Vec<&str>>) -> Arc<RoleRegistry> {
    let mut roles = HashMap::new();
    roles.insert(
        "researcher".to_string(),
        AgentRole {
            name: "researcher".to_string(),
            model: model.map(str::to_string),
            system_prompt: None,
            tools: tools.map(|ts| ts.into_iter().map(str::to_string).collect()),
            max_turns: None,
            timeout_secs: None,
            ..Default::default()
        },
    );
    Arc::new(RoleRegistry::from_config(&AgentRolesConfig { roles }))
}

/// 构造任意工具调用。
fn tool_call(id: &str, name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        call_type: ToolCallType::Function,
        function: FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        },
    }
}

#[tokio::test]
async fn test_delegate_role_model_used() {
    // role="researcher"（配置覆盖 model）：子 Agent 请求使用角色模型
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .withf(|req: &ChatCompletionRequest| req.model == "role-model")
        .returning(|_| Ok(submit_response("researched")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(role_registry_with(Some("role-model"), None));

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"research x","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "researched");
    assert_eq!(
        registry.delegation_depth.load(Ordering::SeqCst),
        0,
        "委托结束后深度应恢复为 0"
    );
}

#[tokio::test]
async fn test_delegate_no_role_uses_self_model() {
    // 无 role：请求模型回落主 Agent 模型（self.model）
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .withf(|req: &ChatCompletionRequest| req.model == "main-model")
        .returning(|_| Ok(submit_response("answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"t"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "answer");
}

#[tokio::test]
async fn test_delegate_explicit_model_beats_role_model() {
    // 显式 model 参数优先级最高（> role.model > self.model）
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .withf(|req: &ChatCompletionRequest| req.model == "explicit-model")
        .returning(|_| Ok(submit_response("answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(role_registry_with(Some("role-model"), None));

    let result = registry
        .execute_delegate_to_agent(
            r#"{"task":"t","role":"researcher","model":"explicit-model"}"#,
            "session-1",
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "answer");
}

#[tokio::test]
async fn test_delegate_role_system_prompt_injected() {
    // 角色系统提示注入子 Agent 请求（无显式 system_prompt 时）
    let mut roles = HashMap::new();
    roles.insert(
        "researcher".to_string(),
        AgentRole {
            name: "researcher".to_string(),
            model: None,
            system_prompt: Some("你是检索调研助手。只调研不改文件。".to_string()),
            tools: None,
            max_turns: None,
            timeout_secs: None,
            ..Default::default()
        },
    );
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .withf(|req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("检索调研助手"))
        })
        .returning(|_| Ok(submit_response("answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"t","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "answer");
}

#[tokio::test]
async fn test_delegate_explicit_system_prompt_beats_role() {
    // 显式 system_prompt 优先于角色系统提示
    let mut roles = HashMap::new();
    roles.insert(
        "researcher".to_string(),
        AgentRole {
            name: "researcher".to_string(),
            model: None,
            system_prompt: Some("角色提示".to_string()),
            tools: None,
            max_turns: None,
            timeout_secs: None,
            ..Default::default()
        },
    );
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .withf(|req: &ChatCompletionRequest| {
            req.messages.iter().any(|m| m.content.contains("显式提示"))
                && !req.messages.iter().any(|m| m.content.contains("角色提示"))
        })
        .returning(|_| Ok(submit_response("answer")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));

    let result = registry
        .execute_delegate_to_agent(
            r#"{"task":"t","role":"researcher","system_prompt":"显式提示"}"#,
            "session-1",
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "answer");
}

#[tokio::test]
async fn test_delegate_one_shot_clean_context_every_time() {
    // 一次性子代理：每次委托都是干净上下文——第二次委托不包含首次任务
    // 内容（不加载角色历史，防止旧上下文污染导致子代理跑偏）
    let mut roles = HashMap::new();
    roles.insert(
        "researcher".to_string(),
        AgentRole {
            name: "researcher".to_string(),
            system_prompt: Some("你是检索助手。".to_string()),
            ..Default::default()
        },
    );
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(1)
        .returning(|_| Ok(submit_response("answer1")));
    mock.expect_chat_completion()
        .times(1)
        .withf(|req: &ChatCompletionRequest| {
            // 干净上下文：不含首次任务 t1，仅含当前任务 t2 与角色系统提示
            !req.messages.iter().any(|m| m.content.contains("t1"))
                && req.messages.iter().any(|m| m.content.contains("t2"))
                && req
                    .messages
                    .iter()
                    .any(|m| m.content.contains("你是检索助手"))
        })
        .returning(|_| Ok(submit_response("answer2")));

    let (vfs, _dir) = real_vfs().await;
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_vfs(vfs)
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));

    // 第一次委托
    let r1 = registry
        .execute_delegate_to_agent(r#"{"task":"t1","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(r1["result"].as_str().unwrap(), "answer1");
    assert!(
        r1.get("role_session").is_none(),
        "一次性子代理无会话语义信息"
    );

    // 第二次委托：干净上下文（无 t1 残留）
    let r2 = registry
        .execute_delegate_to_agent(r#"{"task":"t2","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(r2["result"].as_str().unwrap(), "answer2");
    assert!(r2.get("role_session").is_none());
}

#[tokio::test]
async fn test_delegate_role_filters_disallowed_tools() {
    // 角色白名单外的工具：不执行，合成错误结果回喂模型（循环继续，不硬失败）
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().times(1).returning(|_| {
        Ok(chat_response_with_tools(vec![tool_call(
            "c1",
            "write_file",
            r#"{"path":"x","content":"y"}"#,
        )]))
    });
    mock.expect_chat_completion()
        .times(1)
        .withf(|req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("不允许使用工具 write_file"))
        })
        .returning(|_| Ok(submit_response("fixed answer")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(role_registry_with(
            None,
            Some(vec!["read_file", "delegate_to_agent"]),
        ));

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"t","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "fixed answer");
}

#[tokio::test]
async fn test_delegate_role_allows_allowlisted_tool() {
    // 白名单内的工具正常执行：read_file 直接执行并回喂结果
    let mut mock = MockChatService::new();
    mock.expect_chat_completion().times(1).returning(|_| {
        Ok(chat_response_with_tools(vec![tool_call(
            "c1",
            "read_file",
            r#"{"path":"x"}"#,
        )]))
    });
    mock.expect_chat_completion()
        .times(1)
        .withf(|req: &ChatCompletionRequest| {
            req.messages.iter().any(|m| {
                m.tool_call_id.as_deref() == Some("c1") && !m.content.contains("不允许使用工具")
            })
        })
        .returning(|_| Ok(submit_response("done")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(role_registry_with(None, Some(vec!["read_file"])));

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"t","role":"researcher"}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "done");
}

#[tokio::test]
async fn test_delegate_unknown_role_errors_with_available_roles() {
    // 未知角色：循环启动前报错，错误消息列出可用角色（模型可重试）
    let mock = MockChatService::new();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model");

    let err = registry
        .execute_delegate_to_agent(r#"{"task":"t","role":"ghost"}"#, "session-1")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("角色不存在：ghost"), "应报未知角色: {err}");
    assert!(err.contains("researcher"), "应列出可用角色: {err}");
    assert!(
        err.contains("editor") && err.contains("reviewer"),
        "应列出可用角色: {err}"
    );
}

#[tokio::test]
async fn test_delegate_background_role_applies() {
    // 后台委托同样应用角色：角色模型 + 白名单过滤（与前台共用循环）
    use crate::agent::background::TaskStatus;

    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .times(1)
        .withf(|req: &ChatCompletionRequest| req.model == "role-model")
        .returning(|_| {
            Ok(chat_response_with_tools(vec![tool_call(
                "c1",
                "write_file",
                r#"{"path":"x","content":"y"}"#,
            )]))
        });
    mock.expect_chat_completion()
        .times(1)
        .withf(|req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("不允许使用工具"))
        })
        .returning(|_| Ok(submit_response("bg done")));

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("main-model")
        .with_role_registry(role_registry_with(
            Some("role-model"),
            Some(vec!["read_file", "delegate_to_agent"]),
        ));

    let result = registry
        .execute_delegate_to_agent(
            r#"{"task":"bg role job","background":true,"role":"researcher"}"#,
            "session-1",
        )
        .await
        .unwrap();
    assert_eq!(result["status"].as_str(), Some("running"));
    let task_id = result["task_id"].as_str().unwrap().to_string();

    // 等待后台任务完成（tokio::spawn 异步执行）
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Some(task) = registry.background_tasks.get(&task_id).await {
            if task.status.is_terminal() {
                break;
            }
        }
    }

    let task = registry.background_tasks.get(&task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(
        task.result.as_deref().unwrap_or("").contains("bg done"),
        "结果应包含委托输出: {:?}",
        task.result
    );
}

// ── 后台任务（delegate background / task_status / task_cancel）──────────

#[tokio::test]
async fn test_delegate_background_starts_and_completes() {
    use crate::agent::background::TaskStatus;

    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| Ok(submit_response("bg done")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(
            r#"{"task":"background job","background":true}"#,
            "session-1",
        )
        .await
        .unwrap();
    assert_eq!(result["status"].as_str(), Some("running"));
    let task_id = result["task_id"].as_str().unwrap().to_string();
    assert!(
        task_id.starts_with("bt_"),
        "任务 ID 应为 bt_ 前缀: {task_id}"
    );

    // 等待后台任务完成（tokio::spawn 异步执行）
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Some(task) = registry.background_tasks.get(&task_id).await {
            if task.status.is_terminal() {
                break;
            }
        }
    }

    let task = registry.background_tasks.get(&task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(
        task.result.as_deref().unwrap_or("").contains("bg done"),
        "结果应包含委托输出: {:?}",
        task.result
    );
    assert_eq!(task.parent_session_id, "session-1");

    // task_status 工具查询
    let status = registry
        .execute_task_status(&format!(r#"{{"task_id":"{task_id}"}}"#))
        .await
        .unwrap();
    assert_eq!(status["status"].as_str(), Some("completed"));
}

// 多线程 flavor：mock 内 thread::sleep 只阻塞当前 worker，
// select 的预算 sleep 在另一 worker 正常触发（current_thread 下会互相饿死）。
#[tokio::test(flavor = "multi_thread")]
async fn test_delegate_sync_budget_promotes_to_background() {
    // 同步委托超预算：子代理未完成 → 自动升级为后台任务（不杀死子代理），
    // 返回 promoted_to_background + task_id。
    use crate::agent::background::TaskStatus;

    let mut mock = MockChatService::new();
    // 子代理永不完成（每轮输出开场白不提交），每轮 mock 阻塞 20ms 放慢循环
    // （200 轮 ≈ 4s > 1s 预算，保证升级在 max_turns 前触发）。
    mock.expect_chat_completion().returning(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        Ok(chat_response("我先看看"))
    });

    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"slow job","timeout_secs":1}"#, "session-1")
        .await
        .unwrap();
    assert_eq!(
        result["status"].as_str(),
        Some("promoted_to_background"),
        "预算耗尽应升级为后台: {result}",
    );
    let task_id = result["task_id"].as_str().unwrap().to_string();
    assert!(
        task_id.starts_with("bt_"),
        "任务 ID 应为 bt_ 前缀: {task_id}"
    );

    // 后台任务已注册且处于运行态（子代理循环已脱离调用栈继续跑）
    let task = registry.background_tasks.get(&task_id).await;
    assert!(task.is_some(), "升级后任务应注册到后台管理器");
    let task = task.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.parent_session_id, "session-1");
    // 取消后台任务，终止无限循环（测试收尾）
    registry.background_tasks.cancel(&task_id).await.unwrap();
}

#[tokio::test]
async fn test_task_cancel_via_tool() {
    use crate::agent::background::{TaskKind, TaskStatus};

    let registry = ToolRegistry::new(default_strict_policy());
    let id = registry
        .background_tasks
        .register(TaskKind::Delegate, "task".to_string(), "s1".to_string())
        .await;
    registry.background_tasks.mark_running(&id).await;

    let result = registry
        .execute_task_cancel(&format!(r#"{{"task_id":"{id}"}}"#))
        .await
        .unwrap();
    assert_eq!(result["status"].as_str(), Some("cancelled"));

    let task = registry.background_tasks.get(&id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn test_task_status_missing_task() {
    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_task_status(r#"{"task_id":"bt_nope"}"#)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("任务不存在"),
        "缺失任务应报错: {err}"
    );
}

#[tokio::test]
async fn test_background_assigns_correct_parent_session() {
    // 回归保护：后台任务归属由调用链显式传递的 session_id 决定（非共享可变
    // 状态）——多会话并发 turn 下，各任务挂到正确的父会话。
    let mut mock = MockChatService::new();
    mock.expect_chat_completion()
        .returning(|_| Ok(chat_response("x")));
    let registry = ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_model("test-model");

    // 不同会话各自发起后台委托（模拟并发 turn 的交错调用）
    let r1 = registry
        .execute_delegate_to_agent(r#"{"task":"bg-a","background":true}"#, "session-a")
        .await
        .unwrap();
    let r2 = registry
        .execute_delegate_to_agent(r#"{"task":"bg-b","background":true}"#, "session-b")
        .await
        .unwrap();

    let task_a = registry
        .background_tasks
        .get(r1["task_id"].as_str().unwrap())
        .await
        .unwrap();
    let task_b = registry
        .background_tasks
        .get(r2["task_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        task_a.parent_session_id, "session-a",
        "任务 A 应归属 session-a"
    );
    assert_eq!(
        task_b.parent_session_id, "session-b",
        "任务 B 应归属 session-b"
    );
}

/// 真实文件后端 + tempdir 的 VFS（角色会话存储用）。
async fn real_vfs() -> (Arc<dyn VirtualFileSystem>, tempfile::TempDir) {
    use crate::config::StorageConfig;
    use crate::vfs::backend::LocalFileBackend;
    use crate::vfs::{MockVectorStorage, VectorStorage, VfsCore, VirtualFileSystemImpl};

    let dir = tempfile::tempdir().unwrap();
    let config = StorageConfig {
        data_dir: dir.path().into(),
        ..Default::default()
    };
    let storage = Arc::new(LocalFileBackend::new(config.clone()));
    let vector_storage: Arc<dyn VectorStorage> = Arc::new(MockVectorStorage::new());
    let vfs = VirtualFileSystemImpl::new(storage, vector_storage, config);
    vfs.initialize().await.unwrap();
    (Arc::new(vfs), dir)
}
