//! 服务器二进制入口。

use tianyan_server::start_server_default;

#[tokio::main]
async fn main() -> tianyan::common::error::Result<()> {
    // 日志初始化：级别/格式来自配置文件 [logging] 节（RUST_LOG 环境变量可覆盖级别）
    let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
        eprintln!("配置加载失败（使用默认配置）: {}", e);
        tianyan::config::TianyanConfig::default()
    });
    tianyan::common::logging::init_logging(&config.logging)?;

    start_server_default().await
}
