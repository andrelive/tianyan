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
use crate::common::types::{MessageRole, StructuredMessage};
use crate::session::store::SessionStore;

/// 工作集空闲失效时长（ADR-035 §7）：超过此时长未活动 → 卸载（纯缓存语义）。
pub const WORKING_SET_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// 会话锁表清理阈值（超过后清理无持有者条目，防长驻服务内存泄漏）。
const LOCK_PRUNE_THRESHOLD: usize = 256;

/// 机会式空闲清扫的最小间隔（避免每次 `ensure` 都遍历工作集表）。
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// 单会话工作集：物化上下文 + 读取进度 + 活动时间。
/// 边界消息推送回调（System 通知 / 压缩点落库后触发；ADR-028「落库即推送」）。
///
/// core 不感知具体通道（ADR-028 原则）：实现由装配层注入（server 侧复用
/// `event_push` 的映射与广播）。未注入 = 不推送（纯库场景 / 单测）。
pub type BoundaryPushCallback = Arc<dyn Fn(&str, &StructuredMessage) + Send + Sync>;

/// 回调槽：注册表与**所有**工作集共享同一实例——支持「先建工作集、后注入
/// 回调」的装配顺序（server 侧 `working_sets` 早于事件通道创建），注入后
/// 对已加载与后续新建的工作集一并生效。
#[derive(Default)]
pub struct BoundaryPushSlot {
    cb: std::sync::RwLock<Option<BoundaryPushCallback>>,
}

impl BoundaryPushSlot {
    /// 取当前回调（未注入返回 `None`）。
    fn get(&self) -> Option<BoundaryPushCallback> {
        self.cb.read().ok().and_then(|g| g.clone())
    }

    /// 注入/替换回调。
    fn set(&self, cb: BoundaryPushCallback) {
        if let Ok(mut g) = self.cb.write() {
            *g = Some(cb);
        }
    }
}

/// 单会话工作集：物化上下文 + 读取进度 + 活动时间。
pub struct SessionWorkingSet {
    session_id: String,
    store: Arc<SessionStore>,
    /// 会话内存态（完整链；`load_and_build_state` 的返回源）。
    state: Arc<RwLock<SessionState>>,
    /// 会话头部镜像（title / working_directory / 前缀快照）——写入收口 ③ 的
    /// 读改写基准（与库同步的唯一副本）。
    header: RwLock<crate::session::SessionHeader>,
    /// 库尾 seq（-1 = 无消息）。与 `state.structured_messages` 同源推进。
    last_seq: AtomicI64,
    /// **段起点 seq**（ADR-035 §2：段 = 最近压缩点 → 现在；无压缩点时为 0）。
    /// 不变式：`段内第 i 条的 seq = start_seq + i`（段 = 完整链的后缀）。
    start_seq: AtomicI64,
    /// 最后一次被 loop 消费到的上界（ADR-035 §4：读取进度的推导值，
    /// 供「轮边界续跑判定」与「唤醒幂等」使用；非独立水位机制）。
    last_consumed_seq: AtomicI64,
    /// 最后活动时间（空闲卸载判定）。
    last_activity: TokioMutex<Instant>,
    /// 边界消息推送回调槽（与注册表共享同一实例，见 [`BoundaryPushSlot`]）。
    boundary_push: Arc<BoundaryPushSlot>,
}

