//! Agent 交互类工具执行器测试：execute_command / call_skill / ask_user /
//! self_check / delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::agent::{AgentRole, RoleRegistry};
use crate::common::types::{FunctionCall, StructuredMessage, TokenUsage, ToolCallType};
use crate::config::AgentRolesConfig;
use crate::config::{ApprovalMode, SafetyMode};
use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
use crate::executor::SecurityPolicy;
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChunkChoice, DeltaContent, ToolCallDelta,
    ToolCallFunctionDelta,
};
use crate::model::MockChatService;
use crate::observability::AgentMetrics;
use crate::vfs::VirtualFileSystem;

use super::*;

// ── 流式 mock 辅助（ADR-030：run_subagent_loop 用 run_stream） ──

/// 构造携带工具调用 delta 的流式 chunk。
fn stream_chunk_with_tools(tools: Vec<ToolCallDelta>) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chunk-t".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: DeltaContent {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: Some(tools),
            },
            finish_reason: None,
        }],
        usage: Some(TokenUsage::default()),
    }
}

/// 构造携带正文 + finish_reason 的流式 chunk（带 usage，避免 TokenEstimator 兜底）。
fn stream_chunk_finish(content: &str, finish_reason: &str) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chunk-f".to_string(),
        object: "chat.completion.chunk".to_string(),
        created: 0,
        model: "test".to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: DeltaContent {
                role: None,
                content: Some(content.to_string()),
                reasoning_content: None,
                tool_calls: None,
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
        usage: Some(TokenUsage::default()),
    }
}

/// mock 固定 chunk 序列的流式响应（每次调用重放同一序列）。
fn stream_mock(chunks: Vec<ChatCompletionChunk>) -> MockChatService {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(move |_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let chunks = chunks.clone();
        tokio::spawn(async move {
            for chunk in chunks {
                tx.send(Ok(chunk)).await.ok();
            }
        });
        Ok(rx)
    });
    mock
}

/// 两轮流式 mock（submit_result 降级）：第 1 轮 submit_result 工具调用
/// （执行器写入暂存结果），第 2 轮纯文本完成（无工具调用 = 收尾）。
fn stream_mock_submit(result: &str) -> MockChatService {
    use mockall::Sequence;
    let mut mock = MockChatService::new();
    let mut seq = Sequence::new();
    let result = result.to_string();
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(move |_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta(&result)])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("结果 ID：bt_test", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock
}

/// 两轮流式 mock（带请求谓词 withf）：第 1 轮 submit_result（withf 校验请求），
/// 第 2 轮纯文本完成。
fn stream_mock_submit_withf<F>(pred: F, result: &str) -> MockChatService
where
    F: Fn(&ChatCompletionRequest) -> bool + Send + Sync + 'static,
{
    use mockall::Sequence;
    let mut mock = MockChatService::new();
    let mut seq = Sequence::new();
    let result = result.to_string();
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(pred)
        .returning(move |_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta(&result)])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("结果 ID：bt_test", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock
}

