//! LSP 诊断存储测试：诊断读取排序 + 推送转换。

use std::str::FromStr;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

use super::*;

#[tokio::test]
async fn test_diagnostics_for_empty_store() {
    let manager = LspManager::new();
    assert!(manager.diagnostics_for("C:\\fake\\main.rs").is_empty());
}

#[tokio::test]
async fn test_diagnostics_for_sorted_by_line_then_column() {
    let manager = LspManager::new();
    let path = "C:\\fake\\main.rs".to_string();
    let diag = |line: usize, column: usize, message: &str| StructuredDiagnostic {
        file: path.clone(),
        line,
        column,
        level: "error".to_string(),
        code: None,
        message: message.to_string(),
        suggestion: None,
    };
    manager.store.insert(
        path.clone(),
        vec![
            diag(5, 1, "later"),
            diag(1, 9, "earlier-col"),
            diag(1, 2, "earliest"),
        ],
    );
    let diagnostics = manager.diagnostics_for(&path);
    let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages, vec!["earliest", "earlier-col", "later"]);
}

#[tokio::test]
async fn test_handle_publish_stores_converted_diagnostics() {
    let manager = LspManager::new();
    let uri = Uri::from_str("file:///C:/fake/main.rs").expect("URI");
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: vec![Diagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 2,
                },
                end: Position {
                    line: 0,
                    character: 5,
                },
            },
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::Number(1)),
            message: "类型错误".to_string(),
            ..Default::default()
        }],
        version: Some(1),
    };
    manager.handle_publish(params);

    // 与客户端同款 URI → 路径转换，保证平台一致
    let path = url::Url::parse("file:///C:/fake/main.rs")
        .expect("URL")
        .to_file_path()
        .expect("file path")
        .to_string_lossy()
        .to_string();
    let diagnostics = manager.diagnostics_for(&path);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "类型错误");
    assert_eq!(diagnostics[0].line, 1, "LSP 0 起始 → 1 起始");
    assert_eq!(diagnostics[0].column, 3, "LSP 0 起始 → 1 起始");
    assert_eq!(diagnostics[0].level, "error");
    assert_eq!(diagnostics[0].code.as_deref(), Some("1"));
}
