//! 知识库类工具执行器测试：search_vfs / knowledge_ingest。

use std::sync::Arc;

use crate::common::types::{ContextNamespace, TianyanUri};
use crate::config::SafetyMode;
use crate::executor::SecurityPolicy;
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

// ── search_vfs ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_vfs_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_search_vfs(r#"{"query":"hello"}"#).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_search_vfs_success_surfaces_vfs_results() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::builder()
        .with_content(&uri, ContentLevel::Abstract, "abstract text")
        .with_content(&uri, ContentLevel::Overview, "overview text")
        .with_search_results(vec![SearchResult {
            uri: uri.clone(),
            score: 0.9,
        }])
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_search_vfs(r#"{"query":"hello","top_k":5}"#)
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
async fn test_search_vfs_surfaces_search_error() {
    let vfs = MockVfs::builder()
        .with_search_error("simulated failure")
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));
    let result = registry.execute_search_vfs(r#"{"query":"hello"}"#).await;
    assert!(result.is_err());
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
