//! 取消等待原语（结构性取消的公共辅助）。
//!
//! 背景：取消信号是全链路统一的 `Arc<AtomicBool>`（停止端点 / 服务关停 /
//! 唤醒轮注册槽均置位同一份），但"置位"只是写一个标志——若等待方正在
//! await 阻塞（LLM 请求在飞、流式 recv 间隙、上下文组装、压缩），同步
//! 检查点覆盖不到，取消要等阻塞自然结束才生效（实测最长 15s+）。
//!
//! 本模块补齐"可等待"：await 阻塞点与 [`wait_cancelled`] 竞争（select），
//! 取消先到 → 该 future 被 drop——**结构性取消**：future 内的一切在途
//! 操作（HTTP 请求、通道等待、退避重试）随之自动终止，无需逐层传导
//! 取消信号，下层（model / context）签名零改动。
//!
//! 粒度：50ms 检查间隔（与工具执行层既有取消粒度一致，
//! 见 `executor::command::execute_command_action_cancellable`）。后续计划
//! 如需 0ms 信号级（Notify/watch 通知），只改本文件内部实现——调用点
//! （`cancellable` 的四个使用处）无需变化。

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 取消检查间隔（与工具执行层 50ms 粒度一致）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 等待取消置位。
///
/// - 已置位：立即返回（零延迟）；
/// - 未置位：每 50ms 检查一次，置位后最迟一个间隔内返回；
/// - `None`：永不就绪（供 `select!` 中作为"不可取消"分支，不影响其它分支）。
pub(crate) async fn wait_cancelled(cancel: Option<&AtomicBool>) {
    match cancel {
        None => std::future::pending::<()>().await,
        Some(flag) => {
            while !flag.load(Ordering::Relaxed) {
                tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
            }
        }
    }
}

/// 与取消信号竞争任意 future。
///
/// 返回 `None` = 取消先到（`fut` 被 drop——结构性取消，在途操作自动终止）；
/// 返回 `Some(输出)` = future 正常完成（无论其内部成败）。
///
/// 快速路径：cancel 已置位时**不启动** `fut`（避免"点击停止后仍发起新
/// LLM 请求 / 新压缩"）；未置位时进入 select 竞争。
///
/// 本函数不改变取消的**收尾语义**——调用方拿到 `None` 后按其上下文走既有
/// 路径（流式收尾 / Cancelled 结果 / 跳过压缩）。
pub(crate) async fn cancellable<F: Future>(
    cancel: Option<&AtomicBool>,
    fut: F,
) -> Option<F::Output> {
    match cancel {
        None => Some(fut.await),
        Some(flag) => {
            if flag.load(Ordering::Relaxed) {
                return None;
            }
            tokio::select! {
                out = fut => Some(out),
                _ = wait_cancelled(Some(flag)) => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 已置位：等待立即返回（零延迟）。
    #[tokio::test]
    async fn test_wait_cancelled_returns_immediately_when_pre_set() {
        let flag = AtomicBool::new(true);
        tokio::time::timeout(Duration::from_millis(200), wait_cancelled(Some(&flag)))
            .await
            .expect("预置位应零延迟返回");
    }

    /// 等待期置位：最迟一个粒度内返回。
    #[tokio::test]
    async fn test_wait_cancelled_wakes_within_interval() {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let waiter = tokio::spawn(async move { wait_cancelled(Some(&f)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        flag.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_millis(1000), waiter)
            .await
            .expect("置位后应在一个粒度内唤醒")
            .expect("等待任务不应 panic");
    }

    /// `None`：永不就绪（select 语义下不影响其它分支）。
    #[tokio::test]
    async fn test_wait_cancelled_none_is_pending() {
        let r = tokio::time::timeout(Duration::from_millis(100), wait_cancelled(None)).await;
        assert!(r.is_err(), "None 应保持 pending");
    }

    /// 正常完成：`Some(输出)` 透传。
    #[tokio::test]
    async fn test_cancellable_returns_output_when_not_cancelled() {
        let flag = AtomicBool::new(false);
        let out = cancellable(Some(&flag), async { 42 }).await;
        assert_eq!(out, Some(42));
    }

    /// 预置位：不启动 future（快速路径）。
    #[tokio::test]
    async fn test_cancellable_pre_set_does_not_start_future() {
        let flag = AtomicBool::new(true);
        let started = Arc::new(AtomicBool::new(false));
        let s = started.clone();
        let out = cancellable(Some(&flag), async move {
            s.store(true, Ordering::Relaxed);
            1
        })
        .await;
        assert!(out.is_none(), "预置位应返回 None");
        assert!(!started.load(Ordering::Relaxed), "预置位不得启动 future");
    }

    /// 挂起 future + 等待期置位：取消打断（future 被 drop）。
    #[tokio::test]
    async fn test_cancellable_breaks_pending_future_on_cancel() {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let handle =
            tokio::spawn(async move { cancellable(Some(&f), std::future::pending::<()>()).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        flag.store(true, Ordering::Relaxed);
        let r = tokio::time::timeout(Duration::from_millis(1000), handle)
            .await
            .expect("挂起 future 应被取消打断")
            .expect("任务不应 panic");
        assert!(r.is_none());
    }

    /// `None`：直接等待 future（不参与竞争）。
    #[tokio::test]
    async fn test_cancellable_none_awaits_future() {
        let out = cancellable(None, async { 7 }).await;
        assert_eq!(out, Some(7));
    }
}
