//! 系统通知通道：`core::notification::NotificationSink` 的原生桌面实现。
//!
//! 与 `tauri/src/lib.rs` 的 `TauriNotificationSink` **同一个抽象、同一个注入点**：
//! server 侧有一个全局槽（`tianyan_server::notification::register_global_notification_sink`），
//! 注册之后 core 的所有通知场景自动生效，本文件不需要知道它们是谁：
//!
//! - 后台任务完成 / 失败（`core/src/agent/background.rs` 的 `sink.notify`）
//! - 主动提醒（`ReminderTask`）
//! - 审批挂起
//!
//! ## Windows toast 的两个前置（都踩过才知道）
//!
//! 1. **AppUserModelID**：WinRT 的 `CreateToastNotifier` 需要一个 AppID 归属。
//!    不注册也能弹，但通知的归属名会退回 WinRT 的默认值（`Windows PowerShell`）——
//!    用户看到的是「PowerShell 发来一条通知」，比不弹更糟。注册只写
//!    `HKCU\Software\Classes\AppUserModelId\<id>`，**不需要管理员**。
//! 2. **COM**：`show()` 走 WinRT。放在独立线程里发送（见下），线程内的 COM 由
//!    notify-rust 的 WinRT 后端自己初始化；不在 UI 线程上碰 WinRT 也避免拖慢渲染。

use std::sync::Arc;

use tianyan::notification::NotificationSink;

/// 通知归属的应用标识。
///
/// 反向域名式命名（与安装包的 ProductCode 无关）：同一个 id 在注册表里只能有一份
/// `DisplayName`，所以升级安装不需要清理，重复注册就是覆盖。
const APP_ID: &str = "Tianyan.GuiNative";

/// 通知里显示的发送者名字。
const APP_NAME: &str = "天演";

/// 原生通知通道（`notify-rust` → Windows toast）。
pub struct NativeNotificationSink;

impl NotificationSink for NativeNotificationSink {
    /// 发送一条系统通知。
    ///
    /// **非阻塞**是 trait 的契约（`notify` 是同步签名），而 `Notification::show()`
    /// 是同步走 WinRT 的。所以这里起一个短命线程发送：调用点在 agent 的任务收尾
    /// 路径上（`on_task_terminal`），不能因为通知卡住收尾。
    ///
    /// 失败只记日志不 panic：通知是尽力而为的旁路，弹不出来不该影响主流程。
    fn notify(&self, title: &str, body: &str) {
        let title = title.to_string();
        let body = body.to_string();
        std::thread::spawn(move || {
            let result = notify_rust::Notification::new()
                .app_id(APP_ID)
                .summary(&title)
                .body(&body)
                // 8s 后自动收进通知中心（不设会按系统默认，Win11 下可能长期停留）
                .timeout(notify_rust::Timeout::Milliseconds(8000))
                .show();
            if let Err(e) = result {
                tracing::warn!("系统通知发送失败：{e}");
            }
        });
    }
}

/// 注册 AppUserModelID（幂等；只写 HKCU，不需要管理员）。
///
/// 失败不阻断启动：通知会退回默认归属名，功能仍可用。
pub fn ensure_app_id() {
    if let Err(e) = try_register_app_id() {
        tracing::warn!("注册通知 AppUserModelID 失败（通知仍可弹出，但归属名会退回系统默认）：{e}");
    }
}

fn try_register_app_id() -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    let (key, _) = hkcu
        .create_subkey(&path)
        .map_err(|e| format!("创建 {path} 失败：{e}"))?;

    // DisplayName 是通知里显示的发送者（缺了它 WinRT 会退回默认 AppID 的名字）
    key.set_value("DisplayName", &APP_NAME)
        .map_err(|e| format!("写 DisplayName 失败：{e}"))?;

    // 图标：指向本程序自身的 exe —— shell 会从 PE 资源里取图标，
    // 这样不必额外带一个 .ico 文件（安装目录/便携版都能拿到正确图标）。
    if let Ok(exe) = std::env::current_exe() {
        let _ = key.set_value("IconUri", &exe.to_string_lossy().to_string());
    }
    Ok(())
}

/// 构造并注册全局通知实现（`main` 启动时调用一次）。
///
/// 装配顺序与 tauri 版一致：server 先起（core 组件已注入动态包装
/// `global_notification_sink()`），这里注册真实实现后**立即动态生效**，
/// 不需要重建任何组件。
pub fn install() {
    ensure_app_id();
    tianyan_server::notification::register_global_notification_sink(Arc::new(
        NativeNotificationSink,
    ));
    tracing::info!("系统通知通道已注册（AppID={APP_ID}）");
}

/// 发一条测试通知（自动化验收用）。
///
/// 走的是**全局通道**而不是直接 new 一个 sender：这样验证的是「注册 → 全局包装 →
/// 真实实现」整条链路，而不是绕过注册自己弹一条。
pub fn send_test_notification() {
    tianyan_server::notification::global_notification_sink().notify(
        "天演 · 测试通知",
        "这条通知来自 GUI 自动化端口，验证系统通知链路（注册 → 弹窗）已打通。",
    );
}