/// 构造 submit_result 工具调用 delta。
fn submit_delta(result: &str) -> ToolCallDelta {
    ToolCallDelta {
        index: 0,
        id: Some("s1".to_string()),
        call_type: None,
        function: Some(ToolCallFunctionDelta {
            name: Some("submit_result".to_string()),
            arguments: Some(format!(r#"{{"result":"{result}"}}"#)),
        }),
    }
}

/// 构造任意工具调用 delta（read_file / write_file / delegate_to_agent 等）。
fn tool_delta(id: &str, name: &str, args: &str) -> ToolCallDelta {
    ToolCallDelta {
        index: 0,
        id: Some(id.to_string()),
        call_type: None,
        function: Some(ToolCallFunctionDelta {
            name: Some(name.to_string()),
            arguments: Some(args.to_string()),
        }),
    }
}

/// 会话管理器 mock（run_subagent_loop 落库用；ADR-030 统一循环框架）。
struct MockSessionManager;
#[async_trait]
impl crate::session::SessionManager for MockSessionManager {
    async fn add_structured_message(
        &self,
        _session_id: &str,
        _msg: StructuredMessage,
    ) -> crate::common::error::Result<()> {
        Ok(())
    }
    async fn rewrite_messages(
        &self,
        _session_id: &str,
        _messages: &[StructuredMessage],
    ) -> crate::common::error::Result<()> {
        Ok(())
    }
    async fn create_session(
        &self,
        _id: &str,
        _message: Message,
    ) -> crate::common::error::Result<crate::session::Session> {
        Ok(crate::session::Session::new(_id))
    }
    async fn get_session(
        &self,
        _id: &str,
    ) -> crate::common::error::Result<Option<crate::session::Session>> {
        Ok(None)
    }
    async fn update_session(
        &self,
        _session: &crate::session::Session,
    ) -> crate::common::error::Result<()> {
        Ok(())
    }
    async fn list_sessions(&self) -> crate::common::error::Result<Vec<crate::session::Session>> {
        Ok(vec![])
    }
    async fn delete_session(&self, _id: &str) -> crate::common::error::Result<()> {
        Ok(())
    }
}

/// 构造带会话管理器的委托测试注册表。
fn delegate_registry(mock: MockChatService) -> ToolRegistry {
    ToolRegistry::new(default_strict_policy())
        .with_model_service(Arc::new(mock))
        .with_session_manager(Arc::new(MockSessionManager))
        .with_model("test-model")
}

/// 注册委托任务并返回 task_id（submit_result 执行器 stage_result 需要任务存在）。
async fn register_delegate_task(registry: &ToolRegistry, desc: &str) -> String {
    registry
        .background_tasks
        .register(
            crate::agent::background::TaskKind::Delegate,
            desc.to_string(),
            "session-1".to_string(),
            0,
            None,
        )
        .await
}

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

// ── execute_command.ready 配置校验（静默降级 → 明确报错）────────────────

/// 由 JSON 构造 execute_command 参数。
fn command_params(json: &str) -> ExecuteCommandParams {
    serde_json::from_str(json).expect("参数应可解析")
}

#[test]
fn test_ready_spec_requires_background() {
    // 判别力：旧行为**静默忽略** ready——同步执行长驻服务会一路阻塞到命令超时
    let params = command_params(r#"{"command":"npm run dev","ready":{"port":3000}}"#);
    let err = build_ready_spec(&params).unwrap_err();
    assert!(err.is_invalid_input(), "{err}");
    assert!(err.to_string().contains("background"), "{err}");
}

#[test]
fn test_ready_spec_requires_port_or_pattern() {
    // 判别力：旧行为探测永远不满足，静默等到总超时（默认 5 分钟）才报未就绪
    for json in [
        r#"{"command":"npm run dev","background":true,"ready":{}}"#,
        r#"{"command":"npm run dev","background":true,"ready":{"pattern":"  "}}"#,
        r#"{"command":"npm run dev","background":true,"ready":{"port":null,"pattern":""}}"#,
    ] {
        let params = command_params(json);
        let err = build_ready_spec(&params).unwrap_err();
        assert!(err.is_invalid_input(), "{json} → {err}");
        assert!(
            err.to_string().contains("port 或 pattern"),
            "{json} → {err}"
        );
    }
}

#[test]
fn test_ready_spec_defaults_pattern_and_timeout() {
    let params = command_params(
        r#"{"command":"npm run dev","background":true,"ready":{"pattern":"Listening on"}}"#,
    );
    let spec = build_ready_spec(&params).unwrap().expect("应构造规格");
    assert_eq!(spec.pattern.as_deref(), Some("Listening on"));
    assert_eq!(spec.port, None);
    assert_eq!(spec.initial_delay_ms, 500);
    assert_eq!(
        spec.timeout_ms, 60_000,
        "缺省超时应为 60s（旧值 300s 让失败场景沉默 5 分钟）"
    );

    // 未配 ready → 不产生探测
    assert!(
        build_ready_spec(&command_params(r#"{"command":"echo hi"}"#))
            .unwrap()
            .is_none()
    );

    // port 与 pattern 可同时给（任一命中即就绪，双保险）；显式 timeout_ms 生效
    let params = command_params(
        r#"{"command":"npm run dev","background":true,"ready":{"port":5173,"pattern":"ready","timeout_ms":9000}}"#,
    );
    let spec = build_ready_spec(&params).unwrap().unwrap();
    assert_eq!(spec.port, Some(5173));
    assert_eq!(spec.pattern.as_deref(), Some("ready"));
    assert_eq!(spec.timeout_ms, 9_000);
}

#[tokio::test]
async fn test_execute_command_rejects_ready_without_background() {
    // 工具层端到端：校验在审批/执行之前完成——命令不得被拉起。
    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_execute_command(
            r#"{"command":"echo hi","ready":{"port":3000}}"#,
            "test-session",
            false,
        )
        .await
        .unwrap_err();
    assert!(err.is_invalid_input(), "{err}");
    assert!(err.to_string().contains("background"), "{err}");
}

#[tokio::test]
async fn test_subagent_command_denied_without_hang() {
    // 回归保护：子任务（subagent=true）审批不交互——即使交互模式
    // （主循环会挂起等面板），子任务也必须立即
    // 拒绝（timeout 包裹验证不挂起），由主 agent 在主对话确认后重试。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Interactive,
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
        mode: ApprovalMode::Interactive,
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
async fn test_call_skill_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_call_skill(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_call_skill_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_call_skill(r#"{"skill_id":"echo"}"#).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_call_skill_reads_vfs_content() {
    // call_skill = 读 VFS 技能文档（L0 摘要 + L2 详情），无执行语义。
    let vfs = Arc::new(crate::test_utils::MockVfs::new());
    let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["planning".to_string()]);
    vfs.set_content(&skill_uri, ContentLevel::Abstract, "计划阶段软约束");
    vfs.set_content(
        &skill_uri,
        ContentLevel::Detail,
        "# 计划阶段\n\n只读研究，输出结构化计划",
    );
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(vfs);

    let result = registry
        .execute_call_skill(r#"{"skill_id":"planning"}"#)
        .await
        .unwrap();
    assert_eq!(result["skill_id"].as_str().unwrap(), "planning");
    assert!(result["abstract"].as_str().unwrap().contains("计划阶段"));
    assert!(result["detail"].as_str().unwrap().contains("只读研究"));
}

#[tokio::test]
async fn test_call_skill_missing_skill_errors() {
    let vfs = Arc::new(crate::test_utils::MockVfs::new());
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(vfs);
    let result = registry
        .execute_call_skill(r#"{"skill_id":"not_exist"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("不存在"));
}

// ── ask_user ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ask_user_returns_clarification() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_ask_user(
            r#"{"questions":[{"question":"Which file do you mean?"}]}"#,
            "s1",
            true,
        )
        .await;
    // 子代理上下文：返回指导性结果（携带原问题），而非错误字符串
    let value = result.expect("子代理 ask_user 应返回指导性结果");
    assert_eq!(value["status"], "delegated_agent_cannot_ask");
    assert_eq!(value["questions"][0]["question"], "Which file do you mean?");
    assert!(
        value["hint"].as_str().unwrap().contains("无法向用户追问"),
        "应包含无法追问的提示"
    );
}

#[tokio::test]
async fn test_ask_user_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_ask_user(r#"{}"#, "s1", true).await;
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
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"do something"}"#).unwrap(),
            "session-1",
            "bt_test",
        )
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
    // ADR-030：submit_result 降级为普通工具——模型调用 submit_result
    // （执行器写入暂存结果），继续输出（告知主 agent 结果 ID），
    // 无工具调用 = 完成（暂存结果优先）
    let mock = stream_mock_submit("final answer");
    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "summarize the notes").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"summarize the notes"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "final answer");
    assert_eq!(result["submitted"].as_bool(), Some(true));
    assert_eq!(result["total_tokens"].as_u64(), Some(0));
}

#[tokio::test]
async fn test_delegate_nested_delegation_executes_and_depth_released() {
    // 嵌套委托链路：外层 → 子 Agent → 内层委托（层级 2，允许）；
    // 层级是注册表固有属性，父层不因子代理运行而改变（T0-8）。
    use mockall::Sequence;

    // 流式 mock：外层 3 轮 + 内层 2 轮（submit_result 降级后各需两轮：
    // 工具调用 + 纯文本完成）。内层 spawn 在外层第 1 轮 delegate 工具
    // 执行的 await 期间自运行到完成（原测试时序假设一致）。
    let mut mock = MockChatService::new();
    let mut seq = Sequence::new();
    // 第 1 轮：外层子循环收到嵌套委托请求（delegate_to_agent 工具调用）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![tool_delta(
                "c1",
                "delegate_to_agent",
                r#"{"task":"inner task"}"#,
            )])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 内层第 1 轮：submit_result（inner done）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("inner done")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 内层第 2 轮：纯文本完成（inner done）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("inner done", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 外层第 2 轮：submit_result（outer done）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("outer done")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 外层第 3 轮：纯文本完成（outer done）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("outer done", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });

    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "outer task").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"outer task"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "outer done");
    // 层级是注册表固有属性（T0-8：不是在途计数）——父层不因子代理运行而改变
    assert_eq!(
        registry.delegation_level, 0,
        "父注册表层级恒定（旧实现在此断言在途计数归零）"
    );
}

