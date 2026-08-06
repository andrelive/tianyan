//! 代码类工具执行器测试：search_code / run_tests / verify_build。

use crate::config::SafetyMode;
use crate::executor::SecurityPolicy;

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

// ── search_code ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_code_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_search_code(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
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
