//! 服务器二进制入口。

use tianyan_server::start_server_default;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    start_server_default().await
}