#[tokio::test]
async fn test_delegate_depth_limit_rejected_by_level() {
    // 层级到达上限时，新委托被拒绝（T0-8：按注册表层级判定，非在途计数）。
    let mock = MockChatService::new();
    let registry = delegate_registry(mock);
    // 模拟"已在 MAX 层"的子代理注册表
    let deepest = registry.child_registry(MAX_DELEGATION_DEPTH);

    let result = deepest
        .execute_delegate_to_agent(r#"{"task":"too deep"}"#, "session-1")
        .await;
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("委托深度超过上限"),
        "超限应被拒绝，实际: {err}"
    );
    // 拒绝不改变任何层级（层级不可变）：父层仍为 0、最深层仍为 MAX
    assert_eq!(registry.delegation_level, 0);
    assert_eq!(deepest.delegation_level, MAX_DELEGATION_DEPTH);
}

#[tokio::test]
async fn test_delegate_concurrent_siblings_not_rejected() {
    // T0-8（主回归）：**并发兄弟委托不共享深度**——同一父层连续发起的
    // MAX+1 个委托必须全部受理。
    //
    // 判别力：旧实现把"在途委托数"当深度（进入委托 `fetch_add`），第 4 个
    // 并发委托在途时计数 = 4 > MAX_DELEGATION_DEPTH → 被误拒（本会话亲历）。
    // mock 流永久挂起（forget tx）：任务保持"在途"，复现并发场景。
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        std::mem::forget(tx);
        Ok(rx)
    });
    let registry = delegate_registry(mock);

    for i in 0..=MAX_DELEGATION_DEPTH {
        let out = registry
            .execute_delegate_to_agent(&format!(r#"{{"task":"sibling {i}"}}"#), "session-1")
            .await
            .unwrap_or_else(|e| panic!("第 {} 个并发兄弟委托不应被拒：{e}", i + 1));
        assert_eq!(out["status"], "running");
    }
}

#[tokio::test]
async fn test_delegate_max_turns_param_bounds_loop() {
    // max_turns=2：mock 每轮返回 read_file 工具调用（永不完成），
    // 2 轮后返回 MaxTurnsReached（ADR-030：尽力而为，submitted=false）。
    let mock = stream_mock(vec![stream_chunk_with_tools(vec![tool_delta(
        "c1",
        "read_file",
        r#"{"path":"x"}"#,
    )])]);
    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "loop forever").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"loop forever","max_turns":2}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(
        result["submitted"].as_bool(),
        Some(false),
        "max_turns 上限应标记 submitted=false（尽力而为）"
    );
}
#[tokio::test]
async fn test_delegate_timeout_param_accepted() {
    // timeout_secs 参数被接受：正常完成不受影响，深度恢复为 0。
    // （mock 无法真实挂起——mockall async 方法闭包同步返回，超时中断路径
    // 由 tokio::time::timeout 保证，此处覆盖参数解析与正常路径。）
    let mock = stream_mock_submit("quick answer");
    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "quick").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"quick","timeout_secs":30}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "quick answer");
    // 旧实现在此断言"在途计数归零"；层级是注册表固有属性（T0-8）
    assert_eq!(registry.delegation_level, 0, "主层注册表层级恒为 0");
}

