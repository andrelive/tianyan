//! 统一 HTTP 客户端工厂。
//!
//! 全库 reqwest 客户端构造的单点：超时 / 连接超时 / 连接池 / UA 策略一处定义。
//! 禁止在模块内复制 `reqwest::Client::builder()` 样板；构建失败上抛（不静默降级）。
//!
//! ⚠️ 超时语义（ADR-023 之后的流式修正）：`timeout` 是**读空闲超时**
//! （单次 read 之间无字节的最长间隔），**不是总请求时长**——SSE 流式响应
//! （思维链 + 长正文）总时长可达数分钟，若用 reqwest `.timeout()` 限制
//! 总时长，长生成会在中途被客户端自己掐断（表现为流式静默中断）。

use std::time::Duration;

use crate::common::error::{Result, TianyanError};

/// HTTP 客户端规格（构造点的公共参数）。
pub struct HttpClientSpec {
    /// 读空闲超时：单次 read 之间无字节的最长间隔（流式总时长不受限）。
    pub timeout: Duration,
    /// 连接超时。
    pub connect_timeout: Duration,
    /// User-Agent（None 用 reqwest 默认）。
    pub user_agent: Option<String>,
}

/// 构建 HTTP 客户端（统一连接池策略：每主机空闲连接 5、空闲超时 90s）。
pub fn build_http_client(spec: &HttpClientSpec) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .read_timeout(spec.timeout)
        .connect_timeout(spec.connect_timeout)
        .pool_max_idle_per_host(5)
        .pool_idle_timeout(Duration::from_secs(90));
    if let Some(ua) = &spec.user_agent {
        builder = builder.user_agent(ua);
    }
    builder
        .build()
        .map_err(|e| TianyanError::config(format!("构建 HTTP 客户端失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 回归（流式静默中断）：活跃的 SSE 长流不被总时长限制掐断。
    ///
    /// 服务端每 100ms 发一块、持续 2 秒；客户端读空闲超时 300ms——
    /// 字节间隔（100ms）远小于空闲超时，流必须完整收完。若错误地把
    /// 300ms 当总时长限制（旧实现），流会在 0.3 秒处被客户端掐断。
    #[tokio::test]
    async fn test_read_timeout_allows_long_active_streams() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body: String = (0..20).map(|i| format!("data: chunk-{}\n\n", i)).collect();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // 先读完请求头——hyper 未写完请求就收到响应会报 UnexpectedMessage
            let mut buf = [0u8; 8192];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            stream.write_all(header.as_bytes()).unwrap();
            // 20 块 × 100ms = 2 秒的总时长（远超 300ms 的"总时长"），
            // 逐块写出（Content-Length 已声明，语义合法）
            for i in 0..20 {
                std::thread::sleep(Duration::from_millis(100));
                let line = format!("data: chunk-{}\n\n", i);
                stream.write_all(line.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });

        let client = build_http_client(&HttpClientSpec {
            timeout: Duration::from_millis(300),
            connect_timeout: Duration::from_secs(1),
            user_agent: None,
        })
        .unwrap();

        let resp = client
            .get(format!("http://{}/stream", addr))
            .send()
            .await
            .unwrap();
        let body = resp.text().await.unwrap();

        server.join().unwrap();
        // 20 块全部到达——总时长 2s ≫ 300ms 的"空闲超时"，但活跃流不该被掐
        for i in 0..20 {
            assert!(
                body.contains(&format!("chunk-{}", i)),
                "活跃长流被提前掐断，缺少 chunk-{}；body={}",
                i,
                body
            );
        }
    }

    // 注：停滞流的读空闲超时不依赖 reqwest 的 read_timeout（其对本层
    // body 停滞不可靠），由 provider 流式层逐块空闲超时保证——见
    // model/provider/chat.rs create_raw_stream 与其测试。
}
