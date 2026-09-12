//! glob / list_dir 工具执行器测试：安全策略、参数校验、注册与分发链路。
//!
//! 覆盖安全关键行为：白名单拒绝、缺失参数、目录不存在、注册可见性与
//! `execute_single` 分发路由。仅依赖 tempfile，不访问网络或真实 LLM。

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

fn glob_args(path: &Path, pattern: &str) -> String {
    json!({ "pattern": pattern, "path": path.to_string_lossy() }).to_string()
}

fn list_dir_args(path: &Path) -> String {
    json!({ "path": path.to_string_lossy() }).to_string()
}

// ── glob ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_execute_glob_success() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/c.rs"), "").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let result = registry
        .execute_glob(&glob_args(dir.path(), "**/*.rs"), "test-session")
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(2));
    assert_eq!(result["truncated"].as_bool(), Some(false));
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results
        .iter()
        .any(|v| v.as_str().unwrap().ends_with("a.rs")));
    assert!(results
        .iter()
        .any(|v| v.as_str().unwrap().ends_with("c.rs")));
}

#[tokio::test]
async fn test_execute_glob_missing_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let args = json!({ "path": dir.path().to_string_lossy() }).to_string();
    let result = registry.execute_glob(&args, "test-session").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_execute_glob_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();

    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()]));
    let result = registry
        .execute_glob(&glob_args(dir.path(), "**/*.rs"), "test-session")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

// ── list_dir ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_execute_list_dir_success() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let result = registry
        .execute_list_dir(&list_dir_args(dir.path()), "test-session")
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(2));
    assert_eq!(result["total"].as_u64(), Some(2));
    assert_eq!(result["truncated"].as_bool(), Some(false));
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(entries[0]["type"].as_str().unwrap(), "dir");
    assert_eq!(entries[0]["name"].as_str().unwrap(), "subdir/");
    assert_eq!(entries[1]["type"].as_str().unwrap(), "file");
    assert_eq!(entries[1]["name"].as_str().unwrap(), "a.txt");
}

#[tokio::test]
async fn test_execute_list_dir_missing_path() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_list_dir(r#"{}"#, "test-session").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_execute_list_dir_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();

    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()]));
    let result = registry
        .execute_list_dir(&list_dir_args(dir.path()), "test-session")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_execute_list_dir_nonexistent_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_list_dir(&list_dir_args(&missing), "test-session")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("执行失败"));
}

// ── 注册与分发 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_glob_and_list_dir_registered_definitions() {
    let registry = ToolRegistry::new(default_strict_policy());
    let defs = registry.definitions().await;
    assert!(defs.iter().any(|d| d.function.name == "glob"));
    assert!(defs.iter().any(|d| d.function.name == "list_dir"));
}

/// 通过 execute_single 分发链路执行 glob / list_dir（验证 dispatch 分支注册）。
#[tokio::test]
async fn test_glob_and_list_dir_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()]));
    let glob_call = crate::common::types::tool::ToolCall {
        id: "call_g".to_string(),
        call_type: crate::common::types::tool::ToolCallType::Function,
        function: crate::common::types::tool::FunctionCall {
            name: "glob".to_string(),
            arguments: glob_args(dir.path(), "**/*.rs"),
        },
    };
    let list_call = crate::common::types::tool::ToolCall {
        id: "call_l".to_string(),
        call_type: crate::common::types::tool::ToolCallType::Function,
        function: crate::common::types::tool::FunctionCall {
            name: "list_dir".to_string(),
            arguments: list_dir_args(dir.path()),
        },
    };
    let results = registry
        .execute_parallel(&[glob_call, list_call], "test-session", false)
        .await;
    assert_eq!(results.len(), 2);
    assert!(results[0].1.is_ok(), "glob 分发应成功：{:?}", results[0].1);
    assert_eq!(results[0].1.as_ref().unwrap()["count"].as_u64(), Some(1));
    assert!(
        results[1].1.is_ok(),
        "list_dir 分发应成功：{:?}",
        results[1].1
    );
    assert_eq!(results[1].1.as_ref().unwrap()["count"].as_u64(), Some(1));
}