#[tokio::test]
async fn test_delegate_timeout_aborts_hung_subagent() {
    // 超时兜底守卫（ADR-026）：mock 流挂起（不产出 chunk 也不关闭通道），
    // timeout_secs=1 应中断并返回超时错误（watcher 标 fail + 通知主 agent）。
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // 挂起：forget tx 使通道保持打开但永不发送——rx.recv() 永久挂起
        std::mem::forget(tx);
        Ok(rx)
    });
    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "hung").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"hung","timeout_secs":1}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await;
    assert!(result.is_err(), "挂起子代理应超时失败");
    assert!(
        result.unwrap_err().to_string().contains("委托执行超时"),
        "错误应提示超时"
    );
}

/// T0-10（主回归）：子代理**超时**不得污染会话级取消标志。
///
/// 判别力：旧实现把会话槽里的 Arc 直接交给子代理并在超时时置位它——该 Arc
/// 同时是**父轮**的取消标志（coordinator 注入同一槽）→ 父轮下一步与后续
/// 委托被误取消。
#[tokio::test]
async fn test_delegate_timeout_does_not_pollute_shared_cancel() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        std::mem::forget(tx); // 挂起流：逼出超时路径
        Ok(rx)
    });
    let registry = delegate_registry(mock);
    // 会话共享标志 = 父轮"停止"标志的同一条 Arc
    let shared = Arc::new(AtomicBool::new(false));
    registry
        .set_delegation_cancel("session-1", Some(shared.clone()))
        .await;
    let task_id = register_delegate_task(&registry, "hung with timeout").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"hung with timeout","timeout_secs":1}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await;
    assert!(result.is_err(), "挂起子代理应超时失败");
    assert!(
        !shared.load(Ordering::Relaxed),
        "子代理超时不得置位会话共享取消标志（T0-10：会把父轮一并误取消）"
    );

    // 父轮后续委托不受影响（超时只停子代理自己）
    let next = registry
        .execute_delegate_to_agent(r#"{"task":"next sibling"}"#, "session-1")
        .await;
    assert!(next.is_ok(), "超时后父轮后续委托仍应受理：{:?}", next.err());
}

/// T0-10（镜像面）：用户"停止"（会话共享标志）仍必须传播到子代理。
///
/// 镜像任务（10ms 轮询）把共享标志同步到子代理本地标志——本测试防止
/// "修污染"时把用户停止语义一起丢掉。
#[tokio::test]
async fn test_delegate_user_stop_still_propagates_to_subagent() {
    let mut mock = MockChatService::new();
    mock.expect_chat_completion_stream().returning(|_| {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            // 稍后才产出 chunk：给镜像任务时间把共享标志同步到本地
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = tx.send(Ok(stream_chunk_finish("late", "stop"))).await;
        });
        Ok(rx)
    });
    let registry = delegate_registry(mock);
    // 用户已点"停止"（父轮取消标志置位）
    let shared = Arc::new(AtomicBool::new(true));
    registry
        .set_delegation_cancel("session-1", Some(shared))
        .await;
    let task_id = register_delegate_task(&registry, "stopped").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"stopped"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await;
    let err = result.expect_err("用户停止后子代理应被取消");
    assert!(
        err.to_string().contains("取消"),
        "错误应提示取消，实际: {err}"
    );
}

