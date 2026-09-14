//! panic 隔离探针：验证 release 构建（`panic = "unwind"`）下
//! ① tokio 任务内 panic → 该任务转为 `JoinError`（is_panic），进程存活；
//! ② panic hook 把现场写入 `<dir>/panic.log`。
//!
//! 运行：`cargo run --release --example panic_survival_probe`
//! （若 release 仍为 `panic = "abort"`，进程会在 ① 处直接终止——
//! 无 `SURVIVED` 输出、退出码非 0。）
//!
//! 背景：2026-09-14 闪退事故（中文截断 panic × abort 放大为全进程无痕退出）。
//! 本探针是该类事故的回归验收资产：升级 unwind 后必现 `SURVIVED`。

/// 探针日志目录（临时目录，避免污染真实日志）。
fn probe_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("tianyan_panic_probe_{}", std::process::id()))
}

#[tokio::main]
async fn main() {
    let dir = probe_dir();
    let _ = std::fs::remove_dir_all(&dir);

    // 安装 hook（与生产入口同一实现），落盘到探针临时目录。
    tianyan::common::panic_hook::install(dir.clone());

    // ① unwind 隔离：任务内 panic 必须转为 JoinError，而非终止进程。
    let handle = tokio::spawn(async {
        panic!("panic_survival_probe: intentional task panic");
    });
    let joined = handle.await;
    let is_panic = joined.as_ref().err().map(|e| e.is_panic()).unwrap_or(false);
    assert!(
        is_panic,
        "① 任务 panic 应转为 JoinError::is_panic()——若为 panic=abort 构建，进程已在此前终止"
    );
    println!("JOIN_ERROR_IS_PANIC: true");

    // ② hook 落盘：panic 现场必须写入 panic.log。
    let log_path = dir.join("panic.log");
    let content = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        content.contains("panic_survival_probe: intentional task panic"),
        "② panic 报告应落盘: {log_path:?}"
    );
    assert!(content.contains("thread:"), "② 报告应含线程信息");
    assert!(content.contains("backtrace:"), "② 报告应含回溯段");
    println!("HOOK_REPORT_OK: {}", log_path.display());

    // ③ 进程继续存活（能跑到这一行即为证据）。
    println!("SURVIVED");
}
