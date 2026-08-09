//! 系统通知通道（T1 路线：主动提醒 / 后台任务完成 / 审批挂起的桌面通知）。
//!
//! core 只定义通道抽象，不依赖桌面环境：默认 [`NoopNotificationSink`]
//! （无操作）；Tauri 装配层注入真实实现（OS 原生通知）。

use std::sync::Arc;

/// 系统通知通道。
///
/// 由装配层注入（Tauri 实现 → server → core 组件）；未注入时静默。
pub trait NotificationSink: Send + Sync {
    /// 发送一条系统通知（非阻塞；实现方自行处理线程/队列）。
    fn notify(&self, title: &str, body: &str);
}

/// 空实现：未装配通知通道时静默（行为零变化）。
pub struct NoopNotificationSink;

impl NotificationSink for NoopNotificationSink {
    fn notify(&self, _title: &str, _body: &str) {}
}

/// 便捷类型别名。
pub type SharedNotificationSink = Arc<dyn NotificationSink>;

/// 全局空通道（默认值）。
pub fn noop_sink() -> SharedNotificationSink {
    Arc::new(NoopNotificationSink)
}