impl SessionWorkingSet {
    /// 创建空工作集（内部使用；加载经 [`WorkingSetRegistry::ensure`]）。
    fn new(
        session_id: &str,
        store: Arc<SessionStore>,
        boundary_push: Arc<BoundaryPushSlot>,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            store,
            state: Arc::new(RwLock::new(SessionState::new(session_id))),
            header: RwLock::new(crate::session::SessionHeader::default()),
            last_seq: AtomicI64::new(-1),
            start_seq: AtomicI64::new(0),
            last_consumed_seq: AtomicI64::new(-1),
            last_activity: TokioMutex::new(Instant::now()),
            boundary_push,
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
        let (header, mut messages) = match loaded {
            Some((header, messages)) => (header, messages),
            None => (crate::session::SessionHeader::default(), Vec::new()),
        };
        let injectable = header.injectable_snapshot.clone().unwrap_or_default();
        let len = messages.len() as i64;
        // ADR-035 §2 段化：只物化「最近压缩点 → 现在」（完整链的后缀）——
        // 压缩点前的历史不驻留内存（需要时经库按需读：回退/导出/前端上滚）。
        let start_seq = messages
            .iter()
            .rposition(|m| m.compression_marker)
            .unwrap_or(0) as i64;
        {
            let mut st = self.state.write().await;
            st.structured_messages = messages.split_off(start_seq as usize); // 全链按 seq 升序 → 索引 == seq
                                                                             // 前缀快照随会话固化（ADR-012/030）：有快照则恢复，无快照留空待首轮填充
            if !injectable.soul.is_empty() || !injectable.rules_and_experiences.is_empty() {
                st.injectable_context = injectable;
            }
            st.last_activity = Instant::now();
        }
        *self.header.write().await = header;
        self.start_seq.store(start_seq, Ordering::SeqCst);
        self.last_seq.store(len - 1, Ordering::SeqCst);
        self.last_consumed_seq.store(len - 1, Ordering::SeqCst);
        Ok(())
    }

