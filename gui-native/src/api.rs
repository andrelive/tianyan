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
pub(crate) async fn post_json(
    url: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
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
pub(crate) async fn put_json(
    url: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
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

/// 事件流给 UI 侧的消息：**数据帧 + 连接状态**。
///
/// 为什么要带连接状态：断线后必须让 UI 做两件事 ——
/// ① 把已打开会话的订阅**重放**一遍（服务端订阅态随连接丢失，对齐 vite
///    `onopen` 里那段 `for (const sid of subscribedSessions) doSubscribe(sid)`）；
/// ② 把「已断开」显示出来，否则用户只看到界面停在某处不动，无从判断是卡了还是断了。
pub(crate) enum EventStreamMsg {
    /// 一帧事件（`chat_stream` / `snapshot` / `task_status` / `command_output`）
    Event(serde_json::Value),
    /// 已（重新）连上 —— UI 应重放所有已打开会话的订阅
    Connected,
    /// 连接断开，进入重连退避
    Disconnected,
}

/// 重连退避上限。
///
/// 服务端重启/未起时不要每秒猛敲（旧实现固定 1s，既没退避也没有任何状态上抛）。
const RECONNECT_MAX_SECS: u64 = 30;

/// 心跳超时：服务端每 15s 发一次 `keep-alive`（`SSE_KEEPALIVE_SECS`），超过这个
/// 时长一个字节都没收到，就判定**半死连接**（TCP 还在、对端不再发数据）主动断开重连。
///
/// 没有这一条时 `stream.next()` 会永久 `Pending`：既不返回 `Err` 也不返回 `None`，
/// 于是「不重连、不报错、界面停在某一步」——用户实测「会话输出完了但页面卡在中间
/// 某个阶段，重启应用才恢复」正是这个。（`reqwest` 的 `bytes_stream()` 默认没有
/// 读超时，`timeout(...)` 必须自己加。）
const IDLE_TIMEOUT_SECS: u64 = 50;

/// 后台读统一事件流（`GET /api/v1/events`，ADR-028 常驻水管）。
///
/// `POST /api/v1/chat/stream` 只是开关，流式输出全部经这条常驻 SSE 下发
/// （`server/src/api/chat/handlers.rs`），所以订阅必须**独立于发送**常驻。
///
/// 可靠性三条（对齐 vite 的 `EventSource` 语义；此前只有第一条的一半）：
/// 1. **断线重连**：指数退避 1s → 2s → … → [`RECONNECT_MAX_SECS`]，连上即重置；
/// 2. **心跳超时**：见 [`IDLE_TIMEOUT_SECS`]；
/// 3. **状态上抛**：`Connected` / `Disconnected` 交给 UI —— 重连成功后由 UI 重放
///    订阅，服务端随即推**快照帧**（`type=snapshot`）把窗口 replace 成权威内容，
///    这就是「断线期间丢了事件也能自己收敛」的那条兜底。
pub(crate) fn spawn_event_reader(
    port: u16,
    tx: tokio::sync::mpsc::UnboundedSender<EventStreamMsg>,
) {
    // `spawn` 返回的是 `JoinHandle`（任务**已经**开始调度，不是惰性 Future）；
    // 我们不需要 join，也不关心它何时结束——退出条件是 `tx.is_closed()`（窗口销毁）。
    // 这里显式 `drop` 掉句柄（= detach），顺带说明为什么不是 `let _ =`：clippy 的
    // `let_underscore_future` 会把 `JoinHandle` 误判成「没 await 的 Future」。
    drop(runtime().spawn(async move {
        let client = reqwest::Client::new();
        let url = format!("http://{SERVER_HOST}:{port}/api/v1/events");
        let mut backoff = Duration::from_secs(1);
        loop {
            if tx.is_closed() {
                return;
            }
            let resp = match client.get(&url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    println!(
                        "[事件] 订阅被拒（HTTP {}），{}s 后重试",
                        r.status(),
                        backoff.as_secs()
                    );
                    let _ = tx.send(EventStreamMsg::Disconnected);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(RECONNECT_MAX_SECS));
                    continue;
                }
                Err(e) => {
                    println!("[事件] 订阅失败（{}s 后重试）：{e}", backoff.as_secs());
                    let _ = tx.send(EventStreamMsg::Disconnected);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(RECONNECT_MAX_SECS));
                    continue;
                }
            };
            println!("[事件] 已订阅 {}（HTTP {}）", url, resp.status());
            // 连上即重置退避，并通知 UI 重放订阅（快照帧随后到达，replace 窗口）
            backoff = Duration::from_secs(1);
            if tx.send(EventStreamMsg::Connected).is_err() {
                return;
            }
            let mut stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            loop {
                let next =
                    tokio::time::timeout(Duration::from_secs(IDLE_TIMEOUT_SECS), stream.next())
                        .await;
                let bytes = match next {
                    Err(_) => {
                        println!(
                            "[事件] {}s 无任何数据（心跳丢失），判定连接半死 → 重连",
                            IDLE_TIMEOUT_SECS
                        );
                        break;
                    }
                    Ok(None) => {
                        println!("[事件] 流结束");
                        break;
                    }
                    Ok(Some(Err(e))) => {
                        println!("[事件] 读取中断：{e}");
                        break;
                    }
                    Ok(Some(Ok(b))) => b,
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
                    // 心跳帧（`: keep-alive` 注释行 → 没有 data 行；或空 data）→ 只用来
                    // 保鲜，不投递。**注意**：心跳帧能不能解析成 JSON 都无所谓，直接跳过。
                    if data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(&data) {
                        Ok(v) => {
                            if tx.send(EventStreamMsg::Event(v)).is_err() {
                                return;
                            }
                        }
                        Err(e) => println!("[事件] 解析失败：{e}"),
                    }
                }
            }
            let _ = tx.send(EventStreamMsg::Disconnected);
            println!("[事件] 连接结束，{}s 后重连", backoff.as_secs());
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(RECONNECT_MAX_SECS));
        }
    }));
}
