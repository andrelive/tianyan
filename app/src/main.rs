//! 天演装配器（服务器二进制入口）。
//!
//! ## 为什么单独成包（ADR-049 期 1）
//!
//! **装配清单住在 bin**。Cargo 的依赖粒度是 package，若清单写在
//! `tianyan-server` 的 bin 里，整个 server 包就依赖插件 crate —— 改一行工具会
//! 重编 server 库（13.4K 源码），闸门 A（≤3.5K）直接失效。
//!
//! 因此本包只做三件事：加载配置与日志、给出插件清单、启动 server。
//! server 库通过 `ToolPluginFactory`（类型擦除）接收清单，**不认识任何插件类型**。

use tianyan_server::{start_server_with_plugins, ToolPluginFactory};

/// 插件清单（**全仓唯一 `use` 具体插件类型的地方**）。
///
/// 新增插件在此追加一行；插件内部顺序即注册顺序（追加到动态工具末尾，
/// 内置工具顺序不受影响 —— 提示词前缀稳定性）。
fn plugin_factory() -> ToolPluginFactory {
    Box::new(|ctx| {
        // 顺序即注册顺序（追加到动态工具末尾）：fs 域在前、code 域在后。
        let mut tools = tianyan_tools_fs::tools(ctx);
        tools.extend(tianyan_tools_code::tools(ctx));
        tools
    })
}

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
        match start_server_with_plugins(server_config, config.clone(), plugin_factory()).await {
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