    /// **违规检测器**（廉价：一次 `MAX(seq)` 查询）：库尾与缓存不符 → 重建 + 告警。
    ///
    /// 定位：写路径**全部**收口到工作集（§3 四类）后，一致性由「单一写入口」
    /// 保证——本检查不再承担正确性，只是"有写路径绕过工作集"的**架构违规
    /// 探测器**（warn 日志 + 重建兜底）。正常运行时不应触发。
    ///
    /// 已知局限（为何不能当正确性机制）：`MAX(seq)` 是弱信号——`rewrite` 到
    /// **相同长度**、或两个写操作在窗口内相互抵消（delete+redo 复原长度）时
    /// 检测不到；仅 header 变化也不在信号内。故**不得**用它替代写侧收口。
    async fn ensure_fresh(&self) -> Result<()> {
        let db_last = self.store.last_seq(&self.session_id).await?;
        if db_last != self.last_seq() {
            tracing::warn!(
                session = %self.session_id,
                cached = self.last_seq(),
                db = db_last,
                "工作集与库不一致（有写路径绕过工作集？ADR-035 写侧收口违规），已重建兜底"
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
        drop(st);
        // 段化后不能数段内条数（段是完整链的后缀）——库尾 seq 由写收口保证
        // 与段尾同步，直接推进（append 前 last_seq 即库尾，新条 seq = 库尾+1）。
        let seq = self.last_seq() + 1;
        self.last_seq.store(seq, Ordering::SeqCst);
        // ADR-028「落库即推送」（ADR-035 §3 补记）：写侧收口后所有写入经工作集，
        // 边界消息（System 通知 / 压缩点）的推送改挂此处——与旧
        // `BroadcastingSessionManager` 同门控、同映射（core 不感知通道，
        // 回调由装配层注入；见 [`BoundaryPushSlot`]）。
        if msg.role == MessageRole::System || msg.compression_marker {
            if let Some(cb) = self.boundary_push.get() {
                cb(&self.session_id, msg);
            }
        }
        Ok(seq)
    }

    /// 取「某个 seq 之后」的增量条目（轮边界续跑注入，ADR-035 §4）。
    ///
    /// 传入值早于段起点（ctx 落后于一个压缩点）时返回**整段**（下一次组装
    /// 本就以段为口径，不会重复）。
    pub async fn delta_since(&self, seq: i64) -> Vec<(i64, StructuredMessage)> {
        let st = self.state.read().await;
        let start_seq = self.start_seq();
        // 段内索引 = seq + 1 - start_seq（clamp 到 0：seq 早于段起点 → 整段）
        let start_idx = (seq + 1 - start_seq).max(0) as usize;
        if start_idx >= st.structured_messages.len() {
            return Vec::new();
        }
        st.structured_messages[start_idx..]
            .iter()
            .enumerate()
            .map(|(i, m)| ((start_seq + start_idx as i64 + i as i64), m.clone()))
            .collect()
    }

    /// 段内消息条数（组装/压缩判定口径）。
    ///
    /// 注意：段化后**不是**完整链长度——全链长度用 `last_seq() + 1`。
    pub async fn message_count(&self) -> usize {
        self.state.read().await.structured_messages.len()
    }

    /// 段起点 seq（完整链上最近压缩点的位置；无压缩点为 0）。
    pub fn start_seq(&self) -> i64 {
        self.start_seq.load(Ordering::SeqCst)
    }

    /// 写入收口 ②（整表重写，ADR-035 §3）：删消息 / 回退 / 重做 / 编辑。
    ///
    /// 语义：落库全量重写（header 保持不变）→ 段全量替换 → 消费水位重置到
    /// 新库尾（下一次组装必读全量，视为已消费）。
    ///
    /// # Errors
    /// * 库重写失败时返回错误（缓存不动，保持一致）。
    pub async fn rewrite(&self, messages: &[StructuredMessage]) -> Result<()> {
        let header = self.header.read().await.clone();
        self.store
            .rewrite(&self.session_id, &header, messages)
            .await?;
        let len = messages.len() as i64;
        // 段化：与 rebuild 同规则（只保留最后一个压缩点之后；调用方传完整链）
        let start_seq = messages
            .iter()
            .rposition(|m| m.compression_marker)
            .unwrap_or(0) as i64;
        {
            let mut st = self.state.write().await;
            st.structured_messages = messages[start_seq as usize..].to_vec();
            st.last_activity = Instant::now();
        }
        self.start_seq.store(start_seq, Ordering::SeqCst);
        self.last_seq.store(len - 1, Ordering::SeqCst);
        self.last_consumed_seq.store(len - 1, Ordering::SeqCst);
        Ok(())
    }

    /// 压缩后**收缩段**（ADR-035 §2）：段起点前移到最后一个压缩点，
    /// 丢弃其前的历史（压缩已把历史折叠为摘要消息，段内保留摘要及其后）。
    ///
    /// 调用点：`maybe_compress_and_persist` 追加摘要消息之后——压缩是段内
    /// 唯一的收缩时机（否则段随会话无限增长，段化收益归零）。
    pub async fn reshape_after_compression(&self) {
        let mut st = self.state.write().await;
        if let Some(pos) = st
            .structured_messages
            .iter()
            .rposition(|m| m.compression_marker)
        {
            if pos > 0 {
                st.structured_messages.drain(..pos);
                let start = self.start_seq() + pos as i64;
                self.start_seq.store(start, Ordering::SeqCst);
            }
        }
        drop(st);
        // 收缩后段即当前全量口径 → 消费水位推进到库尾（避免"段已含内容却判未消费"）
        self.mark_consumed(self.last_seq());
    }

    /// 写入收口 ③（头部更新，ADR-035 §3）：title / working_directory / 前缀快照。
    ///
    /// 语义：读改镜像 → 落库 → **仅当快照实际变化时**同步内存前缀（避免
    /// 无关字段更新导致前缀白失效、打掉 prompt 缓存）。
    pub async fn update_header(
        &self,
        f: impl FnOnce(&mut crate::session::SessionHeader),
    ) -> Result<()> {
        let mut h = self.header.write().await;
        let old_snapshot = h.injectable_snapshot.clone();
        f(&mut h);
        self.store.update_header(&self.session_id, &h).await?;
        if h.injectable_snapshot != old_snapshot {
            let snapshot = h.injectable_snapshot.clone().unwrap_or_default();
            self.state.write().await.injectable_context = snapshot;
        }
        Ok(())
    }

    /// 写入收口 ④（删除会话，ADR-035 §3）：落库删除（含级联子会话）。
    ///
    /// 调用方须同时从注册表移除本工作集（[`WorkingSetRegistry::remove`]）。
    pub async fn delete(&self) -> Result<()> {
        self.store.delete(&self.session_id).await?;
        Ok(())
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
    /// 边界消息推送回调槽（与所有工作集共享，支持后注入）。
    boundary_push: Arc<BoundaryPushSlot>,
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
            boundary_push: Arc::new(BoundaryPushSlot::default()),
        })
    }

