//! LSP 客户端测试：内存 duplex 假服务器（无需真实语言服务器）。
//!
//! 覆盖：生命周期握手、Content-Length 分帧（含多字节 UTF-8 跨分片写入）、
//! 主动推送诊断回调、JSON-RPC 错误响应、服务器断连优雅降级。

use std::str::FromStr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{
    duplex, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

use crate::lsp::client::{LspClient, PublishDiagnosticsCallback};

/// 读取一个 Content-Length 分帧消息（与客户端同款分帧协议的独立实现）。
async fn read_framed<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Value {
    let mut content_length = None;
    loop {
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await.expect("读头部");
        assert!(n > 0, "假服务器读侧提前 EOF");
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            break;
        }
        if let Some(rest) = text.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let mut payload = vec![0u8; content_length.expect("有 Content-Length")];
    reader.read_exact(&mut payload).await.expect("读消息体");
    serde_json::from_slice(&payload).expect("JSON 解析")
}

/// 写出一个 Content-Length 分帧消息。
async fn write_framed<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) {
    let payload = serde_json::to_vec(message).expect("JSON 序列化");
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    writer.write_all(header.as_bytes()).await.expect("写头部");
    writer.write_all(&payload).await.expect("写消息体");
    writer.flush().await.expect("flush");
}

/// 假 LSP 服务器：完整生命周期握手 + 主动推送诊断 + hover 响应。
///
/// 诊断推送与 hover 响应体均跨两个分片写入，验证客户端分帧鲁棒性
/// （多字节 UTF-8 文本不被截断）。
async fn fake_server(server_read: impl AsyncRead + Unpin, server_write: impl AsyncWrite + Unpin) {
    let mut reader = BufReader::new(server_read);
    let mut writer = server_write;

    // initialize 请求 → 能力响应
    let init = read_framed(&mut reader).await;
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["jsonrpc"], "2.0");
    write_framed(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": init["id"],
            "result": { "capabilities": {} }
        }),
    )
    .await;

    // initialized 通知（无需响应）
    let notif = read_framed(&mut reader).await;
    assert_eq!(notif["method"], "initialized");
    assert!(notif.get("id").is_none(), "通知不应带 id");

    // 主动推送诊断（UTF-8 中文，跨两个分片写入）
    let diagnostic = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": "file:///fake/main.rs",
            "diagnostics": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                "severity": 1,
                "message": "类型错误：悬停内容测试"
            }]
        }
    });
    let payload = serde_json::to_vec(&diagnostic).expect("JSON 序列化");
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let split = payload.len() / 2;
    writer.write_all(header.as_bytes()).await.expect("写头部");
    writer.write_all(&payload[..split]).await.expect("写前半");
    writer.write_all(&payload[split..]).await.expect("写后半");
    writer.flush().await.expect("flush");

    // hover 请求 → 响应（同样跨分片 + 中文）
    let hover = read_framed(&mut reader).await;
    assert_eq!(hover["method"], "textDocument/hover");
    let response = json!({
        "jsonrpc": "2.0",
        "id": hover["id"],
        "result": { "contents": "悬停结果：fn main" }
    });
    let payload = serde_json::to_vec(&response).expect("JSON 序列化");
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let split = payload.len() / 2;
    writer.write_all(header.as_bytes()).await.expect("写头部");
    writer.write_all(&payload[..split]).await.expect("写前半");
    writer.write_all(&payload[split..]).await.expect("写后半");
    writer.flush().await.expect("flush");
}

#[tokio::test]
async fn test_lifecycle_handshake_and_unsolicited_diagnostics() {
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    let server_task = tokio::spawn(async move { fake_server(server_read, server_write).await });

    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let callback_received = received.clone();
    let on_publish: Option<PublishDiagnosticsCallback> =
        Some(Arc::new(move |p: lsp_types::PublishDiagnosticsParams| {
            callback_received.lock().unwrap().push(p.diagnostics.len());
        }));

    let client = LspClient::from_streams(
        Box::new(client_read),
        Box::new(client_write),
        "fake-server",
        "安装假服务器",
        on_publish,
    );

    // 生命周期：initialize → initialized → hover
    let uri = lsp_types::Uri::from_str("file:///fake/main.rs").expect("URI");
    let init_result = client
        .initialize(uri.clone())
        .await
        .expect("initialize 成功");
    assert_eq!(init_result["capabilities"], json!({}));
    client.initialized().await.expect("initialized 通知成功");

    // 读循环按序处理：诊断推送先于 hover 响应到达，
    // hover 返回时回调必然已触发（同一读循环 happen-before）。
    let hover = client.hover(&uri, 0, 0).await.expect("hover 成功");
    assert_eq!(hover["contents"], "悬停结果：fn main");

    assert_eq!(
        *received.lock().unwrap(),
        vec![1usize],
        "应收到 1 条推送诊断"
    );
    server_task.await.expect("假服务器正常退出");
}

/// 假服务器：收到 initialize 后直接断开（不响应）→ 客户端应优雅降级。
async fn fake_server_drops_after_initialize(
    server_read: impl AsyncRead + Unpin,
    server_write: impl AsyncWrite + Unpin,
) {
    let mut reader = BufReader::new(server_read);
    let _init = read_framed(&mut reader).await;
    drop(server_write); // 关闭连接：客户端读侧 EOF
}

