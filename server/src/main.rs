//! 服务器二进制入口。

use tianyan_server::start_server;

#[tokio::main]
async fn main() -> tianyan::common::error::Result<()> {
    // 日志初始化只做一次（tracing 全局 subscriber 不可重复设置；
    // 搬迁重启循环复用同一日志）
    let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
        eprintln!("配置加载失败（使用默认配置）: {}", e);
        tianyan::config::TianyanConfig::default()
    });
    tianyan::common::logging::init_logging(&config.logging)?;
    // panic hook（最后防线）：panic 现场写入 <日志目录>/panic.log
    // （与桌面端同一目录：core::common::panic_hook::default_log_dir）。
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // 主循环：数据目录搬迁后以新配置重启（搬迁由 start_server 关停后的
    // handle_pending_migration 执行；无搬迁时单次运行即退出）
    loop {
        let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
            eprintln!("配置加载失败（使用默认配置）: {}", e);
            tianyan::config::TianyanConfig::default()
        });

        // 端口来源单点：from_env 读 TIANYAN_PORT（未设 → 默认 127.0.0.1:3000）。
        // e2e 借此用备用端口启动，避免与运行中的桌面应用抢占 3000。
        let server_config = tianyan_server::ServerConfig::from_env();
        match start_server(server_config, config.clone()).await {
            Ok(()) => {
                // 服务器正常关停：检查是否有待处理的数据目录搬迁
                if tianyan_server::migration::handle_pending_migration(&config)
                    .await
                    .is_some()
                {
                    eprintln!("数据目录搬迁完成，以新配置重启");
                    continue;
                }
                break;
            }
            Err(e) => {
                eprintln!("服务器错误：{}", e);
                break;
            }
        }
    }
    Ok(())
}
