//! 会话工作集（ADR-035）：会话级物化上下文缓存与生命周期管理。
//!
//! 定位（ADR-035 §0）：
//! - **非第二权威**：SQLite 会话表仍是唯一权威，工作集是缓存——可随时卸载重建，
//!   任何不一致以库为准（`ensure` 每次做廉价新鲜度校验，发现库尾变化即重建）。
//! - **非新存储抽象**：不新增持久化，读写仍经 [`SessionStore`]。
//! - **per-session 锁的唯一持有者**：ADR-013 串行化的锁表由本注册表承载
//!   （`lock` 排队 / `try_lock` 幂等两种获取方式，见 [`WorkingSetRegistry`]）。
//!
//! 解决的问题（ADR-035 背景）：每轮入口全量 `load_and_build_state`、轮之间不共享、
//! 读取进度靠旁路水位记账。工作集把「会话内存态」从「一轮」提升到「一会话」，
//! 并以 `last_consumed_seq` 表达「读到哪了」（推导值，非独立水位）。
//!
//! 内存布局说明：工作集当前保留**完整链**（与 ADR-027 的「内存态不裁剪」一致——
//! 快照索引与回退依赖完整链长度）；「只物化压缩点后」的内存优化需同时改造快照
//! 索引与回退路径，留待后续批次（ADR-035 §2 修订）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard, RwLock};

use crate::agent::session_state::SessionState;
use crate::common::error::{Result, TianyanError};
use crate::common::types::StructuredMessage;
use crate::session::store::SessionStore;

/// 工作集空闲失效时长（ADR-035 §7）：超过此时长未活动 → 卸载（纯缓存语义）。
pub const WORKING_SET_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// 会话锁表清理阈值（超过后清理无持有者条目，防长驻服务内存泄漏）。
const LOCK_PRUNE_THRESHOLD: usize = 256;

/// 机会式空闲清扫的最小间隔（避免每次 `ensure` 都遍历工作集表）。
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// 单会话工作集：物化上下文 + 读取进度 + 活动时间。
pub struct SessionWorkingSet {
    session_id: String,
    store: Arc<SessionStore>,
    /// 会话内存态（完整链；`load_and_build_state` 的返回源）。
    state: Arc<RwLock<SessionState>>,
    /// 库尾 seq（-1 = 无消息）。与 `state.structured_messages` 同源推进。
    last_seq: AtomicI64,
    /// 最后一次被 loop 消费到的上界（ADR-035 §4：读取进度的推导值，
    /// 供「轮边界续跑判定」与「唤醒幂等」使用；非独立水位机制）。
    last_consumed_seq: AtomicI64,
    /// 最后活动时间（空闲卸载判定）。
    last_activity: TokioMutex<Instant>,
}

impl SessionWorkingSet {
    /// 创建空工作集（内部使用；加载经 [`WorkingSetRegistry::ensure`]）。
    fn new(session_id: &str, store: Arc<SessionStore>) -> Self {
        Self {
            session_id: session_id.to_string(),
            store,
            state: Arc::new(RwLock::new(SessionState::new(session_id))),
            last_seq: AtomicI64::new(-1),
            last_consumed_seq: AtomicI64::new(-1),
            last_activity: TokioMutex::new(Instant::now()),
        }
    }

    /// 会话 ID。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 会话内存态句柄（`load_and_build_state` 的返回源）。
    pub fn state(&self) -> Arc<RwLock<SessionState>> {
        self.state.clone()
    }

    /// 库尾 seq（-1 = 无消息）。
    pub fn last_seq(&self) -> i64 {
        self.last_seq.load(Ordering::SeqCst)
    }

    /// 最后一次被 loop 消费到的上界。
    pub fn last_consumed_seq(&self) -> i64 {
        self.last_consumed_seq.load(Ordering::SeqCst)
    }

