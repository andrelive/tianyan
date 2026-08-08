//! symbol_outline 工具执行器测试：安全策略、参数校验、注册与分发链路。

use std::path::{Path, PathBuf};

use serde_json::json;

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

/// 带目录白名单的严格策略（目录规范化与 `check_path_rules` 保持一致）。
fn file_policy(allowed: Vec<PathBuf>) -> SecurityPolicy {
    let mut policy = default_strict_policy();
    policy.allowed_directories = allowed
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    policy
}

fn symbol_args(path: &Path) -> String {
    json!({ "path": path.to_string_lossy() }).to_string()
}

// ── 执行 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_execute_symbol_outline_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.rs");
    std::fs::write(&path, "fn main() {}\nstruct Foo {}\n").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let result = registry
        .execute_symbol_outline(&symbol_args(&path))
        .await
        .unwrap();
    assert_eq!(result["path"], path.to_string_lossy().to_string());
    assert_eq!(result["language"], "rust");
    assert_eq!(result["count"].as_u64(), Some(2));
    assert_eq!(result["truncated"].as_bool(), Some(false));
    assert_eq!(result["errors"].as_bool(), Some(false));

    let symbols = result["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0]["name"], "main");
    assert_eq!(symbols[0]["kind"], "Function");
    assert_eq!(symbols[0]["line"].as_u64(), Some(1));
    assert_eq!(symbols[1]["name"], "Foo");
    assert_eq!(symbols[1]["kind"], "Struct");
    assert_eq!(symbols[1]["line"].as_u64(), Some(2));
}

#[tokio::test]
async fn test_execute_symbol_outline_missing_path() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_symbol_outline(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_execute_symbol_outline_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let outside = dir.path().join("main.rs");
    std::fs::write(&outside, "fn main() {}\n").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()]));
    let result = registry
        .execute_symbol_outline(&symbol_args(&outside))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_symbol_outline_nonexistent_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.rs");
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_symbol_outline(&symbol_args(&missing))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("执行失败"));
}

#[tokio::test]
async fn test_execute_symbol_outline_unsupported_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.txt");
    std::fs::write(&path, "hello").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let result = registry.execute_symbol_outline(&symbol_args(&path)).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("不支持的语言"));
}

// ── 注册与分发 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_symbol_outline_registered_definition() {
    let registry = ToolRegistry::new(default_strict_policy());
    let defs = registry.definitions().await;
    let def = defs
        .iter()
        .find(|d| d.function.name == "symbol_outline")
        .expect("symbol_outline 应已注册");
    assert!(def.function.description.contains("tree-sitter"));
    assert!(def.function.description.contains("Rust"));
}

/// 通过 execute_single 分发链路执行 symbol_outline（验证 dispatch 分支注册）。
#[tokio::test]
async fn test_symbol_outline_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "fn f() {}\n").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let call = crate::common::types::tool::ToolCall {
        id: "call_s".to_string(),
        call_type: crate::common::types::tool::ToolCallType::Function,
        function: crate::common::types::tool::FunctionCall {
            name: "symbol_outline".to_string(),
            arguments: symbol_args(&path),
        },
    };
    let results = registry.execute_parallel(&[call], "test-session").await;
    assert_eq!(results.len(), 1);
    assert!(
        results[0].1.is_ok(),
        "symbol_outline 分发应成功：{:?}",
        results[0].1
    );
    assert_eq!(results[0].1.as_ref().unwrap()["count"].as_u64(), Some(1));
    assert_eq!(results[0].1.as_ref().unwrap()["symbols"][0]["name"], "f");
}
