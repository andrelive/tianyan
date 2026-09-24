//! 会话租约（ADR-041）：控制面互斥 + 命令幂等 + 忙语义。
//!
//! **一张表表达「谁在跑」**（取代散落的守卫判定）：
//!
//! | 已有 \ 请求 | Stream | DestructiveOp |
//! |------------|--------|---------------|
//! | 无 | ✅ 授予 | ✅ 授予 |
//! | Stream | ❌ 409 `stream_in_progress` | ✅ **接管**（置位旧流取消标志 → 挡住新流 → 等静默） |
//! | DestructiveOp | ❌ 409 `destructive_op_in_progress` | ❌ 409（同 `opId` → 幂等重放/等待） |
//!
//! 关键不变量：
//! - **owner 令牌校验**：释放只在令牌相等时移除租约（修掉「任何后到者都能清槽」）；
//! - **静默观测与 owner 无关**：控制面接管后旧轮仍会调 [`SessionLeases::release_stream`]，
//!   此时租约已易主（不移除），但必须把 `stream_active` 置 false 并唤醒等待者——
//!   否则控制面会在旧轮仍在写会话时进入（回退截断与轮写入竞态）。
//! - **幂等**：`opId` 台账（进行中 / 已完成 + 结果）→ 双击/重试不二次执行。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Notify;

/// 租约种类：同一会话同一时刻只允许一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseKind {
    /// 数据面：用户轮 / 唤醒轮的流式往返。
    Stream,
    /// 控制面：回退 / 重做 / 压缩 / 删除会话（read-modify-write 型命令）。
    DestructiveOp,
}

/// 租约令牌：每次授予 / 接管都**新建**（不复用指针，避免 owner 误判）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseToken(u64);

/// 忙的原因（409 的结构化 `reason`——前端据此分流文案，不再一律显示「连接断开」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyReason {
    /// 有流在跑（数据面占用）。
    StreamInProgress,
    /// 有别的控制面命令在跑。
    DestructiveOpInProgress,
    /// 同一条命令（`opId`）仍在执行中（幂等等待超时）。
    SameOperationInFlight,
}

impl BusyReason {
    /// 结构化标识（响应 `reason` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            BusyReason::StreamInProgress => "stream_in_progress",
            BusyReason::DestructiveOpInProgress => "destructive_op_in_progress",
            BusyReason::SameOperationInFlight => "same_operation_in_flight",
        }
    }

    /// 面向用户的说明（响应 `error` 字段）。
    pub fn message(self) -> &'static str {
        match self {
            BusyReason::StreamInProgress => "该会话有进行中的对话流，请先停止或等待完成",
            BusyReason::DestructiveOpInProgress => {
                "该会话有进行中的会话操作（回退/重做/压缩），请等待完成"
            }
            BusyReason::SameOperationInFlight => "同一次操作仍在处理中，请稍候",
        }
    }
}

/// 会话上的租约记录。
struct SessionLease {
    kind: LeaseKind,
    owner: LeaseToken,
    /// 取消标志：`Stream` 由持有者创建；控制面**接管时沿用旧流的标志**（置位即停旧轮）。
    cancel: Arc<AtomicBool>,
    /// 控制面命令的幂等键。
    op_id: Option<String>,
    /// 该会话是否仍有流在跑：流结束时置 false（**不看 owner**，见模块文档）。
    stream_active: bool,
    /// 「会话静默」广播。
    quiet: Arc<Notify>,
}

/// 命令台账条目（幂等）。
enum OpEntry {
    InFlight(Arc<Notify>),
    Done(Value),
}