#[tokio::test]
async fn test_delegate_plain_text_is_final() {
    // ADR-030：收尾统一"无工具调用"——纯文本输出即完成（submitted=true），
    // 结果 = 模型输出（无 submit_result 暂存时）。
    let mock = stream_mock(vec![stream_chunk_finish("我先说说我的看法", "stop")]);
    let registry = delegate_registry(mock);
    let task_id = register_delegate_task(&registry, "审查").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"审查"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "我先说说我的看法");
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

#[tokio::test]
async fn test_delegate_role_model_used() {
    // role="researcher"（配置覆盖 model）：子 Agent 请求使用角色模型
    let mock = stream_mock_submit_withf(
        |req: &ChatCompletionRequest| req.model == "role-model",
        "researched",
    );
    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(role_registry_with(Some("role-model"), None));
    let task_id = register_delegate_task(&registry, "research x").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"research x","role":"researcher"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "researched");
    // 层级是注册表固有属性（T0-8）：委托结束后主层仍为 0
    assert_eq!(registry.delegation_level, 0, "主层注册表层级恒为 0");
}

#[tokio::test]
async fn test_delegate_no_role_uses_self_model() {
    // 无 role：请求模型回落主 Agent 模型（self.model）
    let mock = stream_mock_submit_withf(
        |req: &ChatCompletionRequest| req.model == "main-model",
        "answer",
    );
    let registry = delegate_registry(mock).with_model("main-model");
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "answer");
}

#[tokio::test]
async fn test_delegate_explicit_model_beats_role_model() {
    // 显式 model 参数优先级最高（> role.model > self.model）
    let mock = stream_mock_submit_withf(
        |req: &ChatCompletionRequest| req.model == "explicit-model",
        "answer",
    );
    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(role_registry_with(Some("role-model"), None));
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"researcher","model":"explicit-model"}"#)
                .unwrap(),
            "session-1",
            &task_id,
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
    let mock = stream_mock_submit_withf(
        |req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("检索调研助手"))
        },
        "answer",
    );
    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"researcher"}"#).unwrap(),
            "session-1",
            &task_id,
        )
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
    let mock = stream_mock_submit_withf(
        |req: &ChatCompletionRequest| {
            req.messages.iter().any(|m| m.content.contains("显式提示"))
                && !req.messages.iter().any(|m| m.content.contains("角色提示"))
        },
        "answer",
    );
    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"researcher","system_prompt":"显式提示"}"#)
                .unwrap(),
            "session-1",
            &task_id,
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
    // 第一次委托：任何请求 → submit + 完成
    let mut mock = MockChatService::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("answer1")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("answer1", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 第二次委托：干净上下文（无 t1 残留）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| {
            !req.messages.iter().any(|m| m.content.contains("t1"))
                && req.messages.iter().any(|m| m.content.contains("t2"))
                && req
                    .messages
                    .iter()
                    .any(|m| m.content.contains("你是检索助手"))
        })
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("answer2")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("answer2", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });

    let (vfs, _dir) = real_vfs().await;
    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_vfs(vfs)
        .with_role_registry(Arc::new(RoleRegistry::from_config(&AgentRolesConfig {
            roles,
        })));

    // 第一次委托
    let task1 = register_delegate_task(&registry, "t1").await;
    let r1 = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t1","role":"researcher"}"#).unwrap(),
            "session-1",
            &task1,
        )
        .await
        .unwrap();
    assert_eq!(r1["result"].as_str().unwrap(), "answer1");
    assert!(
        r1.get("role_session").is_none(),
        "一次性子代理无会话语义信息"
    );

    // 第二次委托：干净上下文（无 t1 残留）
    let task2 = register_delegate_task(&registry, "t2").await;
    let r2 = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t2","role":"researcher"}"#).unwrap(),
            "session-1",
            &task2,
        )
        .await
        .unwrap();
    assert_eq!(r2["result"].as_str().unwrap(), "answer2");
    assert!(r2.get("role_session").is_none());
}

#[tokio::test]
async fn test_delegate_role_filters_disallowed_tools() {
    // 角色白名单外的工具：不执行，合成错误结果回喂模型（循环继续，不硬失败）
    let mut mock = MockChatService::new();
    let mut seq = mockall::Sequence::new();
    // 第 1 轮：write_file 工具调用（白名单外 → 合成错误）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![tool_delta(
                "c1",
                "write_file",
                r#"{"path":"x","content":"y"}"#,
            )])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 第 2 轮：模型看到错误后 submit_result
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("不允许使用工具 write_file"))
        })
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("fixed answer")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 第 3 轮：纯文本完成
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("fixed answer", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });

    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(role_registry_with(
            None,
            Some(vec!["read_file", "delegate_to_agent"]),
        ));
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"researcher"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "fixed answer");
}

