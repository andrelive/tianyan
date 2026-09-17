//! LSP 诊断存储测试：诊断读取排序 + 推送转换。

use std::str::FromStr;
use std::time::Duration;

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

/// 等读循环置位 dead（有限轮询，避免测试卡死）。
async fn wait_dead(client: &LspClient) {
    for _ in 0..100 {
        if client.is_dead() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// T1-7：池中的「死客户端」（服务器崩溃/退出）必须被逐出，不得复用。
///
/// 旧行为：`ensure_server` 命中池即返回，死客户端的 `Arc` 永远留在池里，
/// 之后该项目根的所有 LSP 查询都快速失败（“连接已关闭”）——永不自愈。
#[tokio::test]
async fn test_take_live_server_evicts_dead_client() {
    let manager = LspManager::new();
    // 读流立即 EOF → 读循环置 dead（等价于服务器崩溃关闭 stdout）
    let dead_client = Arc::new(LspClient::from_streams(
        Box::new(tokio::io::empty()),
        Box::new(tokio::io::sink()),
        "fake-lsp",
        "安装提示",
        None,
    ));
    manager
        .servers
        .insert("C:\\proj".to_string(), dead_client.clone());
    wait_dead(&dead_client).await;
    assert!(dead_client.is_dead(), "读流 EOF 后客户端应标记为已死");

    assert!(
        manager.take_live_server("C:\\proj").is_none(),
        "死客户端不得被复用（返回 None 触发重建）"
    );
    assert!(
        !manager.servers.contains_key("C:\\proj"),
        "死客户端应被逐出池"
    );
}

/// T1-7 反向：存活客户端正常复用（逐出逻辑不得误伤）。
#[tokio::test]
async fn test_take_live_server_reuses_alive_client() {
    let manager = LspManager::new();
    let (read, _write) = tokio::io::duplex(64); // 写端保持打开 → 读侧不 EOF
    let client = Arc::new(LspClient::from_streams(
        Box::new(read),
        Box::new(tokio::io::sink()),
        "fake-lsp",
        "安装提示",
        None,
    ));
    manager
        .servers
        .insert("C:\\proj".to_string(), client.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!client.is_dead());

    let got = manager
        .take_live_server("C:\\proj")
        .expect("存活客户端应被复用");
    assert!(Arc::ptr_eq(&got, &client), "复用应返回同一客户端实例");
    assert!(
        manager.servers.contains_key("C:\\proj"),
        "存活客户端不应被逐出"
    );
}