    /// 是否已装配会话存储（工作集可用）。
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// 注入边界消息推送回调（System 通知 / 压缩点在 `append` 落库后触发）。
    ///
    /// 支持**后注入**（装配顺序：注册表先于事件通道创建）：共享
    /// [`BoundaryPushSlot`] → 对已加载与后续新建的工作集一并生效。
    pub fn set_boundary_push(&self, cb: BoundaryPushCallback) {
        self.boundary_push.set(cb);
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
                let ws = Arc::new(SessionWorkingSet::new(
                    session_id,
                    store,
                    self.boundary_push.clone(),
                ));
                ws.rebuild().await?;
                // 竞态收口：检查—重建—插入之间无锁（rebuild 是 await 点），并发
                // ensure 可能各建一个实例 → 同一会话两份工作集（缓存/写收口分叉）。
                // 以**先入者**为准：后到者丢弃自建实例，返回注册表中的那一个。
                let mut sets = self.sets.lock().await;
                let ws = sets.entry(session_id.to_string()).or_insert(ws).clone();
                drop(sets);
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
                // 使用中判定双保险：① 会话锁被持有（有 loop 在跑）；
                // ② 工作集实例被外部引用（有人 get 到 Arc 但未持锁）——
                // 卸载会使其后续写入绕过注册表（缓存与库分叉）
                let lock_held = locks
                    .get(sid)
                    .map(|l| Arc::strong_count(l) > 1)
                    .unwrap_or(false);
                let ws_shared = Arc::strong_count(ws) > 1;
                if !lock_held && !ws_shared && ws.is_idle(ttl).await {
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

    /// ADR-028「落库即推送」（ADR-035 §3 补记）：工作集 `append` 对**边界消息**
    /// （System 通知 / 压缩点）触发推送回调，普通消息不触发；**后注入**对已加载
    /// 与后续新建的工作集同样生效（共享回调槽——装配顺序：注册表先于事件通道）。
    #[tokio::test]
    async fn test_append_pushes_boundary_messages_only() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        create_session(&store, "s2").await;
        let reg = WorkingSetRegistry::new(Some(store));
        let ws = reg.ensure("s1").await.unwrap();
        let hits = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let hits = hits.clone();
            reg.set_boundary_push(Arc::new(move |_sid: &str, msg: &StructuredMessage| {
                let kind = if msg.compression_marker {
                    "marker"
                } else {
                    "system"
                };
                hits.lock().unwrap().push(kind.to_string());
            }));
        }
        ws.append(&user_msg("s1", "hi"), true).await.unwrap();
        assert!(hits.lock().unwrap().is_empty(), "普通消息不触发边界推送");
        ws.append(&StructuredMessage::system("s1", "[通知] 完成"), true)
            .await
            .unwrap();
        let mut marker = user_msg("s1", "摘要");
        marker.compression_marker = true;
        ws.append(&marker, true).await.unwrap();
        let ws2 = reg.ensure("s2").await.unwrap();
        ws2.append(&StructuredMessage::system("s2", "[通知] s2"), true)
            .await
            .unwrap();
        let seen = hits.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec!["system", "marker", "system"],
            "边界消息各推送一次（含后注入生效）: {seen:?}"
        );
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

