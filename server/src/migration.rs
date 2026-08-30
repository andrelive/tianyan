//! 数据目录搬迁协调（服务器关停后执行）。
//!
//! 搬迁请求由 API（POST /api/v1/config/migrate-data-dir）写入请求文件并
//! 触发优雅关停；服务器进程退出后（AppState drop，SQLite/LanceDB 释放
//! 文件锁），本模块的 [`handle_pending_migration`] 执行搬迁并重读配置。
//! 调用方（Tauri 监督循环 / 独立 server 主循环）以新配置重启。

use tianyan::config::TianyanConfig;

/// 服务器关停后调用：执行待处理的数据目录搬迁。
///
/// 返回 `Some(新配置)` = 已搬迁（调用方应以新配置重启）；
/// `None` = 无待处理请求或搬迁失败（调用方以旧配置重启）。
pub async fn handle_pending_migration(old_config: &TianyanConfig) -> Option<TianyanConfig> {
    let old_dir = old_config.storage.data_dir.clone();
    match tianyan::config::migration::run_pending_migration(&old_dir) {
        Ok(true) => {
            // 重读配置（新 data_dir 生效；失败时回退旧配置并告警）
            match TianyanConfig::load() {
                Ok(c) => {
                    tracing::info!(
                        "搬迁完成，以新配置重启（data_dir={}）",
                        c.storage.data_dir.display()
                    );
                    Some(c)
                }
                Err(e) => {
                    tracing::error!(error = %e, "搬迁后重读配置失败，使用旧配置重启");
                    Some(old_config.clone())
                }
            }
        }
        Ok(false) => None,
        Err(e) => {
            // 失败已回滚（数据在旧目录、配置未动、请求已清除）：
            // 返回旧配置让调用方重启，应用恢复正常运行
            tracing::error!(error = %e, "数据目录搬迁失败（已回滚），以旧配置重启");
            Some(old_config.clone())
        }
    }
}