/// 控制面命令的接入结果。
#[derive(Debug)]
pub enum OpAccess {
    /// 首次执行：执行完必须 [`SessionLeases::finish_op`]（失败则 [`SessionLeases::abort_op`]）。
    /// `drain` = 接管了正在跑的流 → 先 [`SessionLeases::wait_quiet`] 等它退出。
    Owned {
        /// 租约令牌（释放 / 完成时校验 owner）。
        token: LeaseToken,
        /// 接管场景的「等静默」句柄（`None` = 无流在跑，直接执行）。
        drain: Option<Arc<Notify>>,
    },
    /// 同 `opId` 已完成：直接返回既有结果（**不重复执行**）。
    Replay {
        /// 既有结果（原样返回给调用方）。
        outcome: Value,
    },
    /// 同 `opId` 正在执行：等它完成后重放（调用方给等待上限）。
    InFlight {
        /// 命令结束的广播句柄。
        quiet: Arc<Notify>,
    },
}

/// 会话租约注册表（`AppState` 持有；`stream_cancels` 原地升级，不新增第二张表）。
pub struct SessionLeases {
    leases: Mutex<HashMap<String, SessionLease>>,
    ops: Mutex<HashMap<(String, String), OpEntry>>,
    next_token: AtomicU64,
}

impl SessionLeases {
    /// 创建注册表。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            leases: Mutex::new(HashMap::new()),
            ops: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
        })
    }

    fn lock_leases(&self) -> MutexGuard<'_, HashMap<String, SessionLease>> {
        self.leases.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_ops(&self) -> MutexGuard<'_, HashMap<(String, String), OpEntry>> {
        self.ops.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_token(&self) -> LeaseToken {
        LeaseToken(self.next_token.fetch_add(1, Ordering::Relaxed))
    }

    // ── 数据面（流） ─────────────────────────────────────────────

    /// 注册流（用户轮 / 唤醒轮）：同会话已有**任何**租约 → 忙。
    ///
    /// 返回令牌用于释放时校验 owner。
    pub fn acquire_stream(
        &self,
        session_id: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<LeaseToken, BusyReason> {
        let mut leases = self.lock_leases();
        if let Some(lease) = leases.get(session_id) {
            return Err(match lease.kind {
                LeaseKind::Stream => BusyReason::StreamInProgress,
                LeaseKind::DestructiveOp => BusyReason::DestructiveOpInProgress,
            });
        }
        let token = self.next_token();
        leases.insert(
            session_id.to_string(),
            SessionLease {
                kind: LeaseKind::Stream,
                owner: token,
                cancel,
                op_id: None,
                stream_active: true,
                quiet: Arc::new(Notify::new()),
            },
        );
        Ok(token)
    }

    /// 流结束：标记静默并唤醒等待者；租约仍属本流时移除（**接管后不移除**）。
    ///
    /// 无论租约是否已易主都要清 `stream_active`——控制面接管后靠它观测旧轮真正退出。
    pub fn release_stream(&self, session_id: &str, token: LeaseToken) {
        let mut quiet: Option<Arc<Notify>> = None;
        {
            let mut leases = self.lock_leases();
            let mut remove = false;
            if let Some(lease) = leases.get_mut(session_id) {
                lease.stream_active = false;
                quiet = Some(lease.quiet.clone());
                remove = lease.owner == token && lease.kind == LeaseKind::Stream;
            }
            if remove {
                leases.remove(session_id);
            }
        }
        if let Some(q) = quiet {
            q.notify_waiters();
        }
    }

    /// 「停止」按钮：置位当前**流**的取消标志。
    ///
    /// 控制面命令（回退/重做/压缩）期间不命中——命令是原子的，不可中途停
    /// （要停是取消前端请求本身）。
    pub fn cancel_active_stream(&self, session_id: &str) -> bool {
        let leases = self.lock_leases();
        match leases.get(session_id) {
            Some(lease) if lease.kind == LeaseKind::Stream && lease.stream_active => {
                lease.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// 该会话是否仍有流在跑。
    ///
    /// **只看 `stream_active` 字段、不看 `kind`**：控制面接管后 kind 已改写为
    /// `DestructiveOp`，但旧轮可能仍在退出途中——`wait_quiet` 依赖本判定。
    pub fn stream_active(&self, session_id: &str) -> bool {
        self.lock_leases()
            .get(session_id)
            .map(|l| l.stream_active)
            .unwrap_or(false)
    }

    // ── 控制面（命令） ───────────────────────────────────────────

    /// 控制面命令接入：幂等门 + 互斥门。
    ///
    /// 无 `op_id` 时保持非幂等（向后兼容：旧前端不传 opId 仍可用）。
    pub fn acquire_op(
        &self,
        session_id: &str,
        op_id: Option<&str>,
    ) -> Result<OpAccess, BusyReason> {
        // 幂等：同 opId 已完成 → 重放；进行中 → 等待其完成
        if let Some(id) = op_id {
            let ops = self.lock_ops();
            if let Some(entry) = ops.get(&(session_id.to_string(), id.to_string())) {
                return Ok(match entry {
                    OpEntry::Done(outcome) => OpAccess::Replay {
                        outcome: outcome.clone(),
                    },
                    OpEntry::InFlight(quiet) => OpAccess::InFlight {
                        quiet: quiet.clone(),
                    },
                });
            }
        }

        // 互斥：无租约 → 授予；Stream → 接管；DestructiveOp → 忙
        let token = self.next_token();
        let drain = {
            let mut leases = self.lock_leases();
            match leases.get_mut(session_id) {
                None => {
                    leases.insert(
                        session_id.to_string(),
                        SessionLease {
                            kind: LeaseKind::DestructiveOp,
                            owner: token,
                            cancel: Arc::new(AtomicBool::new(false)),
                            op_id: op_id.map(str::to_string),
                            stream_active: false,
                            quiet: Arc::new(Notify::new()),
                        },
                    );
                    None
                }
                Some(lease) if lease.kind == LeaseKind::DestructiveOp => {
                    return Err(BusyReason::DestructiveOpInProgress);
                }
                Some(lease) => {
                    // 接管：① 置位旧流取消标志（旧轮在轮边界退出）；② 就地改写为控制面
                    // 租约（**挡住新流注册**——否则新轮会插进回退事务）；③ 保留旧 quiet
                    // 供「等静默」。
                    lease.cancel.store(true, Ordering::Relaxed);
                    let old_cancel = lease.cancel.clone();
                    let quiet = lease.quiet.clone();
                    lease.kind = LeaseKind::DestructiveOp;
                    lease.owner = token;
                    lease.cancel = old_cancel;
                    lease.op_id = op_id.map(str::to_string);
                    Some(quiet)
                }
            }
        };

        if let Some(id) = op_id {
            self.lock_ops().insert(
                (session_id.to_string(), id.to_string()),
                OpEntry::InFlight(Arc::new(Notify::new())),
            );
        }
        Ok(OpAccess::Owned { token, drain })
    }

    /// 命令成功结束：记录结果（幂等重放用）+ 释放互斥 + 唤醒等待者。
    pub fn finish_op(&self, session_id: &str, token: LeaseToken, outcome: Value) {
        let mut op_id: Option<String> = None;
        let mut quiet: Option<Arc<Notify>> = None;
        {
            let mut leases = self.lock_leases();
            let owned = leases
                .get(session_id)
                .map(|l| l.owner == token && l.kind == LeaseKind::DestructiveOp)
                .unwrap_or(false);
            if owned {
                if let Some(lease) = leases.remove(session_id) {
                    op_id = lease.op_id;
                    quiet = Some(lease.quiet);
                }
            }
        }
        if let Some(id) = op_id {
            let prev = self
                .lock_ops()
                .insert((session_id.to_string(), id), OpEntry::Done(outcome));
            if let Some(OpEntry::InFlight(notify)) = prev {
                notify.notify_waiters();
            }
        }
        if let Some(q) = quiet {
            q.notify_waiters();
        }
    }

    /// 命令失败（或申请后未能执行）：释放互斥 + 撤销台账条目（允许重试）。
    pub fn abort_op(&self, session_id: &str, token: LeaseToken) {
        let mut op_id: Option<String> = None;
        {
            let mut leases = self.lock_leases();
            let owned = leases
                .get(session_id)
                .map(|l| l.owner == token && l.kind == LeaseKind::DestructiveOp)
                .unwrap_or(false);
            if owned {
                op_id = leases.remove(session_id).and_then(|l| l.op_id);
            }
        }
        if let Some(id) = op_id {
            let prev = self.lock_ops().remove(&(session_id.to_string(), id));
            if let Some(OpEntry::InFlight(notify)) = prev {
                notify.notify_waiters();
            }
        }
    }

    /// 查询命令结果（等待完成后重放用）：`Some` = 已完成的结果。
    pub fn op_outcome(&self, session_id: &str, op_id: &str) -> Option<Value> {
        match self
            .lock_ops()
            .get(&(session_id.to_string(), op_id.to_string()))
        {
            Some(OpEntry::Done(outcome)) => Some(outcome.clone()),
            _ => None,
        }
    }

    /// 等待「会话静默」（接管后等旧轮退出）：条件循环 + 广播，通知不丢失。
    ///
    /// 返回 `true` = 已静默；`false` = 超时（调用方应放弃本次命令，避免与仍在
    /// 写会话的旧轮竞态）。
    pub async fn wait_quiet(
        &self,
        session_id: &str,
        quiet: Arc<Notify>,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.stream_active(session_id) {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return !self.stream_active(session_id);
            }
            tokio::select! {
                _ = quiet.notified() => continue,
                _ = tokio::time::sleep(remaining) => return !self.stream_active(session_id),
            }
        }
    }

    /// 等待同 `opId` 的命令结束（幂等等待）：返回是否拿到结果。
    pub async fn wait_inflight(
        &self,
        session_id: &str,
        op_id: &str,
        quiet: Arc<Notify>,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.op_outcome(session_id, op_id).is_some() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.op_outcome(session_id, op_id).is_some();
            }
            tokio::select! {
                _ = quiet.notified() => continue,
                _ = tokio::time::sleep(remaining) => {
                    return self.op_outcome(session_id, op_id).is_some();
                }
            }
        }
    }

    /// 当前活跃租约数（观测 / 测试）。
    pub fn active_count(&self) -> usize {
        self.lock_leases().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// 互斥：流在跑 → 控制面命令接管（而不是拒绝），旧流标志被置位。
    #[test]
    fn test_op_takes_over_running_stream() {
        let leases = SessionLeases::new();
        let cancel = flag();
        leases.acquire_stream("s1", cancel.clone()).unwrap();

        let access = leases.acquire_op("s1", Some("op-1")).unwrap();
        let OpAccess::Owned { drain, .. } = access else {
            panic!("应授予（接管）控制面租约");
        };
        assert!(drain.is_some(), "接管必须带「等静默」句柄");
        assert!(
            cancel.load(Ordering::Relaxed),
            "接管须置位旧流的取消标志（旧轮在轮边界退出）"
        );
        // 挡住新流：接管期间新轮不得注册
        assert_eq!(
            leases.acquire_stream("s1", flag()),
            Err(BusyReason::DestructiveOpInProgress),
            "控制面接管期间必须挡住新流（否则新轮插进回退事务）"
        );
    }

    /// 旧轮退出：静默标记必须清除（**即使租约已易主**），否则控制面会与旧轮竞态。
    #[test]
    fn test_release_stream_after_takeover_marks_quiet() {
        let leases = SessionLeases::new();
        let stream_token = leases.acquire_stream("s1", flag()).unwrap();
        let OpAccess::Owned {
            token: op_token, ..
        } = leases.acquire_op("s1", None).unwrap()
        else {
            panic!("应授予");
        };
        assert_eq!(leases.active_count(), 1, "接管是就地改写，不新增条目");

        leases.release_stream("s1", stream_token);
        assert!(!leases.stream_active("s1"), "旧轮退出后必须报告静默");
        assert_eq!(leases.active_count(), 1, "接管后的控制面租约不得被旧轮移除");

        leases.finish_op("s1", op_token, serde_json::json!({"ok": true}));
        assert_eq!(leases.active_count(), 0, "命令结束释放租约");
    }

    /// 幂等：同 opId 第二次不再执行（已完成 → 重放；进行中 → 等待）。
    #[test]
    fn test_op_idempotency() {
        let leases = SessionLeases::new();
        let OpAccess::Owned { token, .. } = leases.acquire_op("s1", Some("op-1")).unwrap() else {
            panic!("应授予");
        };
        // 进行中：第二个同 opId 请求 → 等待（不执行）
        assert!(matches!(
            leases.acquire_op("s1", Some("op-1")).unwrap(),
            OpAccess::InFlight { .. }
        ));
        leases.finish_op("s1", token, serde_json::json!({"truncated": 2}));

        // 已完成：第三个同 opId 请求 → 重放既有结果
        match leases.acquire_op("s1", Some("op-1")).unwrap() {
            OpAccess::Replay { outcome } => assert_eq!(outcome["truncated"], 2),
            other => panic!("应重放既有结果：{other:?}"),
        }
        // 不同 opId：正常新命令
        assert!(matches!(
            leases.acquire_op("s1", Some("op-2")).unwrap(),
            OpAccess::Owned { .. }
        ));
    }

    /// 控制面互斥：另一个命令在跑 → 409（不排队、不接管）。
    #[test]
    fn test_second_op_is_busy_while_first_in_flight() {
        let leases = SessionLeases::new();
        let OpAccess::Owned { token, .. } = leases.acquire_op("s1", None).unwrap() else {
            panic!("应授予");
        };
        assert!(
            matches!(
                leases.acquire_op("s1", None),
                Err(BusyReason::DestructiveOpInProgress)
            ),
            "第二个控制面命令必须 409（destructive_op_in_progress）"
        );
        // 失败回滚后同 opId 可重试（不残留 InFlight 条目）
        leases.abort_op("s1", token);
        assert!(matches!(
            leases.acquire_op("s1", None).unwrap(),
            OpAccess::Owned { .. }
        ));
    }

    /// 「停止」只作用于流；控制面命令期间不命中（命令原子，不可中途停）。
    #[test]
    fn test_cancel_active_stream_scope() {
        let leases = SessionLeases::new();
        assert!(!leases.cancel_active_stream("s1"), "无租约 → 不命中");

        let cancel = flag();
        leases.acquire_stream("s1", cancel.clone()).unwrap();
        assert!(leases.cancel_active_stream("s1"), "流在跑 → 命中");
        assert!(cancel.load(Ordering::Relaxed));

        let OpAccess::Owned { token, .. } = leases.acquire_op("s1", None).unwrap() else {
            panic!("应授予");
        };
        assert!(
            !leases.cancel_active_stream("s1"),
            "控制面命令期间「停止」不命中"
        );
        leases.finish_op("s1", token, Value::Null);
    }

    /// 会话间互不影响（租约按会话隔离）。
    #[test]
    fn test_leases_are_per_session() {
        let leases = SessionLeases::new();
        leases.acquire_stream("s1", flag()).unwrap();
        assert!(
            leases.acquire_stream("s2", flag()).is_ok(),
            "不同会话互不影响"
        );
        assert_eq!(leases.active_count(), 2);
    }

    /// 等静默：旧轮退出即返回 true；不退出则超时 false（命令应放弃，避免竞态）。
    #[tokio::test]
    async fn test_wait_quiet_times_out_without_release() {
        let leases = SessionLeases::new();
        let stream_token = leases.acquire_stream("s1", flag()).unwrap();
        let OpAccess::Owned { drain, .. } = leases.acquire_op("s1", None).unwrap() else {
            panic!("应授予");
        };
        let drain = drain.expect("接管应带 drain");

        // 旧轮仍在跑 → 超时
        assert!(
            !leases
                .wait_quiet("s1", drain.clone(), Duration::from_millis(50))
                .await
        );
        // 旧轮退出 → 立即静默
        leases.release_stream("s1", stream_token);
        assert!(
            leases
                .wait_quiet("s1", drain, Duration::from_millis(50))
                .await,
            "旧轮退出后必须报告静默"
        );
    }
}
