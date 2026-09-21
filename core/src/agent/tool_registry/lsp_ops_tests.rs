//! execute_lsp 工具测试：参数校验错误路径 + 工具注册 + schema 形态。
//!
//! 成功路径需要真实语言服务器，由 client 层内存假服务器测试覆盖
//! （此处仅验证校验与降级行为）。

use crate::agent::tool_params::LspParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::executor::SecurityPolicy;
use crate::lsp::diagnostics::LspOperation;

fn registry() -> ToolRegistry {
    ToolRegistry::new(SecurityPolicy::default())
}

#[tokio::test]
async fn test_lsp_missing_file_path() {
    // file_path 现为必填 String（旧形态是 Option + 运行期校验）
    let err = registry()
        .execute_lsp(r#"{"operation":"hover","line":0,"character":0}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_unknown_operation_rejected_at_parse() {
    // 判别力：旧形态靠运行期字符串白名单兜底；现由类型化枚举在**解析期**拒绝，
    // 消息列出合法取值（serde 枚举错误）
    let err = registry()
        .execute_lsp(r#"{"operation":"frobnicate","file_path":"C:\\x\\main.rs"}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
    assert!(err.to_string().contains("goToDefinition"), "{}", err);
}

#[tokio::test]
async fn test_lsp_position_operation_requires_line_and_character() {
    // 判别力：旧形态对缺 line/character 静默按 0:0 查询（"漏参"伪装成"空结果"）
    for args in [
        r#"{"operation":"hover","file_path":"C:\\x\\main.rs"}"#,
        r#"{"operation":"goToDefinition","file_path":"C:\\x\\main.rs","line":3}"#,
        r#"{"operation":"findReferences","file_path":"C:\\x\\main.rs","character":5}"#,
        r#"{"operation":"goToImplementation","file_path":"C:\\x\\main.rs"}"#,
    ] {
        let err = registry().execute_lsp(args).await.unwrap_err();
        assert!(err.to_string().contains("参数无效"), "{args} → {err}");
        assert!(
            err.to_string().contains("需要 line 与 character"),
            "{args} → {err}"
        );
    }
}

#[tokio::test]
async fn test_lsp_workspace_symbol_requires_query() {
    let err = registry()
        .execute_lsp(r#"{"operation":"workspaceSymbol","file_path":"C:\\x\\main.rs"}"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
    assert!(err.to_string().contains("query"), "{}", err);
}

#[tokio::test]
async fn test_lsp_malformed_json() {
    let err = registry().execute_lsp("not json").await.unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{}", err);
}

#[tokio::test]
async fn test_lsp_tool_registered_and_schema_constrains_operation() {
    let defs = registry().definitions().await;
    let lsp = defs
        .iter()
        .find(|d| d.function.name == "lsp")
        .expect("lsp 工具已注册");
    assert!(lsp.function.description.contains("goToDefinition"));
    // operation 的类型化枚举必须进 schema——若退回 String，schema 里不会有这些取值
    let schema = lsp.function.parameters.to_string();
    for op in LspOperation::ALL {
        assert!(
            schema.contains(&format!("\"{}\"", op.as_str())),
            "{} 未约束进 schema：{schema}",
            op.as_str()
        );
    }
}

#[test]
fn test_lsp_operation_wire_names_match_serde() {
    // 单一事实源：`as_str()`（错误消息与工具结果用）与 serde wire 名一致
    for op in LspOperation::ALL {
        let value = serde_json::to_value(op).expect("枚举可序列化");
        assert_eq!(value.as_str(), Some(op.as_str()), "{op:?}");
    }
    assert_eq!(LspOperation::ALL.len(), 6);
}

#[test]
fn test_lsp_params_accepts_only_named_operations() {
    for op in LspOperation::ALL {
        let json = format!(
            r#"{{"operation":"{}","file_path":"C:\\x\\main.rs"}}"#,
            op.as_str()
        );
        let params: LspParams = serde_json::from_str(&json).expect("合法操作可解析");
        assert_eq!(params.operation, op);
    }
    assert!(
        serde_json::from_str::<LspParams>(r#"{"operation":"frobnicate","file_path":"x"}"#).is_err()
    );
}
