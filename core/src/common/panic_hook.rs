//! panic hook：把进程内恐慌的"现场"落盘（GUI 无 stderr 时的唯一留痕手段）。
//!
//! 与 release `panic = "unwind"`（见根 `Cargo.toml`）配套：
//! - **第一防线** `unwind`：panic 被任务边界隔离（tokio `JoinError`、
//!   `catch_unwind` 可捕获），单任务 panic 不再终止整个进程；
//! - **最后防线**（本模块）：任何线程的 panic（含跨 FFI 边界等仍会 abort 的
//!   场景）都先经过 panic hook——把线程 / 位置 / 消息 / 回溯追加写入
//!   `<log_dir>/panic.log`，保证"若仍死，必留现场"。
//!
//! 设计约束（hook 内不得再失败）：
//! - 只做文件 append，不用 `tracing`（其内部持锁，panic 现场调用有死锁风险）；
//! - 所有 IO 错误静默忽略；整体再套一层 `catch_unwind` 防御——hook 内 panic
//!   会触发 double panic → 直接 abort，连默认输出都会丢失。

use std::any::Any;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

/// 默认日志目录：`{data_dir}/com.tianyan.app/logs/`（与桌面端日志目录一致；
/// data_dir 不可得时退化为临时目录）。
pub fn default_log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.tianyan.app")
        .join("logs")
}

/// 安装 panic hook（进程级；重复调用以最后一次为准）。
///
/// panic 时：先把报告 append 到 `<log_dir>/panic.log`（最重要），再转发
/// 默认 hook（保留终端 stderr 输出——服务器/开发模式可见）。日志目录不存在
/// 时尽力创建。
pub fn install(log_dir: PathBuf) {
    let _ = std::fs::create_dir_all(&log_dir);
    let panic_file = log_dir.join("panic.log");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 先落盘；hook 内任何失败都必须静默（不能再 panic）。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let report = build_report(info);
            append_report(&panic_file, &report);
        }));
        // 再转发默认 hook：保持终端模式的既有输出行为。
        default_hook(info);
    }));
}

/// 由 hook 信息组装完整报告（时间戳 + 回溯）。
fn build_report(info: &PanicHookInfo<'_>) -> String {
    let timestamp = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f %z")
        .to_string();
    let thread = std::thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| "<unnamed>".to_string());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let message = payload_to_string(info.payload());
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    format_panic_report(&timestamp, &thread, &location, &message, &backtrace)
}

/// 提取 panic 载荷文本（`panic!("...")` 的 `&str` / `format!` 的 `String`）。
fn payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// 格式化单条 panic 报告（纯函数，可测）。
pub fn format_panic_report(
    timestamp: &str,
    thread: &str,
    location: &str,
    message: &str,
    backtrace: &str,
) -> String {
    format!(
        "\n===== PANIC @ {timestamp} =====\n\
         thread: {thread}\n\
         location: {location}\n\
         message: {message}\n\
         backtrace:\n{backtrace}\n"
    )
}

/// append 写盘（尽力而为：任何错误静默忽略）。
fn append_report(path: &Path, report: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(report.as_bytes());
        let _ = f.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_panic_report_contains_fields() {
        let r = format_panic_report(
            "2026-09-14 17:00:00.000 +0800",
            "tokio-runtime-worker",
            "core/src/a.rs:1:2",
            "boom",
            "backtrace-line-1",
        );
        assert!(r.contains("PANIC @ 2026-09-14 17:00:00.000 +0800"));
        assert!(r.contains("thread: tokio-runtime-worker"));
        assert!(r.contains("location: core/src/a.rs:1:2"));
        assert!(r.contains("message: boom"));
        assert!(r.contains("backtrace-line-1"));
    }

    #[test]
    fn test_append_report_appends_in_order() {
        let dir = std::env::temp_dir().join(format!(
            "tianyan_panic_hook_test_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("panic.log");
        append_report(&path, "first-report");
        append_report(&path, "second-report");
        let content = std::fs::read_to_string(&path).expect("读回");
        let first = content.find("first-report").expect("含第一条");
        let second = content.find("second-report").expect("含第二条");
        assert!(first < second, "应按追加顺序");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_default_log_dir_suffix() {
        let p = default_log_dir();
        assert!(p.ends_with("logs"), "目录应以 logs 结尾: {p:?}");
        assert!(
            p.to_string_lossy().contains("com.tianyan.app"),
            "应含应用目录: {p:?}"
        );
    }
}
