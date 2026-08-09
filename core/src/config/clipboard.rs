//! 剪贴板配置（T1 路线：复制即记忆——隐私敏感，默认关闭 opt-in）。

use serde::{Deserialize, Serialize};

/// 剪贴板 I/O 配置。
///
/// 隐私敏感功能：默认全部关闭（不监听系统剪贴板、行为零变化）；
/// 用户显式开启后才监听，且沉淀仍需确认（除非关闭确认条）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    /// 是否启用剪贴板监听（默认关闭）。
    pub enabled: bool,
    /// 捕获到复制内容后是否自动沉淀（跳过确认条）。
    ///
    /// 默认 false：捕获后仍弹确认条，用户确认才沉淀（不自动存储）。
    pub auto_capture: bool,
    /// 捕获后是否显示确认条（默认 true；auto_capture=true 时忽略）。
    pub prompt_confirm: bool,
    /// 监听剪贴板变化的最小间隔（毫秒，默认 1000；防抖动）。
    pub poll_interval_ms: u64,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_capture: false,
            prompt_confirm: true,
            poll_interval_ms: 1000,
        }
    }
}