#[tokio::test]
async fn test_broken_server_returns_unavailable() {
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    let server_task =
        tokio::spawn(
            async move { fake_server_drops_after_initialize(server_read, server_write).await },
        );

    let client = LspClient::from_streams(
        Box::new(client_read),
        Box::new(client_write),
        "fake-broken",
        "安装提示：假装我",
        None,
    );
    let err = client
        .request("initialize", json!({}))
        .await
        .expect_err("服务器断连应报错");
    let msg = err.to_string();
    assert!(msg.contains("不可用"), "应包含 不可用: {msg}");
    assert!(msg.contains("fake-broken"), "应包含服务器名: {msg}");
    assert!(msg.contains("安装提示：假装我"), "应包含安装提示: {msg}");
    server_task.await.expect("假服务器正常退出");
}

/// 假服务器：对 references 请求返回 JSON-RPC 错误对象。
async fn fake_server_jsonrpc_error(
    server_read: impl AsyncRead + Unpin,
    server_write: impl AsyncWrite + Unpin,
) {
    let mut reader = BufReader::new(server_read);
    let mut writer = server_write;
    let init = read_framed(&mut reader).await;
    write_framed(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":init["id"],"result":{"capabilities":{}}}),
    )
    .await;
    let _notif = read_framed(&mut reader).await;
    let refs = read_framed(&mut reader).await;
    assert_eq!(refs["method"], "textDocument/references");
    write_framed(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":refs["id"],"error":{"code":-32601,"message":"method not found"}}),
    )
    .await;
}

#[tokio::test]
async fn test_jsonrpc_error_response_is_reported() {
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    let server_task =
        tokio::spawn(async move { fake_server_jsonrpc_error(server_read, server_write).await });
    let client = LspClient::from_streams(
        Box::new(client_read),
        Box::new(client_write),
        "fake",
        "hint",
        None,
    );
    let uri = lsp_types::Uri::from_str("file:///fake/main.rs").expect("URI");
    client.initialize(uri.clone()).await.expect("initialize");
    client.initialized().await.expect("initialized");
    let err = client
        .references(&uri, 1, 1, true)
        .await
        .expect_err("JSON-RPC 错误应报错");
    assert!(err.to_string().contains("JSON-RPC 错误"), "{}", err);
    server_task.await.expect("假服务器正常退出");
}

/// 假服务器：连续推送两条诊断，随后响应 hover 请求。
async fn fake_server_two_diagnostics(
    server_read: impl AsyncRead + Unpin,
    server_write: impl AsyncWrite + Unpin,
) {
    let mut reader = BufReader::new(server_read);
    let mut writer = server_write;
    let init = read_framed(&mut reader).await;
    write_framed(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":init["id"],"result":{"capabilities":{}}}),
    )
    .await;
    let _notif = read_framed(&mut reader).await;
    for i in 0..2 {
        let diagnostic = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///fake/main.rs",
                "diagnostics": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                    "severity": 1,
                    "message": format!("诊断 {i}")
                }]
            }
        });
        write_framed(&mut writer, &diagnostic).await;
    }
    // 保持连接：等待 hover 请求并响应（读循环存活才会到达这里）。
    let hover = read_framed(&mut reader).await;
    write_framed(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":hover["id"],"result":{"contents":"ok"}}),
    )
    .await;
}
#[tokio::test]
async fn test_callback_panic_does_not_kill_read_loop() {
    // 回调 panic 不得杀死读循环（读循环是唯一响应分发者）：catch_unwind
    // 兜底——panic 回调被隔离并记 error，后续消息照常分发。
    // 判别力：若移除 catch_unwind，回调 panic 杀死读循环 → 随后的 hover
    // 请求得不到响应 → 本测试超时红。
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    let server_task =
        tokio::spawn(async move { fake_server_two_diagnostics(server_read, server_write).await });
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let on_publish: Option<PublishDiagnosticsCallback> =
        Some(Arc::new(move |_p: lsp_types::PublishDiagnosticsParams| {
            let n = callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                // 首次回调 panic——读循环必须隔离它并继续分发后续消息。
                panic!("publishDiagnostics 回调测试性 panic");
            }
        }));
    let client = LspClient::from_streams(
        Box::new(client_read),
        Box::new(client_write),
        "fake-panic-server",
        "安装提示",
        on_publish,
    );
    let uri = lsp_types::Uri::from_str("file:///fake/main.rs").expect("URI");
    client
        .initialize(uri.clone())
        .await
        .expect("initialize 成功");
    client.initialized().await.expect("initialized 通知成功");
    // hover 响应在两条诊断之后——能收到即证明读循环穿越回调 panic 存活。
    let hover = tokio::time::timeout(std::time::Duration::from_secs(10), client.hover(&uri, 0, 0))
        .await
        .expect("读循环应在回调 panic 后继续分发（未超时）")
        .expect("hover 成功");
    assert_eq!(hover["contents"], "ok");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "两条诊断回调都应被调用（第一条 panic 被隔离、第二条正常执行）"
    );
    server_task.await.expect("假服务器正常退出");
}
