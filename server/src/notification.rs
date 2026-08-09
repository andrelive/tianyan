//! 系统通知全局注册点（T1 路线：主动提醒 / 后台任务完成 / 审批挂起）。
//!
//! server 是 core 与桌面壳之间的装配层：core 组件（后台任务管理器、
//! ReminderTask）在构建时注入 [`global_notification_sink`]（动态包装），
//! Tauri 桌面壳在启动后调用 [`register_global_notification_sink`] 注入真实
//! 实现——包装器每次通知读取当前实现，注册前后均动态生效，无需重建组件。

use std::sync::Arc;

use tokio::sync::RwLock;

use tianyan::notification::{NoopNotificationSink, SharedNotificationSink};

/// 全局通知实现槽。
static GLOBAL_SINK: RwLock<Option<SharedNotificationSink>> = RwLock::const_new(None);

/// 注册全局通知实现（Tauri 启动后调用；重复注册覆盖）。
///
/// tokio RwLock 无 poison 语义；try_write 失败（并发写者）时跳过——
/// 注册仅发生在启动装配期，实际不可能并发。
pub fn register_global_notification_sink(sink: SharedNotificationSink) {
    if let Ok(mut guard) = GLOBAL_SINK.try_write() {
        *guard = Some(sink);
    }
}

/// 获取动态通知包装器（core 组件构建时注入此包装，读取当前全局实现）。
pub fn global_notification_sink() -> SharedNotificationSink {
    Arc::new(GlobalSinkRef)
}

/// 动态通知包装：每次通知读取全局槽当前值。
struct GlobalSinkRef;

impl tianyan::notification::NotificationSink for GlobalSinkRef {
    fn notify(&self, title: &str, body: &str) {
        let current = GLOBAL_SINK.try_read().ok().and_then(|guard| guard.clone());
        match current {
            Some(sink) => sink.notify(title, body),
            None => NoopNotificationSink.notify(title, body),
        }
    }
}

/// 导出别名（server 内部使用）。
pub type SharedSink = SharedNotificationSink;