    /// 标记「已消费到 seq」（轮组装/增量注入后调用）。
    ///
    /// 单调不回退：并发/乱序调用不会把进度推回（防止重复投递）。
    pub fn mark_consumed(&self, seq: i64) {
        let mut current = self.last_consumed_seq.load(Ordering::SeqCst);
        while seq > current {
            match self.last_consumed_seq.compare_exchange(
                current,
                seq,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// 是否有「未被消费的新消息」（续跑判定 / 唤醒幂等的唯一依据）。
    pub fn has_unconsumed(&self) -> bool {
        self.last_seq() > self.last_consumed_seq()
    }

    /// 刷新活动时间（任何读写路径调用）。
    pub async fn touch(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    /// 空闲判定（超过 ttl 未活动）。
    pub async fn is_idle(&self, ttl: Duration) -> bool {
        self.last_activity.lock().await.elapsed() > ttl
    }

    /// 从库重建工作集（缓存丢失/新鲜度不符时调用）。
    ///
    /// 语义：**重建即视为「已读到库尾」**（ADR-035 §4）——重启/卸载后首次组装
    /// 必然读到库尾，历史通知因此不会被当作「未消费」而重放（T1-24 的
    /// 「不重现」在这一层兑现）。
    async fn rebuild(&self) -> Result<()> {
        let loaded = self.store.load(&self.session_id).await?;
        let (messages, injectable) = match loaded {
            Some((header, messages)) => (messages, header.injectable_snapshot.unwrap_or_default()),
            None => (Vec::new(), crate::common::types::InjectableContext::new()),
        };
        let len = messages.len() as i64;
        {
            let mut st = self.state.write().await;
            st.structured_messages = messages;
            // 前缀快照随会话固化（ADR-012/030）：有快照则恢复，无快照留空待首轮填充
            if !injectable.soul.is_empty() || !injectable.rules_and_experiences.is_empty() {
                st.injectable_context = injectable;
            }
            st.last_activity = Instant::now();
        }
        self.last_seq.store(len - 1, Ordering::SeqCst);
        self.last_consumed_seq.store(len - 1, Ordering::SeqCst);
        Ok(())
    }

    /// 新鲜度校验（廉价：一次 `MAX(seq)` 查询）：库尾与缓存不符 → 重建。
    ///
    /// 这是「工作集不承担一致性责任」的落地：外部直接改库（如 server 侧
    /// `rewrite_messages` 尚未收口到工作集的阶段）后，下一次 `ensure` 自动
    /// 自愈，无需调用方感知。
    async fn ensure_fresh(&self) -> Result<()> {
        let db_last = self.store.last_seq(&self.session_id).await?;
        if db_last != self.last_seq() {
            tracing::debug!(
                session = %self.session_id,
                cached = self.last_seq(),
                db = db_last,
                "工作集新鲜度不符，重建"
            );
            self.rebuild().await?;
        }
        Ok(())
    }

    /// 追加消息（写入收口 ①，ADR-035 §3）：落库（原子取号）→ 更新缓存 → 返回 seq。
    ///
    /// `index_fts = false` 用于子智能体会话（ADR-026：不索引 FTS）。
    pub async fn append(&self, msg: &StructuredMessage, index_fts: bool) -> Result<i64> {
        if index_fts {
            self.store.append_message(&self.session_id, msg).await?;
        } else {
            self.store
                .append_message_no_fts(&self.session_id, msg)
                .await?;
        }
        let mut st = self.state.write().await;
        st.structured_messages.push(msg.clone());
        st.last_activity = Instant::now();
        let seq = st.structured_messages.len() as i64 - 1;
        drop(st);
        self.last_seq.store(seq, Ordering::SeqCst);
        Ok(seq)
    }

    /// 取「某个 seq 之后」的增量条目（轮边界续跑注入，ADR-035 §4）。
    ///
    /// 调用方传入自己的 `last_consumed_seq`（即上下文上界）；返回空表示无新消息。
    /// 传入值小于 0（尚无上下文）时返回全量——首次组装即覆盖完整链。
    pub async fn delta_since(&self, seq: i64) -> Vec<(i64, StructuredMessage)> {
        let st = self.state.read().await;
        let start = (seq + 1).max(0) as usize;
        if start >= st.structured_messages.len() {
            return Vec::new();
        }
        st.structured_messages[start..]
            .iter()
            .enumerate()
            .map(|(i, m)| ((start + i) as i64, m.clone()))
            .collect()
    }

    /// 消息总数（完整链长度——快照索引与压缩判定依赖）。
    pub async fn message_count(&self) -> usize {
        self.state.read().await.structured_messages.len()
    }
}

/// 工作集注册表：生命周期（ensure / remove / sweep_idle）+ per-session 锁。
///
/// 锁表由本注册表承载（ADR-035 §6：锁的唯一持有者，不引入第二把锁）。
/// 两种获取方式语义不同，**不可混用**：
/// - [`Self::lock`]：排队等待（用户消息——输入必须被处理，不能丢弃）；
/// - [`Self::try_lock`]：拿不到即返回 `None`（通知唤醒——幂等，已有 loop 则无需
///   再启一个；ADR-035 §4）。
pub struct WorkingSetRegistry {
    store: Option<Arc<SessionStore>>,
    sets: TokioMutex<HashMap<String, Arc<SessionWorkingSet>>>,
    locks: TokioMutex<HashMap<String, Arc<TokioMutex<()>>>>,
    /// 上次机会式清扫时间（节流用）。
    last_sweep: TokioMutex<Instant>,
}

impl WorkingSetRegistry {
    /// 创建注册表。`store = None`（测试桩/未装配会话存储）时 `ensure` 不可用，
    /// 锁与生命周期管理仍可用。
    pub fn new(store: Option<Arc<SessionStore>>) -> Arc<Self> {
        Arc::new(Self {
            store,
            sets: TokioMutex::new(HashMap::new()),
            locks: TokioMutex::new(HashMap::new()),
            last_sweep: TokioMutex::new(Instant::now()),
        })
    }

    /// 是否已装配会话存储（工作集可用）。
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// 取（或重建）会话工作集；同时做廉价新鲜度校验。
    ///
    /// # Errors
    /// 未装配会话存储，或库读取失败时返回错误（调用方可回退旧路径）。
    pub async fn ensure(&self, session_id: &str) -> Result<Arc<SessionWorkingSet>> {
        let store = self.store.clone().ok_or_else(|| {
            TianyanError::not_found("工作集：未装配会话存储（session_store 缺失）")
        })?;

        let existing = {
            let sets = self.sets.lock().await;
            sets.get(session_id).cloned()
        };
        let ws = match existing {
            Some(ws) => ws,
            None => {
                let ws = Arc::new(SessionWorkingSet::new(session_id, store));
                ws.rebuild().await?;
                self.sets
                    .lock()
                    .await
                    .insert(session_id.to_string(), ws.clone());
                return Ok(ws);
            }
        };
        ws.ensure_fresh().await?;
        ws.touch().await;
        self.maybe_sweep().await;
        Ok(ws)
    }

    /// 机会式空闲清扫（节流）——不引入常驻任务：访问点（`ensure`）本身就是
    /// 「有人在用」的信号，顺手清理长期不用的**其它**会话（ADR-035 §7）。
    async fn maybe_sweep(&self) {
        {
            let last = self.last_sweep.lock().await;
            if last.elapsed() < SWEEP_INTERVAL {
                return;
            }
        }
        *self.last_sweep.lock().await = Instant::now();
        let unloaded = self.sweep_idle(WORKING_SET_IDLE_TTL).await;
        if unloaded > 0 {
            tracing::debug!(unloaded, "工作集空闲卸载（ADR-035 §7）");
        }
    }

    /// 取已加载的工作集（不重建、不校验）。
    pub async fn get(&self, session_id: &str) -> Option<Arc<SessionWorkingSet>> {
        self.sets.lock().await.get(session_id).cloned()
    }

    /// 移除工作集（会话删除，写入收口 ④）。
    pub async fn remove(&self, session_id: &str) {
        self.sets.lock().await.remove(session_id);
    }

    /// 获取会话锁（排队）。用户消息轮与唤醒轮共用，实现单写者语义。
    pub async fn lock(&self, session_id: &str) -> OwnedMutexGuard<()> {
        let lock = self.session_lock(session_id).await;
        lock.lock_owned().await
    }

    /// 尝试获取会话锁（幂等，不排队）。用于通知唤醒（ADR-035 §4）：
    /// 拿不到 = 该会话已有 loop 在跑 → 无需再启一个（no-op）。
    pub async fn try_lock(&self, session_id: &str) -> Option<OwnedMutexGuard<()>> {
        let lock = self.session_lock(session_id).await;
        lock.try_lock_owned().ok()
    }

    /// 取（或建）会话锁句柄；顺带按阈值清理无持有者条目。
    async fn session_lock(&self, session_id: &str) -> Arc<TokioMutex<()>> {
        let mut map = self.locks.lock().await;
        if map.len() > LOCK_PRUNE_THRESHOLD {
            map.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        map.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone()
    }

    /// 卸载空闲工作集，返回卸载数量（ADR-035 §7：纯缓存语义，卸载不写库）。
    ///
    /// **正在使用中的会话不卸载**：锁被持有（`strong_count > 1`）= 有 loop 在跑
    /// （长轮内可能久未 touch），卸载会导致该轮的状态与缓存分叉。
    pub async fn sweep_idle(&self, ttl: Duration) -> usize {
        let victims: Vec<String> = {
            let sets = self.sets.lock().await;
            let locks = self.locks.lock().await;
            let mut out = Vec::new();
            for (sid, ws) in sets.iter() {
                let in_use = locks
                    .get(sid)
                    .map(|l| Arc::strong_count(l) > 1)
                    .unwrap_or(false);
                if !in_use && ws.is_idle(ttl).await {
                    out.push(sid.clone());
                }
            }
            out
        };
        let mut sets = self.sets.lock().await;
        for sid in &victims {
            sets.remove(sid);
        }
        victims.len()
    }

    /// 已加载的工作集数量（观测/测试用）。
    pub async fn loaded_count(&self) -> usize {
        self.sets.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::MessageRole;
    use crate::session::SessionHeader;

    async fn test_store() -> Arc<SessionStore> {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        SessionStore::new(db).unwrap()
    }

    fn user_msg(sid: &str, text: &str) -> StructuredMessage {
        StructuredMessage::user(sid, text)
    }

    async fn create_session(store: &SessionStore, sid: &str) {
        store.create(sid, &SessionHeader::default()).await.unwrap();
    }

    #[tokio::test]
    async fn test_ensure_rebuilds_and_reuses() {
        let store = test_store().await;
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        create_session(&store, "s1").await;
        store
            .append_message("s1", &user_msg("s1", "第一条"))
            .await
            .unwrap();

        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 0, "重建后库尾 seq 应为 0");
        assert_eq!(ws.message_count().await, 1);
        // 复用同一实例（不重新读库）
        let ws2 = reg.ensure("s1").await.unwrap();
        assert!(Arc::ptr_eq(&ws, &ws2), "命中缓存应返回同一实例");
        assert_eq!(reg.loaded_count().await, 1);
    }

    #[tokio::test]
    async fn test_rebuild_marks_all_consumed() {
        // ADR-035 §4：重建即视为「已读到库尾」——历史消息不会被当作未消费
        let store = test_store().await;
        create_session(&store, "s1").await;
        for i in 0..3 {
            store
                .append_message("s1", &user_msg("s1", &format!("m{i}")))
                .await
                .unwrap();
        }
        let reg = WorkingSetRegistry::new(Some(store));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 2);
        assert_eq!(ws.last_consumed_seq(), 2, "重建后应视为已消费到库尾");
        assert!(!ws.has_unconsumed(), "重建后不应有未消费消息（T1-24 语义）");
    }

    #[tokio::test]
    async fn test_append_advances_last_seq_and_delta() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store));
        let ws = reg.ensure("s1").await.unwrap();
        let seq = ws.append(&user_msg("s1", "新消息"), true).await.unwrap();

        assert_eq!(seq, 0);
        assert_eq!(ws.last_seq(), 0);
        assert!(ws.has_unconsumed(), "追加后应有未消费消息");

        let delta = ws.delta_since(ws.last_consumed_seq()).await;
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].0, 0);
        assert_eq!(delta[0].1.role, MessageRole::User);