    /// 写入收口 ②：rewrite 后缓存与库一致（缓存不残留旧消息）。
    #[tokio::test]
    async fn test_rewrite_keeps_cache_consistent() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        for i in 0..5 {
            store
                .append_message("s1", &user_msg("s1", &format!("m{i}")))
                .await
                .unwrap();
        }
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 4);

        // 截断到 2 条（删消息 / 回退语义）
        let kept: Vec<StructuredMessage> = vec![user_msg("s1", "m0"), user_msg("s1", "m1")];
        ws.rewrite(&kept).await.unwrap();
        assert_eq!(ws.last_seq(), 1, "重写后库尾 seq 前移");
        assert_eq!(ws.message_count().await, 2, "缓存与库一致（无残留）");
        assert!(
            !ws.has_unconsumed(),
            "重写后消费水位重置到新库尾（下一次组装必读全量）"
        );

        // 库侧核对（不是只看缓存）
        let (_, stored) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(stored.len(), 2);
    }

    /// 写入收口 ③：仅当快照实际变化时才同步内存前缀（无关字段更新不白失效）。
    #[tokio::test]
    async fn test_update_header_syncs_snapshot_only_on_change() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        // 预置：库 header 固化前缀（None → Some = 变化 → 同步内存）
        ws.update_header(|h| {
            h.injectable_snapshot = Some(crate::common::types::InjectableContext {
                soul: "SOUL".to_string(),
                ..Default::default()
            })
        })
        .await
        .unwrap();
        assert_eq!(
            ws.state.read().await.injectable_context.soul,
            "SOUL",
            "快照写入须同步内存镜像"
        );

        // 仅改 title：内存前缀不动（不白失效）
        ws.update_header(|h| h.title = Some("新标题".to_string()))
            .await
            .unwrap();
        assert_eq!(
            ws.state.read().await.injectable_context.soul,
            "SOUL",
            "无关字段更新不得清空前缀"
        );

        // 置空快照（压缩点失效语义）：内存前缀同步清空
        ws.update_header(|h| h.injectable_snapshot = None)
            .await
            .unwrap();
        assert!(
            ws.state.read().await.injectable_context.soul.is_empty(),
            "快照清空须同步内存镜像"
        );

        // 库侧核对
        let header = store.header("s1").await.unwrap().unwrap();
        assert_eq!(header.title.as_deref(), Some("新标题"));
    }

    /// 写入收口 ④：delete 落库（注册表移除由调用方；此处核对库侧已删）。
    #[tokio::test]
    async fn test_delete_removes_from_store() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        store
            .append_message("s1", &user_msg("s1", "m"))
            .await
            .unwrap();
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        ws.delete().await.unwrap();
        assert!(!store.exists("s1").await.unwrap(), "库侧已删除");
        reg.remove("s1").await;
        assert!(reg.get("s1").await.is_none(), "注册表已移除");
    }

    /// ADR-035 §2 段化：重建只物化「最近压缩点 → 现在」（完整链的后缀）。
    #[tokio::test]
    async fn test_rebuild_materializes_segment_since_last_marker() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        store
            .append_message("s1", &user_msg("s1", "旧历史0"))
            .await
            .unwrap();
        store
            .append_message("s1", &user_msg("s1", "旧历史1"))
            .await
            .unwrap();
        // 压缩点（seq=2）
        let mut marker = StructuredMessage::system("s1", "[对话摘要]……");
        marker.compression_marker = true;
        store.append_message("s1", &marker).await.unwrap();
        store
            .append_message("s1", &user_msg("s1", "压缩后消息"))
            .await
            .unwrap();

        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.start_seq(), 2, "段起点 = 最后一个压缩点");
        assert_eq!(
            ws.message_count().await,
            2,
            "段内 = 压缩点 + 其后（不含更早历史）"
        );
        assert_eq!(ws.last_seq(), 3, "库尾不变（完整链坐标）");

        // 段内首条是压缩点（组装/压缩判定依赖）
        let st = ws.state();
        let guard = st.read().await;
        assert!(guard.structured_messages[0].compression_marker);
        drop(guard);
    }

    /// 段化后 append 的 seq 仍正确（不能数段内条数——那是后缀长度）。
    #[tokio::test]
    async fn test_append_seq_after_segmentation() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        store
            .append_message("s1", &user_msg("s1", "旧"))
            .await
            .unwrap();
        let mut marker = StructuredMessage::system("s1", "[对话摘要]……");
        marker.compression_marker = true;
        store.append_message("s1", &marker).await.unwrap();
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.last_seq(), 1);

        let seq = ws.append(&user_msg("s1", "新消息"), true).await.unwrap();
        assert_eq!(seq, 2, "append 的 seq = 库尾 + 1（不是段内条数-1）");
        // 库侧核对
        let (_, stored) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(stored.len(), 3);
    }

    /// delta_since 的段化边界：ctx 早于段起点 → 整段；段内 → 正确切片。
    #[tokio::test]
    async fn test_delta_since_segment_boundaries() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        for i in 0..3 {
            store
                .append_message("s1", &user_msg("s1", &format!("m{i}")))
                .await
                .unwrap();
        }
        let mut marker = StructuredMessage::system("s1", "[对话摘要]……");
        marker.compression_marker = true;
        store.append_message("s1", &marker).await.unwrap();
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.start_seq(), 3);

        // ctx 在段起点之前（落后一个压缩点）→ 整段（2 条：marker + 无后续）
        let all = ws.delta_since(0).await;
        assert_eq!(all.len(), 1, "段内只有压缩点一条");
        assert_eq!(all[0].0, 3, "返回值带真实链上 seq");

        // 追加两条后：delta(3) → 只取其后
        ws.append(&user_msg("s1", "a"), true).await.unwrap();
        ws.append(&user_msg("s1", "b"), true).await.unwrap();
        let delta = ws.delta_since(3).await;
        assert_eq!(delta.len(), 2);
        assert_eq!((delta[0].0, delta[1].0), (4, 5));
        // 无新消息
        assert!(ws.delta_since(5).await.is_empty());
    }

    /// 压缩后收缩段：段起点前移到新压缩点，段内丢弃其前历史。
    #[tokio::test]
    async fn test_reshape_after_compression_shrinks_segment() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        for i in 0..4 {
            store
                .append_message("s1", &user_msg("s1", &format!("m{i}")))
                .await
                .unwrap();
        }
        let reg = WorkingSetRegistry::new(Some(store.clone()));
        let ws = reg.ensure("s1").await.unwrap();
        assert_eq!(ws.start_seq(), 0);
        assert_eq!(ws.message_count().await, 4);

        // 模拟压缩：摘要消息落库（seq=4）→ 收缩
        let mut marker = StructuredMessage::system("s1", "[对话摘要]……");
        marker.compression_marker = true;
        ws.append(&marker, true).await.unwrap();
        ws.reshape_after_compression().await;

        assert_eq!(ws.start_seq(), 4, "段起点前移到新压缩点");
        assert_eq!(ws.message_count().await, 1, "段内只剩压缩点");
        assert_eq!(ws.last_seq(), 4, "库尾不变");
        assert!(!ws.has_unconsumed(), "收缩后消费水位推进到库尾");
    }

    /// P1-32 回归：并发 ensure 只注册一个实例（以先入者为准，防双实例分叉）。
    #[tokio::test]
    async fn test_concurrent_ensure_yields_single_instance() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store));

        let (a, b) = tokio::join!(reg.ensure("s1"), reg.ensure("s1"));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(
            Arc::ptr_eq(&a, &b),
            "并发 ensure 必须返回同一实例（不得双实例）"
        );
        assert_eq!(reg.loaded_count().await, 1);
    }

    /// P1-32 回归：外部持有工作集引用（未持锁）时不得被空闲卸载。
    #[tokio::test]
    async fn test_sweep_keeps_externally_referenced_set() {
        let store = test_store().await;
        create_session(&store, "s1").await;
        let reg = WorkingSetRegistry::new(Some(store));
        let ws = reg.ensure("s1").await.unwrap();

        let unloaded = reg.sweep_idle(Duration::from_secs(0)).await;
        assert_eq!(unloaded, 0, "被外部引用的工作集不得卸载");
        assert!(reg.get("s1").await.is_some());
        drop(ws);

        // 释放引用且空闲（TTL=0）→ 卸载
        let unloaded = reg.sweep_idle(Duration::from_secs(0)).await;
        assert_eq!(unloaded, 1, "无引用且空闲应卸载");
    }
}
