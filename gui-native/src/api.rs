//! 对内嵌 server 的进程级管道：tokio runtime 单例 + HTTP 小工具 + 常驻事件流订阅。
//!
//! 与 `ClientApp` 无耦合——只负责「把 server 的事件读进 channel」，
//! 事件怎么解释是 `ClientApp::handle_event` 的事。

use crate::*;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::runtime::Runtime;

pub(crate) fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("创建 tokio runtime 失败"))
}

/// 在 tokio runtime 上取一段 JSON（GPUI 的 executor 是 smol 系，跨 runtime await
/// `JoinHandle` 是安全的）。
pub(crate) async fn fetch_json(url: String) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
            resp.json::<serde_json::Value>()
                .await
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// POST 一段 JSON 并取回 JSON（同样跑在 tokio runtime 上，跨 runtime await 安全）。
pub(crate) async fn post_json(url: String, body: serde_json::Value) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// POST 一份 multipart（知识导入）并取回 JSON。
///
/// **必须**跑在 `runtime()` 上，不能用裸 `reqwest::Client::send().await`：
/// GPUI 的 executor 是 smol 系、**没有 tokio reactor**，裸 reqwest 在 `cx.spawn`
/// 里 poll 到 TCP 建连时会 panic（实测：知识导入一点，整个进程 abort ——
/// `there is no reactor running, must be called from the context of a Tokio 1.x runtime`）。
pub(crate) async fn post_multipart(
    url: String,
    form: reqwest::multipart::Form,
) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .post(&url)
                .multipart(form)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// DELETE 一个资源（只看状态码，不解析响应体）：删会话用。
pub(crate) async fn delete_json(url: String) -> Result<(), String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .delete(&url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP {status}: {text}"));
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// 在 GPUI executor 上等一段时间：借 tokio runtime 睡（与 `fetch_json` 同一手法，
/// 避免引入 GPUI 自己的 timer 类型）。
pub(crate) async fn sleep_secs(secs: u64) {
    let _ = runtime()
        .spawn(async move { tokio::time::sleep(Duration::from_secs(secs)).await })
        .await;
}

/// PUT 一段 JSON（会话级模型选择等）：与 [`post_json`] 同一手法。
pub(crate) async fn put_json(url: String, body: serde_json::Value) -> Result<serde_json::Value, String> {
    runtime()
        .spawn(async move {
            let resp = reqwest::Client::new()
                .put(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("join: {e}")))
}

/// 在字节缓冲里找子串（SSE 分帧用；`str::find` 在增量缓冲上有 char 边界风险，
/// 且 UTF-8 多字节字符可能被 chunk 切断——必须**按字节**分帧后再解码）
pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 后台读统一事件流（`GET /api/v1/events`，ADR-028 常驻水管）。
///
/// `POST /api/v1/chat/stream` 只是开关，流式输出全部经这条常驻 SSE 下发
/// （`server/src/api/chat/handlers.rs`），所以订阅必须**独立于发送**常驻。
/// 断线（server 未起 / 重启）按 1s 间隔重连；`tx` 关闭即退出（窗口销毁）。
pub(crate) fn spawn_event_reader(port: u16, tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>) {
    // `spawn` 返回的是 `JoinHandle`（任务**已经**开始调度，不是惰性 Future）；
    // 我们不需要 join，也不关心它何时结束——退出条件是 `tx.is_closed()`（窗口销毁）。
    // 这里显式 `drop` 掉句柄（= detach），顺带说明为什么不是 `let _ =`：
    // clippy 的 `let_underscore_future` 会把 `JoinHandle` 误判成「没 await 的 Future」。
    drop(runtime().spawn(async move {
        let client = reqwest::Client::new();
        let url = format!("http://{SERVER_HOST}:{port}/api/v1/events");
        loop {
            if tx.is_closed() {
                return;
            }
            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    println!("[事件] 订阅失败（1s 后重试）：{e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            println!("[事件] 已订阅 {}（HTTP {}）", url, resp.status());
            let mut stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        println!("[事件] 读取中断：{e}");
                        break;
                    }
                };
                buf.extend_from_slice(&bytes);
                while let Some(pos) = find_subslice(&buf, b"\n\n") {
                    let frame: Vec<u8> = buf.drain(..pos + 2).collect();
                    let frame = String::from_utf8_lossy(&frame);
                    // SSE 多行 data 按规范用 \n 拼接（axum 的 Event::data 目前是单行）
                    let mut data = String::new();
                    for line in frame.lines() {
                        if let Some(rest) = line.strip_prefix("data:") {
                            if !data.is_empty() {
                                data.push('\n');
                            }
                            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(v) => {
                            if tx.send(v).is_err() {
                                return;
                            }
                        }
                        Err(e) => println!("[事件] 解析失败：{e}"),
                    }
                }
            }
            println!("[事件] 连接结束，1s 后重连");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }));
}
