//! 服务端测试宿主 — 在测试中启动完整 Axum 服务器并提供 HTTP 客户端

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tracing::info;

/// 测试服务器封装
pub struct TestServer {
    pub addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TestServer {
    /// 使用给定路由启动测试服务器
    pub async fn start(router: Router) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        info!("测试服务器启动于: {}", addr);

        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap_or_else(|e| tracing::error!("测试服务器关闭: {}", e));
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        Ok(Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    /// GET 请求
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client()
            .get(format!("{}{}", self.base_url(), path))
            .send()
            .await
            .expect("GET request failed")
    }

    /// POST 请求（JSON body）
    pub async fn post<T: serde::Serialize>(&self, path: &str, body: &T) -> reqwest::Response {
        self.client()
            .post(format!("{}{}", self.base_url(), path))
            .json(body)
            .send()
            .await
            .expect("POST request failed")
    }

    /// PUT 请求（JSON body）
    pub async fn put<T: serde::Serialize>(&self, path: &str, body: &T) -> reqwest::Response {
        self.client()
            .put(format!("{}{}", self.base_url(), path))
            .json(body)
            .send()
            .await
            .expect("PUT request failed")
    }

    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}
