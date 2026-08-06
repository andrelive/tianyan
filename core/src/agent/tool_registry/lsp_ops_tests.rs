//! execute_lsp 工具测试：参数校验错误路径 + 工具注册。
//!
//! 成功路径需要真实语言服务器，由 client 层内存假服务器测试覆盖
//! （此处仅验证校验与降级行为）。

use crate::agent::tool_registry::ToolRegistry;
use crate::executor::SecurityPolicy;

fn registry() -> ToolRegistry {
    ToolRegistry::new(SecurityPolicy::default())
}

#[tokio::test]
async fn test_lsp_missing_file_path() {
    let err = registry()
        .execute_lsp(r#"{"operation":"hover"}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_unknown_operation() {
    let err = registry()
        .execute_lsp(r#"{"operation":"frobnicate","file_path":"C:\\x\\main.rs"}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_workspace_symbol_requires_query() {
    let err = registry()
        .execute_lsp(r#"{"operation":"workspaceSymbol","file_path":"C:\\x\\main.rs"}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_malformed_json() {
    let err = registry().execute_lsp("not json").await.unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_tool_registered() {
    let defs = registry().definitions().await;
    let lsp = defs
        .iter()
        .find(|d| d.function.name == "lsp")
        .expect("lsp 工具已注册");
    assert!(lsp.function.description.contains("goToDefinition"));
}
