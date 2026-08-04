//! 工具执行器测试。
//!
//! 覆盖 14 个 `execute_*` 方法的安全关键行为：安全策略拒绝、VFS 检索、
//! 未配置依赖的错误路径、成功路径。所有测试仅依赖 tempfile 与共享 Mock，
//! 不访问网络、真实 LLM 或真实 LanceDB。

use std::path::PathBuf;
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
use crate::test_utils::MockVfs;

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

/// 带目录白/黑名单的严格策略。
///
/// 目录先规范化（与 `SecurityPolicy::check_path` 对目标路径的规范化保持一致，
/// 避免 Windows 上 Temp 目录 junction 导致的 `starts_with` 前缀失配）。
fn file_policy(allowed: Vec<PathBuf>, blocked: Vec<PathBuf>) -> SecurityPolicy {
    let mut policy = default_strict_policy();
    policy.allowed_directories = allowed
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    policy.blocked_directories = blocked
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    policy
}

// ── read_file ────────────────────────────────────────────────────────────

/// 将文件路径安全地编码进 JSON 参数（Windows 路径含反斜杠，必须转义）。
fn read_file_args(path: &std::path::Path) -> String {
    json!({ "path": path.to_string_lossy() }).to_string()
}

/// 将路径与内容编码进 write_file 的 JSON 参数。
fn write_file_args(path: &std::path::Path, content: &str) -> String {
    json!({ "path": path.to_string_lossy(), "content": content }).to_string()
}

#[tokio::test]
async fn test_read_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    std::fs::write(&path, "hello content").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()], vec![]));
    let result = registry
        .execute_read_file(&read_file_args(&path))
        .await
        .unwrap();
    assert_eq!(result.as_str().unwrap(), "hello content");
}

#[tokio::test]
async fn test_read_file_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_read_file(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_read_file_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(dir.path().join("secret.txt"), "secret").unwrap();

    // `../` 逃逸路径：虽然 secret.txt 真实存在，但其规范化路径
    // 位于 allowed 目录之外，必须被安全策略拒绝。
    let escape = allowed.join("..").join("secret.txt");
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry.execute_read_file(&read_file_args(&escape)).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_read_file_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry.execute_read_file(&read_file_args(&secret)).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_read_file_missing_file_returns_error() {
    // 无目录白名单限制：安全检查放行后，读取不存在文件应报执行错误。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let missing = dir.path().join("does_not_exist.txt");
    let result = registry.execute_read_file(&read_file_args(&missing)).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("执行失败"));
}

// ── write_file ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_write_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let path = dir.path().join("out.txt");
    let result = registry
        .execute_write_file(&write_file_args(&path, "written content"))
        .await;
    assert!(result.is_ok());
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "written content");
}

#[tokio::test]
async fn test_write_file_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_write_file(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_write_file_rejects_blocked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    // 预创建目标文件，使规范化路径能命中黑名单目录（与生产 check_path 行为一致）。
    let path = blocked.join("x.txt");
    std::fs::write(&path, "existing").unwrap();
    let registry = ToolRegistry::new(file_policy(vec![], vec![blocked.to_path_buf()]));
    let result = registry
        .execute_write_file(&write_file_args(&path, "x"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_write_file_rejects_when_file_write_disabled() {
    let mut policy = default_strict_policy();
    policy.allow_file_write = false;
    let registry = ToolRegistry::new(policy);
    let result = registry
        .execute_write_file(r#"{"path":"any.txt","content":"x"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
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

// ── search_code ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_code_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_search_code(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

// ── search_knowledge ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_knowledge_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_search_knowledge(r#"{"query":"hello"}"#)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_search_knowledge_success_surfaces_vfs_results() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::builder()
        .with_content(&uri, ContentLevel::Abstract, "abstract text")
        .with_content(&uri, ContentLevel::Overview, "overview text")
        .with_search_results(vec![SearchResult {
            uri: uri.clone(),
            score: 0.9,
            matched_level: ContentLevel::Abstract,
            content: Some("abstract text".to_string()),
        }])
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_search_knowledge(r#"{"query":"hello","top_k":5}"#)
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(1));
    assert_eq!(
        result["results"][0]["uri"].as_str().unwrap(),
        "tianyan://knowledge/doc1"
    );
    assert!((result["results"][0]["score"].as_f64().unwrap() - 0.9).abs() < 0.001);
    assert_eq!(
        result["results"][0]["abstract"].as_str().unwrap(),
        "abstract text"
    );
    assert_eq!(
        result["results"][0]["overview"].as_str().unwrap(),
        "overview text"
    );
}

#[tokio::test]
async fn test_search_knowledge_surfaces_search_error() {
    let vfs = MockVfs::builder()
        .with_search_error("simulated failure")
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));
    let result = registry
        .execute_search_knowledge(r#"{"query":"hello"}"#)
        .await;
    assert!(result.is_err());
}

// ── vfs_read ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vfs_read_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1"}"#)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_vfs_read_success_with_content() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::builder()
        .with_content(&uri, ContentLevel::Abstract, "the abstract")
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1"}"#)
        .await
        .unwrap();
    assert_eq!(result["abstract"].as_str().unwrap(), "the abstract");
    // 未写入的层级返回空字符串（not_found 正常状态），而非错误。
    assert_eq!(result["overview"].as_str().unwrap(), "");
    assert_eq!(result["detail"].as_str().unwrap(), "");
}

#[tokio::test]
async fn test_vfs_read_rejects_invalid_uri() {
    let vfs = MockVfs::new();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));
    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://invalid_namespace/doc"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("无效 URI"));
}

// ── vfs_list ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vfs_list_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_vfs_list(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_vfs_list_success() {
    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let vfs = MockVfs::new();
    vfs.add_entry(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]),
    );
    vfs.add_directory(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["subdir".to_string()]),
    );
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_list(r#"{"uri":"tianyan://knowledge"}"#)
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(2));
    assert_eq!(result["entries"][0]["is_directory"].as_bool(), Some(false));
    assert_eq!(result["entries"][1]["is_directory"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_vfs_list_defaults_to_knowledge_root() {
    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let vfs = MockVfs::new();
    vfs.add_entry(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]),
    );
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry.execute_vfs_list(r#"{}"#).await.unwrap();
    assert_eq!(result["uri"].as_str().unwrap(), "tianyan://knowledge");
    assert_eq!(result["count"].as_u64(), Some(1));
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

// ── run_tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_run_tests_success() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_run_tests(r#"{"command":"echo hello"}"#)
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_run_tests_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_run_tests(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

// ── verify_build ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_verify_build_success_fallback() {
    // 未配置 verification_gate 时回退到退出码校验路径。
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_verify_build(r#"{"command":"echo hello"}"#)
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_verify_build_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_verify_build(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
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

// ── knowledge_ingest ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_knowledge_ingest_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_knowledge_ingest(r#"{"path":"some/file.txt"}"#)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("KnowledgeIngestor not configured"));
}

#[tokio::test]
async fn test_knowledge_ingest_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_knowledge_ingest(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
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