        ws.mark_consumed(0);
        assert!(!ws.has_unconsumed());
        assert!(ws.delta_since(0).await.is_empty());
    }

    #[tokio::test]
    async fn test_mark_consumed_is_monotonic() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store));
        let ws = reg.ensure("s1").await.unwrap();
        ws.mark_consumed(5);
        ws.mark_consumed(3); // 回退不应生效（防重复投递）
        assert_eq!(ws.last_consumed_seq(), 5);
    }

    #[tokio::test]
    async fn test_ensure_fresh_rebuilds_on_external_write() {
        // 外部直接改库（尚未收口到工作集的写入路径）→ 下一次 ensure 自愈
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), -1);

        store
            .append_message("s1", &user_msg("s1", "外部写入"))
            .await
            .unwrap();

        let ws2 = reg.ensure("s1").await.unwrap();
        assert!(Arc::ptr_eq(&ws, &ws2), "同一实例");
        assert_eq!(ws.last_seq(), 0, "新鲜度校验后应重建到库尾");
    }

    #[tokio::test]
    async fn test_try_lock_is_idempotent() {
        // ADR-035 §4 判别力基座：已有 loop 持锁 → try_lock 拿不到（幂等不排队）
        let store = test_store().await;
        let reg = WorkingSetRegistry::new(Some(store));
        let guard = reg.lock("s1").await;
        // 带超时：若注入旧行为（`try_lock` 改排队 `lock`，见实证）则超时 → 断言红
        // （而不是无限挂起）
        let res = tokio::time::timeout(Duration::from_millis(500), reg.try_lock("s1")).await;
        assert!(
            res.is_ok(),
            "try_lock 必须立即返回，不得排队等待（旧行为 = 每条通知各排一轮）"
        );
        assert!(res.unwrap().is_none(), "已持锁时 try_lock 应返回 None");
        drop(guard);
        assert!(reg.try_lock("s1").await.is_some(), "释放后可获取");
    }

    #[tokio::test]
    async fn test_sweep_idle_unloads_and_rebuilds() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        store
            .append_message("s1", &user_msg("s1", "m"))
            .await
            .unwrap();
        let reg = WorkingSetRegistry::new(Some(store));
        reg.ensure("s1").await.unwrap();
        assert_eq!(reg.loaded_count().await, 1);

        // TTL = 0 → 立即视为空闲
        assert_eq!(reg.sweep_idle(Duration::from_secs(0)).await, 1);
        assert_eq!(reg.loaded_count().await, 0);

        // 再访问重建一致（纯缓存语义）
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 0);
        assert_eq!(ws.message_count().await, 1);
    }

    #[tokio::test]
    async fn test_sweep_idle_skips_in_use_session() {
        // 正在跑 loop 的会话不得被卸载（否则该轮状态与缓存分叉）
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store));
        reg.ensure("s1").await.unwrap();
        let guard = reg.lock("s1").await; // 模拟 loop 持锁运行
        assert_eq!(
            reg.sweep_idle(Duration::from_secs(0)).await,
            0,
            "持锁中的会话不得卸载"
        );
        drop(guard);
        assert_eq!(reg.sweep_idle(Duration::from_secs(0)).await, 1);
    }

    #[tokio::test]
    async fn test_ensure_without_store_fails() {
        let reg = WorkingSetRegistry::new(None);
        assert!(!reg.has_store());
        assert!(reg.ensure("s1").await.is_err());
        // 锁仍可用（无 store 场景下串行化不退化）
        let guard = reg.lock("s1").await;
        drop(guard);
    }
}