#[tokio::test]
async fn test_delegate_role_allows_allowlisted_tool() {
    // 白名单内的工具正常执行：read_file 直接执行并回喂结果
    let mut mock = MockChatService::new();
    let mut seq = mockall::Sequence::new();
    // 第 1 轮：read_file 工具调用（白名单内 → 执行）
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![tool_delta(
                "c1",
                "read_file",
                r#"{"path":"x"}"#,
            )])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 第 2 轮：模型看到工具结果后 submit_result
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| {
            req.messages.iter().any(|m| {
                m.tool_call_id.as_deref() == Some("c1") && !m.content.contains("不允许使用工具")
            })
        })
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("done")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    // 第 3 轮：纯文本完成
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("done", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });

    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(role_registry_with(None, Some(vec!["read_file"])));
    let task_id = register_delegate_task(&registry, "t").await;

    let result = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"researcher"}"#).unwrap(),
            "session-1",
            &task_id,
        )
        .await
        .unwrap();
    assert_eq!(result["result"].as_str().unwrap(), "done");
}

#[tokio::test]
async fn test_delegate_unknown_role_errors_with_available_roles() {
    // 未知角色：循环启动前报错，错误消息列出可用角色（模型可重试）
    let mock = MockChatService::new();
    let registry = delegate_registry(mock).with_model("main-model");

    let err = registry
        .run_subagent_loop(
            &serde_json::from_str(r#"{"task":"t","role":"ghost"}"#).unwrap(),
            "session-1",
            "bt_test",
        )
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

    // 流式 mock：第 1 轮 write_file（白名单外 → 合成错误），
    // 第 2 轮 submit_result（bg done），第 3 轮纯文本完成
    let mut mock = MockChatService::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| req.model == "role-model")
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![tool_delta(
                "c1",
                "write_file",
                r#"{"path":"x","content":"y"}"#,
            )])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|req: &ChatCompletionRequest| {
            req.messages
                .iter()
                .any(|m| m.content.contains("不允许使用工具"))
        })
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_with_tools(vec![submit_delta("bg done")])];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
    mock.expect_chat_completion_stream()
        .times(1)
        .in_sequence(&mut seq)
        .returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = vec![stream_chunk_finish("bg done", "stop")];
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });

    let registry = delegate_registry(mock)
        .with_model("main-model")
        .with_role_registry(role_registry_with(
            Some("role-model"),
            Some(vec!["read_file", "delegate_to_agent"]),
        ));

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"bg role job","role":"researcher"}"#, "session-1")
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

    let mock = stream_mock_submit("bg done");
    let registry = delegate_registry(mock);

    let result = registry
        .execute_delegate_to_agent(r#"{"task":"background job"}"#, "session-1")
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
        .execute_task_status(&format!(r#"{{"task_id":"{task_id}"}}"#), "s1")
        .await
        .unwrap();
    assert_eq!(status["status"].as_str(), Some("completed"));
}

// 注：timeout_secs 超时中断路径由 tokio::time::timeout 保证（代码层），
// mock 无法真实挂起（mockall async 方法闭包同步返回）——参数接受与正常
// 路径由 test_delegate_timeout_param_accepted 覆盖，此处不再重复。

#[tokio::test]
async fn test_task_cancel_via_tool() {
    use crate::agent::background::{TaskKind, TaskStatus};

    let registry = ToolRegistry::new(default_strict_policy());
    let id = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "task".to_string(),
            "s1".to_string(),
            0,
            None,
        )
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

/// 命令任务取消（回归保护）：task_cancel 必须路由到 CommandManager——
/// 此前只查委托表：命令任务被 BackgroundTaskManager 静默 no-op 却报「已取消」
/// （假成功，用户实测：返回成功但进程仍在跑）。
#[tokio::test]
async fn test_task_cancel_via_tool_command_task() {
    use crate::executor::CommandTaskStatus;

    let registry = ToolRegistry::new(default_strict_policy());
    // 起一个真实的跨平台短命令（`sleep` 别名 Windows/Unix 通用；
    // 30s 内必被 kill 或自然退出——测试失败时的残留也自愈）
    let task = registry
        .command_tasks
        .spawn_background("s1", "sleep 30", None, None, 0, None)
        .await
        .expect("后台命令应启动成功");
    assert_eq!(task.status, CommandTaskStatus::Running);

    let result = registry
        .execute_task_cancel(&format!(r#"{{"task_id":"{}"}}"#, task.id))
        .await
        .unwrap();
    assert_eq!(result["status"].as_str(), Some("cancelled"));

    // 关键回归：命令任务必须真正进入 Cancelled（旧实现只查委托表——
    // 状态仍 Running、进程继续跑）
    let after = registry.command_tasks.get(&task.id).await.unwrap();
    assert_eq!(
        after.status,
        CommandTaskStatus::Cancelled,
        "task_cancel 必须真正终止命令任务（旧实现：假成功、进程仍在跑）"
    );
}

/// 不存在的任务：必须明确报「未找到」——不再假成功
/// （旧实现：BackgroundTaskManager::cancel 对未知 id 静默 Ok → 报「已取消」）。
#[tokio::test]
async fn test_task_cancel_via_tool_unknown_id_not_found() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_task_cancel(r#"{"task_id":"cmd_999999"}"#)
        .await
        .unwrap();
    assert_eq!(
        result["status"].as_str(),
        Some("not_found"),
        "未知 id 应明确报未找到: {result}"
    );
}

#[tokio::test]
async fn test_task_status_list_mode_without_task_id() {
    // 回归保护：task_status 支持列表模式（task_id 缺省）——schema 与行为一致
    // （TaskStatusParams.task_id 为 Option，缺省列出全部任务 + kind 过滤）。
    use crate::agent::background::TaskKind;

    let registry = ToolRegistry::new(default_strict_policy());
    let id = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "task-a".to_string(),
            "s1".to_string(),
            0,
            None,
        )
        .await;
    registry.background_tasks.mark_running(&id).await;

    // 无 task_id：列表模式（含全部任务）
    let result = registry.execute_task_status("{}", "s1").await.unwrap();
    assert_eq!(
        result["total"].as_u64(),
        Some(1),
        "列表应包含已注册任务: {result}"
    );
    // kind 过滤：delegate 命中，command 为空
    let result = registry
        .execute_task_status(r#"{"kind":"delegate"}"#, "s1")
        .await
        .unwrap();
    assert_eq!(result["total"].as_u64(), Some(1));
    let result = registry
        .execute_task_status(r#"{"kind":"command"}"#, "s1")
        .await
        .unwrap();
    assert_eq!(result["total"].as_u64(), Some(0));
}
#[tokio::test]
async fn test_task_status_missing_task() {
    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_task_status(r#"{"task_id":"bt_nope"}"#, "s1")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("任务不存在"),
        "缺失任务应报错: {err}"
    );
}

#[tokio::test]
async fn test_task_status_running_includes_no_poll_note() {
    // U1 回归：非终态任务的单查询附"无需轮询"提醒（完成会自动通知）；
    // 终态后不再附（提醒只针对等待场景）。
    use crate::agent::background::TaskKind;

    let registry = ToolRegistry::new(default_strict_policy());
    let id = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "task".to_string(),
            "s1".to_string(),
            0,
            None,
        )
        .await;
    registry.background_tasks.mark_running(&id).await;

    // 运行中：附提醒
    let v = registry
        .execute_task_status(&format!(r#"{{"task_id":"{id}"}}"#), "s1")
        .await
        .unwrap();
    assert!(
        v["note"].as_str().is_some_and(|n| n.contains("无需轮询")),
        "非终态查询应附无需轮询提醒: {v}"
    );

    // 终态：不再附
    registry
        .background_tasks
        .complete(&id, "ok".to_string())
        .await;
    let v = registry
        .execute_task_status(&format!(r#"{{"task_id":"{id}"}}"#), "s1")
        .await
        .unwrap();
    assert!(v.get("note").is_none(), "终态查询不应附提醒: {v}");
}

#[tokio::test]
async fn test_task_status_workspace_scope_filters_other_directories() {
    // U5 回归：task_status 默认 scope=workspace——只显示**本工作目录**下
    // （含跨会话）的任务；其他目录的任务默认不查、不用、不提（防跨目录
    // 注意力偏移）；scope=global 显式查全局可见。归属未知（None——旧数据、
    // 解析失败）不构成"其他目录"证据，仍可见。
    use crate::agent::background::TaskKind;

    let registry = ToolRegistry::new(default_strict_policy());
    let here = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let elsewhere = "X:\\unrelated-other-dir";

    // A：本会话（s1）+ 本目录；B：他会话（s2）+ 其他目录；
    // C：他会话（s2）+ 本目录（同目录跨会话协调面）；D：归属未知（旧数据）。
    let id_a = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "A".to_string(),
            "s1".to_string(),
            0,
            Some(here.clone()),
        )
        .await;
    let id_b = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "B".to_string(),
            "s2".to_string(),
            0,
            Some(elsewhere.to_string()),
        )
        .await;
    let id_c = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "C".to_string(),
            "s2".to_string(),
            0,
            Some(here.clone()),
        )
        .await;
    let id_d = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "D".to_string(),
            "s3".to_string(),
            0,
            None,
        )
        .await;

    // 默认 workspace：A + C + D（B 不可见）
    let v = registry.execute_task_status("{}", "s1").await.unwrap();
    assert_eq!(v["scope"].as_str(), Some("workspace"));
    let ids: Vec<&str> = v["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(ids.contains(&id_a.as_str()), "本目录任务应可见: {v}");
    assert!(ids.contains(&id_c.as_str()), "同目录跨会话任务应可见: {v}");
    assert!(
        ids.contains(&id_d.as_str()),
        "归属未知（旧数据）应可见: {v}"
    );
    assert!(!ids.contains(&id_b.as_str()), "其他目录任务默认不可见: {v}");
    assert_eq!(v["total"].as_u64(), Some(3));

    // global：全部可见
    let v = registry
        .execute_task_status(r#"{"scope":"global"}"#, "s1")
        .await
        .unwrap();
    assert_eq!(v["scope"].as_str(), Some("global"));
    assert_eq!(v["total"].as_u64(), Some(4));
}

#[tokio::test]
async fn test_task_status_single_other_directory_denied_without_leak() {
    // U5 回归：单任务查询同样受视野约束——其他目录任务默认拒绝（附
    // scope="global" 指引；错误信息不泄漏任务实际所在目录）；global 可查。
    use crate::agent::background::TaskKind;

    let registry = ToolRegistry::new(default_strict_policy());
    let id = registry
        .background_tasks
        .register(
            TaskKind::Delegate,
            "B".to_string(),
            "s2".to_string(),
            0,
            Some("X:\\unrelated-other-dir".to_string()),
        )
        .await;

    let err = registry
        .execute_task_status(&format!(r#"{{"task_id":"{id}"}}"#), "s1")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("可见范围"), "应拒绝并说明可见范围: {msg}");
    assert!(msg.contains("global"), "应附 global 指引: {msg}");
    assert!(
        !msg.contains("unrelated-other-dir"),
        "错误信息不应泄漏其他目录: {msg}"
    );

    // global 显式查询可读
    let v = registry
        .execute_task_status(&format!(r#"{{"task_id":"{id}","scope":"global"}}"#), "s1")
        .await
        .unwrap();
    assert_eq!(v["id"].as_str(), Some(id.as_str()));
}

#[tokio::test]
async fn test_task_status_invalid_scope_rejected() {
    // U5 回归：scope 仅接受 workspace|global——其他值报错（防静默按默认
    // 处理导致的视野误解）。
    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_task_status(r#"{"scope":"local"}"#, "s1")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("scope"),
        "非法 scope 应报错: {err}"
    );
}

#[tokio::test]
async fn test_background_assigns_correct_parent_session() {
    // 回归保护：后台任务归属由调用链显式传递的 session_id 决定（非共享可变
    // 状态）——多会话并发 turn 下，各任务挂到正确的父会话。
    let mock = stream_mock(vec![stream_chunk_finish("x", "stop")]);
    let registry = delegate_registry(mock);

    // 不同会话各自发起后台委托（模拟并发 turn 的交错调用）
    let r1 = registry
        .execute_delegate_to_agent(r#"{"task":"bg-a"}"#, "session-a")
        .await
        .unwrap();
    let r2 = registry
        .execute_delegate_to_agent(r#"{"task":"bg-b"}"#, "session-b")
        .await
        .unwrap();

    // 等待两个后台任务完成（parent_session_id 在注册时已固化——无需等终态）
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

// ── 协议工具豁免（角色白名单 vs 循环协议） ──────────────────────────────

/// 构造 ToolCall（执行侧过滤测试用）。
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

/// 回归测试：**协议工具 `submit_result` 不受角色白名单限制**（执行侧恒允许）。
///
/// 历史 bug：请求侧恒注入 submit_result，执行侧按白名单过滤却没豁免 →
/// 子代理调用被拒（"角色 X 不允许使用工具 submit_result"），结果只能靠
/// 最后一条输出兜底。同时验证对照组：白名单外的**普通**工具仍被拒。
#[tokio::test]
async fn test_protocol_tool_exempt_from_role_whitelist() {
    let registry = delegate_registry(MockChatService::default());
    let task_id = register_delegate_task(&registry, "协议工具豁免").await;

    let protocol_call = tool_call("s1", PROTOCOL_TOOLS[0], r#"{"result":"done"}"#);
    let normal_call = tool_call("w1", "write_file", r#"{"path":"a.txt","content":"x"}"#);
    let whitelist = vec!["read_file".to_string()]; // 白名单不含 submit_result / write_file

    // 子代理 = 会话：task_id 即 session_id（stage_result 按此定位任务）
    let results = registry
        .execute_tool_calls_with_role(
            &[protocol_call, normal_call],
            &task_id,
            Some(&whitelist),
            Some("tester"),
        )
        .await;

    assert_eq!(results.len(), 2, "两个调用都应返回结果（含被拒的合成错误）");

    let protocol_outcome = results
        .iter()
        .find(|(id, _)| id == "s1")
        .map(|(_, o)| o)
        .expect("submit_result 应有结果");
    assert!(
        protocol_outcome.result.is_ok(),
        "协议工具必须被执行（豁免白名单），实际错误：{:?}",
        protocol_outcome
            .result
            .as_ref()
            .err()
            .map(|e| e.to_string())
    );

    let normal_outcome = results
        .iter()
        .find(|(id, _)| id == "w1")
        .map(|(_, o)| o)
        .expect("write_file 应有结果");
    let err = normal_outcome
        .result
        .as_ref()
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("不允许使用工具"),
        "白名单外的普通工具仍应被拒，实际：{err:?}"
    );
}

/// 判定单点：`is_protocol_tool` 只认协议工具。
#[test]
fn test_is_protocol_tool_membership() {
    assert!(is_protocol_tool(SUBMIT_RESULT_TOOL));
    assert!(
        is_protocol_tool("submit_result"),
        "常量与字面量必须一致（单点防漂移）"
    );
    for tool in [
        "read_file",
        "write_file",
        "execute_command",
        "delegate_to_agent",
    ] {
        assert!(!is_protocol_tool(tool), "{tool} 不应被视为协议工具");
    }
}
