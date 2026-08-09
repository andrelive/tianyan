//! 主动提醒配置（T1 路线：记忆/规则 relevant-now 评估 + 桌面通知）。

use serde::{Deserialize, Serialize};

/// 主动提醒配置。
///
/// 定时评估记忆与规则：命中"当前值得告知用户"的内容时，经
/// [`crate::notification::NotificationSink`] 推送系统通知并注入会话。
/// 默认关闭（本地助手不主动打扰，用户显式开启）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReminderConfig {
    /// 是否启用主动提醒（默认关闭）。
    pub enabled: bool,
    /// 评估间隔（秒，默认 3600 = 每小时）。
    pub interval_secs: u64,
    /// 每次运行最多提醒条数（默认 1）。
    pub max_per_run: usize,
    /// 是否同时注入到最新会话（默认 true）。
    pub inject_to_session: bool,
}

impl Default for ReminderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 3600,
            max_per_run: 1,
            inject_to_session: true,
        }
    }
}
