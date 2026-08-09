//! 事件驱动触发配置（T1 路线：文件监听 + webhook）。

use serde::{Deserialize, Serialize};

/// 事件驱动触发配置。
///
/// 自动化从"仅定时"扩展为"定时 + 事件"：文件系统变化与外部 webhook
/// 进入统一事件总线，按规则触发任务执行或唤醒主 agent（ADR-013 通路）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventsConfig {
    /// 是否启用事件驱动触发（默认关闭）。
    pub enabled: bool,
    /// 文件监听目录（相对路径或绝对路径；空 = 不监听文件系统）。
    pub watch_dirs: Vec<String>,
    /// 监听事件类型（create/write/remove/rename，默认全部）。
    pub watch_events: Vec<String>,
    /// 事件触发规则：事件匹配 → 动作。
    ///
    /// 每项格式：`<pattern> => <action>`，pattern 为路径 glob 或
    /// `webhook:<名称>`，action 为 `task:<规则任务名>` 或 `wake:<会话前缀>`。
    pub rules: Vec<String>,
    /// webhook 接入令牌（POST /api/v1/events 校验；空 = 不校验，
    /// 仅本机可访问时安全）。
    pub webhook_token: String,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            watch_dirs: Vec::new(),
            watch_events: vec![
                "create".to_string(),
                "write".to_string(),
                "remove".to_string(),
            ],
            rules: Vec::new(),
            webhook_token: String::new(),
        }
    }
}
