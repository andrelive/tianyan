//! 统一任务调度器（间隔 + 补跑模型，ADR-024）。
//!
//! 个人 PC 场景下服务不常驻：cron「在某时刻执行」假设进程一直活着——
//! 宕机错过即永不执行，且 cron 的 UTC 语义会让本地触发时刻漂移。
//! 因此采用**间隔制**：每个任务声明执行间隔；单一扫描循环每 tick
//! 顺序检查全部任务，「距上次执行 ≥ 间隔」即立即执行。
//!
//! - **宕机补跑**：last_run 持久化到「{data_dir}/scheduler_state.json」，
//!   重启后过期的任务在第一个扫描周期补跑一次（单次补跑，不回放错过的次数）；
//! - **串行执行**：同轮扫描内到期的任务逐个顺序执行，天然避免多个维护
//!   任务（LLM 调用 / LanceDB / SQLite）并发争抢；
//! - **tick 粒度**：默认 60 秒；扫描本身只是内存比对，成本可忽略。
//!   任务实际触发最多延迟一个 tick——对维护类任务可接受。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::schedule::{decide_fire, FireDecision, MissedPolicy, Schedule};
use crate::common::error::TianyanError;
use crate::config::TianyanConfig;
use crate::scheduler::TaskStateStore;
use crate::session::search::SessionRecall;
use crate::vfs::{SummaryService, VirtualFileSystem};

/// 任务上下文。
///
/// 提供给任务执行时访问核心服务。
#[derive(Clone)]
pub struct TaskContext {
    /// 虚拟文件系统。
    pub vfs: Arc<dyn VirtualFileSystem>,
    /// 摘要服务（测试可注入 MockSummaryEngine）。
    pub summary_engine: Arc<dyn SummaryService>,
    /// 会话回忆服务（演化任务：近期会话增量采集；None 时增量材料不可用）。
    pub session_recall: Option<Arc<SessionRecall>>,
    /// 配置。
    pub config: Arc<TianyanConfig>,
    /// 任务作用域状态存储（G5：定时任务跨运行状态——读写自己的持久状态）。
    pub task_state: Arc<TaskStateStore>,
}

impl TaskContext {
    /// 创建新的任务上下文。
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        summary_engine: Arc<dyn SummaryService>,
        session_recall: Option<Arc<SessionRecall>>,
        config: Arc<TianyanConfig>,
    ) -> Self {
        let task_state = Arc::new(TaskStateStore::new(vfs.clone()));
        Self {
            vfs,
            summary_engine,
            session_recall,
            config,
            task_state,
        }
    }
}

/// 任务优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
pub enum TaskPriority {
    /// 低优先级。
    Low = 1,
    /// 普通优先级。
    #[default]
    Normal = 2,
    /// 高优先级。
    High = 3,
    /// 关键优先级。
    Critical = 4,
}

/// 任务执行结果。
#[derive(Debug)]
pub struct TaskResult {
    /// 是否成功。
    pub success: bool,
    /// 处理的条目数量。
    pub processed_count: usize,
    /// 错误信息（如果有）。
    pub error: Option<TianyanError>,
    /// 业务级「跳过」原因（如「今天不是交易日」）。
    ///
    /// 与**调度级**跳过（`MissedPolicy` 判定错过窗口、任务根本没执行）不是一回事：
    /// 这里是任务跑了、但自己判断「现在不该做事」。单独成字段是为了不把正常早退
    /// 记成失败（否则洞察页失败率一片红，而用户其实没做错任何事）。
    pub skipped: Option<String>,
}

impl TaskResult {
    /// 创建成功结果。
    pub fn success(processed_count: usize) -> Self {
        Self {
            success: true,
            processed_count,
            error: None,
            skipped: None,
        }
    }

    /// 创建失败结果。
    pub fn failed(error: impl Into<TianyanError>) -> Self {
        Self {
            success: false,
            processed_count: 0,
            error: Some(error.into()),
            skipped: None,
        }
    }

    /// 创建「业务级跳过」结果：任务正常结束，只是判断现在不该做事。
    ///
    /// `success = true` —— 跳过不是失败（调度器统计、洞察页失败率都不该被它污染）。
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            success: true,
            processed_count: 0,
            error: None,
            skipped: Some(reason.into()),
        }
    }
}

/// 任务处理 trait。
///
/// 所有后台任务必须实现此 trait。
#[async_trait::async_trait]
pub trait TaskHandler: Send + Sync {
    /// 执行任务。
    async fn execute(&self, ctx: &TaskContext) -> TaskResult;

    /// 获取任务名称。
    fn name(&self) -> &str;
}

/// 任务定义。
pub struct TaskDefinition {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 任务优先级。
    pub priority: TaskPriority,
    /// **何时该跑**（间隔 / 每天 / 每周 / 一次性）。
    pub schedule: Schedule,
    /// **错过了怎么办**（立即补跑 / 跳过 / 窗口内补跑）。
    pub missed: MissedPolicy,
    /// 等价的间隔秒数（`schedule` 的镜像；落盘与旧接口/前端兼容用）。
    ///
    /// 由 [`TaskDefinition::with_schedule`] / [`TaskDefinition::new`] 保持同步，
    /// 不要在构造后单独改它——判定一律读 `schedule`。
    pub interval_secs: u64,
    /// 任务处理器。
    pub handler: Arc<dyn TaskHandler>,
}

impl TaskDefinition {
    /// 创建**间隔制**任务定义（旧语义：`Every { secs }` + 错过就补跑）。
    ///
    /// 保留这个签名是为了让既有调用点（内置 5 任务 / 演化任务）零改动迁移。
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        interval_secs: u64,
        handler: Arc<dyn TaskHandler>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            priority: TaskPriority::Normal,
            schedule: Schedule::Every {
                secs: interval_secs.max(1),
            },
            missed: MissedPolicy::RunOnce,
            interval_secs,
            handler,
        }
    }

    /// 换成其它调度规格（`Daily { at: "15:30" }` / `Weekly` / `Once`）。
    ///
    /// 同时把 `interval_secs` 更新为等价间隔——旧接口与 `scheduler_state.json`
    /// 的展示字段仍读它。
    pub fn with_schedule(mut self, schedule: Schedule, missed: MissedPolicy) -> Self {
        self.interval_secs = schedule.equivalent_interval_secs();
        self.schedule = schedule;
        self.missed = missed;
        self
    }
}

/// 已注册任务的信息（注册顺序 = 扫描顺序）。
struct RegisteredTask {
    definition: TaskDefinition,
    /// 上次执行完成时刻（epoch 秒；None = 从未执行，首个扫描周期即执行）。
    last_run: Option<i64>,
    /// **调度游标**：最近处理过的调度时刻点（跑过或判定跳过都推进）。
    ///
    /// 与 `last_run` 严格分开——`last_run` 是「上次真的执行完的时间」，这里是
    /// 「哪个时刻点已经结算过了」。混用会导致：服务每晚关机、白天开机时，反复重判
    /// 同一个已跳过的时刻点（补跑风暴）；或反过来永远不补。
    last_fire: Option<i64>,
    /// 执行次数。
    run_count: u64,
    /// 最近一次执行的失败信息（None = 成功或从未执行）。
    last_error: Option<String>,
    /// 最近一次「跳过」的原因（调度级错过窗口 / 业务级主动跳过）。
    last_skipped: Option<String>,
}

/// 任务运行状态快照（供状态查询接口使用）。
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatus {
    /// 任务唯一 ID。
    pub id: String,
    /// 任务显示名称。
    pub name: String,
    /// 任务优先级。
    pub priority: TaskPriority,
    /// 等价间隔（秒；日历类任务取其近似值，供旧调用方展示）。
    pub interval_secs: u64,
    /// 调度规格描述（如「每天 15:30」「每 1800 秒」）。
    pub schedule: String,
    /// 错过策略描述。
    pub missed: String,
    /// 执行次数。
    pub run_count: u64,
    /// 距上次执行已过秒数（从未执行时为 None）。
    pub last_run_ago_secs: Option<u64>,
    /// 距下次到期秒数（已到期 = 0；从未执行 = 0，首个扫描周期即执行）。
    ///
    /// 日历类任务按**时刻点**算（`next_occurrence_after`），不是 `last_run + 间隔`
    /// —— 后者正是旧模型「重启就补跑」的展示侧同款错误。
    pub next_due_in_secs: u64,
    /// 距上次「结算时刻点」已过秒数（None = 从未结算）。
    pub last_fire_ago_secs: Option<u64>,
    /// 最近一次执行的失败信息（None = 成功或从未执行）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 最近一次跳过原因（调度级错过窗口 / 业务级主动跳过）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_skipped: Option<String>,
}

/// 一次扫描里判定为「需要处理」的任务（**含判定为跳过的**——跳过也要结算游标，
/// 否则同一个错过的时刻点会被反复重判）。
struct DueTask {
    /// 任务 ID。
    id: String,
    /// 时刻点（epoch 秒）。
    anchor: i64,
    /// 该跑还是跳过。
    decision: FireDecision,
    /// 是否为**绝对**时刻点（`Every` 是相对语义，游标由 `last_run` 承担）。
    absolute: bool,
}

/// 持久化状态文件的单任务条目。
#[derive(Debug, Serialize, Deserialize)]
struct PersistedTaskState {
    last_run: i64,
    /// 调度游标（旧状态文件无此字段 → 缺省 None，按「从未结算」处理）。
    #[serde(default)]
    last_fire: Option<i64>,
    run_count: u64,
    /// 最近一次失败信息（旧状态文件无此字段，缺省 None）。
    #[serde(default)]
    last_error: Option<String>,
    /// 最近一次跳过原因（旧状态文件无此字段）。
    #[serde(default)]
    last_skipped: Option<String>,
}

/// 扫描间隔默认值（秒）。
pub const DEFAULT_TICK_SECS: u64 = 60;

/// 单任务执行超时（秒）——串行扫描下防止单任务 hang 拖死全部定时任务。
///
/// 取值依据：最重的任务是演化综述（一次 LLM 轮 + 工具调用，实测数分钟）；
/// 30 分钟是「远大于正常、远小于永久挂死」的兜底。
pub const TASK_TIMEOUT_SECS: u64 = 1800;

/// 统一任务调度器：单扫描循环 + 顺序执行（ADR-024）。
pub struct TaskScheduler {
    /// 已注册任务（注册顺序 = 扫描顺序；量级 <10，Vec 足够）。
    tasks: RwLock<Vec<RegisteredTask>>,
    /// 运行状态。
    running: std::sync::atomic::AtomicBool,
    /// 单一扫描循环句柄。
    handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
    /// last_run 持久化路径（None = 不持久化，如测试）。
    state_path: Option<PathBuf>,
    /// 扫描间隔（秒）。
    tick_secs: u64,
    /// 单任务执行超时（秒；默认 [`TASK_TIMEOUT_SECS`]，测试/运维可覆盖）。
    task_timeout_secs: u64,
    /// 当前正在执行的任务 ID（顺序执行，至多一个；None = 空闲）。
    executing: RwLock<Option<String>>,
}

/// 计算下次到期时刻（epoch 秒）：last_run + interval；从未执行 = 立即到期。
pub fn next_due_at(last_run: Option<i64>, interval_secs: u64, now: i64) -> i64 {
    let due = last_run.map(|t| t + interval_secs as i64).unwrap_or(now);
    due.max(now)
}

/// 判定一个任务此刻该不该处理（**纯函数**：时间可注入，便于单测）。
///
/// 两条语义：
/// - [`Schedule::Every`]：相对语义（锚点 = `last_run + secs`），`missed` 不参与
///   ——「间隔到了就跑」本来就是它的定义；
/// - 日历类：锚点 = 最近一个 `≤ now` 的时刻点，先被调度游标 `last_fire` 去重，
///   再按 [`MissedPolicy`] 判「补跑 / 跳过」。
///
/// 返回 [`FireDecision::NotDue`] 表示「没有待处理的时刻点」（未到 / 已结算）。
pub fn plan_one(
    schedule: &Schedule,
    missed: MissedPolicy,
    last_run: Option<i64>,
    last_fire: Option<i64>,
    now: i64,
    tick_secs: u64,
) -> FireDecision {
    match schedule {
        Schedule::Every { secs } => match last_run.map(|lr| lr + *secs as i64) {
            Some(due) if due > now => FireDecision::NotDue,
            _ => FireDecision::Fire { delay: 0 },
        },
        calendar => {
            let Some(anchor) = calendar.most_recent_occurrence(now) else {
                return FireDecision::NotDue;
            };
            if last_fire.is_some_and(|lf| anchor <= lf) {
                return FireDecision::NotDue; // 该时刻点已结算
            }
            let delay = (now - anchor).max(0) as u64;
            decide_fire(missed, delay, tick_secs)
        }
    }
}

/// 旧 cron 表达式 → 间隔秒数（一次性迁移辅助；仅覆盖本项目文档化的模式）。
///
/// 支持模式（6 字段 = 秒 分 时 日 月 周）：
/// - 「*/N * * * * *」= 每 N 秒
/// - 「0 */N * * * *」= 每 N 分钟
/// - 「0 M H * * *」（M/H 为数字）= 每天 H:M → 24 小时间隔（时间点随
///   开机/宕机漂移，间隔制的固有语义）。
/// - 无法识别返回 None（调用方回退默认间隔并告警）。
pub fn interval_from_cron(cron: &str) -> Option<u64> {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() != 6 {
        return None;
    }
    let (sec, min, hour) = (parts[0], parts[1], parts[2]);
    // */N 秒
    if let Some(n) = sec.strip_prefix("*/") {
        if min == "*" && hour == "*" {
            return n.parse::<u64>().ok().filter(|n| *n > 0);
        }
    }
    if sec == "0" {
        // 0 */N 分钟
        if let Some(n) = min.strip_prefix("*/") {
            if hour == "*" {
                return n.parse::<u64>().ok().filter(|n| *n > 0).map(|n| n * 60);
            }
        }
        // 0 M H * * * = 每天 H:M → 24h
        if min.parse::<u64>().is_ok()
            && hour.parse::<u64>().is_ok()
            && parts[3] == "*"
            && parts[4] == "*"
            && parts[5] == "*"
        {
            return Some(24 * 3600);
        }
    }
    None
}

impl TaskScheduler {
    /// 创建新的任务调度器（tick 60s，不持久化）。
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
            running: std::sync::atomic::AtomicBool::new(false),
            handle: RwLock::new(None),
            state_path: None,
            tick_secs: DEFAULT_TICK_SECS,
            task_timeout_secs: TASK_TIMEOUT_SECS,
            executing: RwLock::new(None),
        }
    }

    /// 设置扫描间隔（秒）。
    pub fn with_tick_secs(mut self, tick_secs: u64) -> Self {
        self.tick_secs = tick_secs.max(1);
        self
    }

    /// 设置单任务执行超时（秒）——默认 [`TASK_TIMEOUT_SECS`]。
    pub fn with_task_timeout_secs(mut self, secs: u64) -> Self {
        self.task_timeout_secs = secs.max(1);
        self
    }

    /// 设置 last_run 持久化路径（启动时加载，运行中落盘——跨重启补跑）。
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
    }

    /// 从磁盘加载持久化的 last_run / run_count / last_error（按任务 id 合并）。
    async fn load_state(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let Ok(content) = tokio::fs::read_to_string(path).await else {
            return; // 文件缺失 = 首次运行
        };
        let Ok(map) = serde_json::from_str::<HashMap<String, PersistedTaskState>>(&content) else {
            // 损坏证据另存（.corrupt）：保留诊断样本，避免被下次成功写覆盖
            let corrupt = path.with_extension("json.corrupt");
            let _ = tokio::fs::rename(path, &corrupt).await;
            tracing::warn!(
                path = %path.display(),
                corrupt = %corrupt.display(),
                "调度器状态文件损坏（已另存 .corrupt；任务按从未执行处理 → 下轮补跑）"
            );
            return;
        };
        let mut tasks = self.tasks.write().await;
        for t in tasks.iter_mut() {
            if let Some(s) = map.get(&t.definition.id) {
                t.last_run = Some(s.last_run);
                t.last_fire = s.last_fire;
                t.run_count = s.run_count;
                t.last_error = s.last_error.clone();
                t.last_skipped = s.last_skipped.clone();
            }
        }
        tracing::info!(count = map.len(), "已加载调度器持久化状态");
    }

    /// 持久化 last_run / run_count（写失败仅告警——下次运行重写）。
    async fn persist_state(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let map: HashMap<String, PersistedTaskState> = {
            let tasks = self.tasks.read().await;
            tasks
                .iter()
                .filter_map(|t| {
                    // 只要「执行过」或「结算过时刻点」就落盘（跳过也要记，否则重启后
                    // 会重判同一个已跳过的时刻点）
                    let lr = t.last_run.or(t.last_fire)?;
                    Some((
                        t.definition.id.clone(),
                        PersistedTaskState {
                            last_run: lr,
                            last_fire: t.last_fire,
                            run_count: t.run_count,
                            last_error: t.last_error.clone(),
                            last_skipped: t.last_skipped.clone(),
                        },
                    ))
                })
                .collect()
        };
        Self::write_state_file(path, &map).await;
    }

    /// 持久化（**merge 语义**）：读现有条目 → 用当前任务的 last_run 覆盖 → 写回。
    ///
    /// 仅**注册路径**使用：注册发生在 `load_state` 之后，此刻 tasks 可能只含刚
    /// 注册的少数任务——全量覆盖会把其它任务的持久化状态清空（实测：状态文件
    /// 被写成 `{}` → 下次 `load_state` 丢失 run_count / last_run，补跑语义失效）。
    async fn persist_state_merge(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let mut map: HashMap<String, PersistedTaskState> =
            match tokio::fs::read_to_string(path).await {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => HashMap::new(),
            };
        {
            let tasks = self.tasks.read().await;
            for t in tasks.iter() {
                if let Some(lr) = t.last_run {
                    map.insert(
                        t.definition.id.clone(),
                        PersistedTaskState {
                            last_run: lr,
                            last_fire: t.last_fire,
                            run_count: t.run_count,
                            last_error: t.last_error.clone(),
                            last_skipped: t.last_skipped.clone(),
                        },
                    );
                } else if let Some(lf) = t.last_fire {
                    // 只有游标、还没执行过（例如全是跳过）：也要落盘
                    map.insert(
                        t.definition.id.clone(),
                        PersistedTaskState {
                            last_run: lf,
                            last_fire: Some(lf),
                            run_count: t.run_count,
                            last_error: t.last_error.clone(),
                            last_skipped: t.last_skipped.clone(),
                        },
                    );
                }
            }
        }
        Self::write_state_file(path, &map).await;
    }

    /// 状态文件原子写（temp + rename）。
    async fn write_state_file(path: &std::path::Path, map: &HashMap<String, PersistedTaskState>) {
        match serde_json::to_string(&map) {
            Ok(json) => {
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                // 原子写（temp + rename）：半写状态文件（断电/崩溃）会被 load_state
                // 判为损坏 → 全部任务视为从未执行 → 同时补跑；rename 到同目录为
                // 原子替换（Windows 亦为 MoveFileEx 替换语义）
                let tmp = path.with_extension("json.tmp");
                if let Err(e) = tokio::fs::write(&tmp, &json).await {
                    tracing::error!(error = %e, path = %tmp.display(), "调度器状态持久化失败（临时文件）");
                    return;
                }
                if let Err(e) = tokio::fs::rename(&tmp, path).await {
                    tracing::error!(error = %e, path = %path.display(), "调度器状态原子替换失败");
                }
            }
            Err(e) => tracing::error!(error = %e, "调度器状态序列化失败"),
        }
    }

    /// 本轮扫描要处理的任务（按注册顺序；顺序扫描 = 串行执行）。
    ///
    /// 判定分两条路：
    /// - [`Schedule::Every`]：**相对**语义，锚点 = `last_run + secs`（与旧模型等价，
    ///   从未执行 = 立即）；
    /// - 日历类（`Daily` / `Weekly` / `Once`）：**绝对**时刻点，锚点 = 最近一个
    ///   `≤ now` 的时刻点，再用调度游标 `last_fire` 去重，最后由 [`decide_fire`]
    ///   按 `MissedPolicy` 判「补跑 / 跳过」。
    ///
    /// 跳过的任务也会出现在结果里（`Skip`）——调用方必须结算游标，否则同一个错过的
    /// 时刻点会被反复重判。
    async fn plan_due(&self) -> Vec<DueTask> {
        let now = Utc::now().timestamp();
        let tick = self.tick_secs;
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter_map(|t| {
                // 判定单点：`plan_one`（纯函数，单测覆盖收盘场景）
                let decision = plan_one(
                    &t.definition.schedule,
                    t.definition.missed,
                    t.last_run,
                    t.last_fire,
                    now,
                    tick,
                );
                if decision == FireDecision::NotDue {
                    return None;
                }
                let (anchor, absolute) = match &t.definition.schedule {
                    Schedule::Every { secs } => {
                        (t.last_run.map(|lr| lr + *secs as i64).unwrap_or(now), false)
                    }
                    calendar => (calendar.most_recent_occurrence(now).unwrap_or(now), true),
                };
                Some(DueTask {
                    id: t.definition.id.clone(),
                    anchor,
                    decision,
                    absolute,
                })
            })
            .collect()
    }

    /// 结算调度游标（绝对时刻点专用）：把「该时刻点已处理」落下来。
    ///
    /// **必须在执行之前调用**：任务执行期间进程被重启时，下次启动不该重跑同一个
    /// 时刻点（这正是旧模型「重启就立刻补跑」的另一面）。
    async fn settle_fire(&self, task_id: &str, anchor: i64) {
        let mut tasks = self.tasks.write().await;
        if let Some(t) = tasks.iter_mut().find(|t| t.definition.id == task_id) {
            t.last_fire = Some(anchor);
        }
    }

    /// 记录一次**调度级**跳过（任务未执行；原因写进状态，用户看得见）。
    async fn record_skip(&self, task_id: &str, reason: String) {
        let mut tasks = self.tasks.write().await;
        if let Some(t) = tasks.iter_mut().find(|t| t.definition.id == task_id) {
            t.last_skipped = Some(truncate_error(&reason));
        }
    }

    /// 单一扫描循环：顺序检查 → 到期即执行 / 判跳过 → 持久化 → 睡到下个 tick。
    async fn run_scanner(scheduler: Arc<TaskScheduler>, ctx: Arc<TaskContext>) {
        loop {
            if !scheduler.is_running() {
                break;
            }
            for due in scheduler.plan_due().await {
                if !scheduler.is_running() {
                    break;
                }
                // **先结算游标、后执行**（绝对时刻点）：任务跑起来就算「处理过」，
                // 执行期间进程被重启也不会重跑同一个时刻点。
                if due.absolute {
                    scheduler.settle_fire(&due.id, due.anchor).await;
                }
                match due.decision {
                    FireDecision::Fire { delay } => {
                        if due.absolute && delay > scheduler.tick_secs {
                            tracing::info!(
                                task = %due.id,
                                delay,
                                "定时任务补跑（已偏离时刻点，按错过策略允许）"
                            );
                        }
                        // 串行执行：一个任务跑完才检查下一个（避免资源争抢）
                        scheduler.execute_task(&due.id, &ctx).await;
                    }
                    FireDecision::Skip { delay } => {
                        tracing::info!(
                            task = %due.id,
                            delay,
                            "定时任务错过窗口，跳过本次（游标已推进，不再重判）"
                        );
                        scheduler
                            .record_skip(&due.id, format!("错过窗口（延迟 {delay} 秒）"))
                            .await;
                    }
                    FireDecision::NotDue => {}
                }
                // 处理完**立即**持久化：任务本体可能长时间运行（用户任务是分钟级
                // 管线），若只在 tick 末尾写，进程在运行中被重启会丢失本次记录，
                // 下次启动重复补跑。
                scheduler.persist_state().await;
            }
            scheduler.persist_state().await;
            tokio::time::sleep(tokio::time::Duration::from_secs(scheduler.tick_secs)).await;
        }
    }

    /// 启动调度器（加载持久化状态 → 起单一扫描循环）。
    pub async fn start_with_scheduler(
        scheduler: Arc<Self>,
        ctx: Arc<TaskContext>,
    ) -> crate::common::error::Result<()> {
        if scheduler
            .running
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            tracing::warn!("任务调度器已在运行");
            return Ok(());
        }

        tracing::info!(tick_secs = scheduler.tick_secs, "任务调度器启动中...");
        scheduler.load_state().await;

        let handle = tokio::spawn(Self::run_scanner(scheduler.clone(), ctx));
        *scheduler.handle.write().await = Some(handle);

        tracing::info!("任务调度器已启动");
        Ok(())
    }

    /// 停止所有任务并释放资源（数据目录搬迁依赖关停后文件锁释放）。
    pub async fn shutdown(&self) -> crate::common::error::Result<()> {
        self.stop().await?;
        self.persist_state().await;

        // 清空任务定义：释放 handler（ScheduledAgentTaskHandler 持有
        // manager，manager 持有 scheduler——不清理则形成循环引用）。
        self.tasks.write().await.clear();

        // 取消扫描循环并等待完全结束（Arc 链释放 → SQLite 文件锁释放）。
        let handle = self.handle.write().await.take();
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }

        tracing::info!("任务调度器已关闭");
        Ok(())
    }

    /// 注册任务（id 重复报错；last_run 不重置——同 id 重注册延续补跑语义）。
    pub async fn register_task(
        &self,
        definition: TaskDefinition,
    ) -> crate::common::error::Result<()> {
        self.register_task_with_last_run(definition, None).await
    }

    /// 注册任务并指定初始 last_run（**注册晚于调度器启动**的任务专用）。
    ///
    /// 背景（实测 bug）：`start_with_scheduler` 内部 `load_state()` 只按
    /// **当时已注册**的任务回填 last_run。定时智能体任务由 server 在调度器
    /// 启动**之后**才注册（`load_and_register`），因此拿不到持久化的
    /// last_run → `None` → `due_task_ids` 判定"从未执行 = 立即到期" →
    /// **每次重启/重装都立刻补跑一次**（用户可见"打开一次就启动一次"）。
    ///
    /// 调用方应传入任务自身持久化的上次运行时间（`last_run_at`）。注册后
    /// **立即 persist_state**，使该条目写入 `scheduler_state.json`——
    /// 即便后续启动顺序不变，也能在下次 `load_state` 时恢复。
    pub async fn register_task_with_last_run(
        &self,
        definition: TaskDefinition,
        initial_last_run: Option<i64>,
    ) -> crate::common::error::Result<()> {
        self.register_task_with_state(definition, initial_last_run, None)
            .await
    }

    /// 注册任务并同时指定初始 `last_run` 与**调度游标** `last_fire`。
    ///
    /// 游标只影响日历类（`Daily` / `Weekly` / `Once`）的判定：`anchor <= last_fire`
    /// 视为「该时刻点已结算」，不会再按 [`MissedPolicy`] 判补跑。
    ///
    /// 用途（编辑已有任务）：注销后用同一 id 重新注册时，内存里的游标会随
    /// `unregister_task` 一起丢掉，若新注册不带上「当前时刻点已结算」，一条
    /// `RunOnce` 的每日任务每保存一次就会立刻补跑一次（编辑 prompt 却触发了执行，
    /// 用户完全预料不到）。**改调度规格时不要带游标**——新规格应当从零判定。
    pub async fn register_task_with_state(
        &self,
        definition: TaskDefinition,
        initial_last_run: Option<i64>,
        initial_last_fire: Option<i64>,
    ) -> crate::common::error::Result<()> {
        let mut tasks = self.tasks.write().await;
        if tasks.iter().any(|t| t.definition.id == definition.id) {
            return Err(TianyanError::Custom(format!(
                "内部错误：任务 ID 已存在：{}",
                definition.id
            )));
        }

        tracing::info!(
            "注册任务：{} ({})，调度：{}，错过策略：{}",
            definition.name,
            definition.id,
            definition.schedule.describe(),
            definition.missed.describe()
        );

        tasks.push(RegisteredTask {
            definition,
            last_run: initial_last_run,
            last_fire: initial_last_fire,
            run_count: 0,
            last_error: None,
            last_skipped: None,
        });
        drop(tasks);
        // 立即落盘（merge）：让动态注册的任务进入 scheduler_state.json
        // （下次启动可恢复），且**不得清空**其它任务已有的持久化状态
        self.persist_state_merge().await;
        Ok(())
    }

    /// 运行期动态注册任务（下个扫描周期生效；用于用户动态创建的任务）。
    pub async fn register_dynamic_task(
        self: &Arc<Self>,
        definition: TaskDefinition,
        _ctx: Arc<TaskContext>,
    ) -> crate::common::error::Result<()> {
        self.register_task(definition).await
    }

    /// 运行期动态注册任务（带初始 last_run；用于**启动后恢复**的用户任务，
    /// 避免每次重启被误判"从未执行"而立即补跑）。
    pub async fn register_dynamic_task_with_last_run(
        self: &Arc<Self>,
        definition: TaskDefinition,
        last_run: Option<i64>,
        _ctx: Arc<TaskContext>,
    ) -> crate::common::error::Result<()> {
        self.register_task_with_last_run(definition, last_run).await
    }

    /// 运行期动态注册任务（带初始 last_run **与调度游标**；用于编辑后重新注册）。
    ///
    /// 语义见 [`Self::register_task_with_state`]：改prompt/名称/工作区不该让日历类
    /// 任务多跑一次，改调度规格则该从零判定（调用方传 `None`）。
    pub async fn register_dynamic_task_with_state(
        self: &Arc<Self>,
        definition: TaskDefinition,
        last_run: Option<i64>,
        last_fire: Option<i64>,
        _ctx: Arc<TaskContext>,
    ) -> crate::common::error::Result<()> {
        self.register_task_with_state(definition, last_run, last_fire)
            .await
    }

    /// 注销任务（幂等；持久化状态保留——同 id 重注册可延续）。
    pub async fn unregister_task(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        let before = tasks.len();
        tasks.retain(|t| t.definition.id != task_id);
        tasks.len() != before
    }

    /// 停止调度器（扫描循环在下个检查点退出）。
    pub async fn stop(&self) -> crate::common::error::Result<()> {
        if !self
            .running
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        tracing::info!("任务调度器停止中...");
        Ok(())
    }

    /// 检查调度器是否正在运行。
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取所有已注册的任务 ID。
    pub async fn get_task_ids(&self) -> Vec<String> {
        self.tasks
            .read()
            .await
            .iter()
            .map(|t| t.definition.id.clone())
            .collect()
    }

    /// 获取任务信息。
    pub async fn get_task_info(&self, task_id: &str) -> Option<(String, u64, u64)> {
        self.tasks
            .read()
            .await
            .iter()
            .find(|t| t.definition.id == task_id)
            .map(|t| {
                (
                    t.definition.name.clone(),
                    t.definition.interval_secs,
                    t.run_count,
                )
            })
    }

    /// 获取全部任务的运行状态快照（按任务 ID 排序）。
    pub async fn snapshot(&self) -> Vec<TaskStatus> {
        let now = Utc::now().timestamp();
        let mut statuses: Vec<TaskStatus> = self
            .tasks
            .read()
            .await
            .iter()
            .map(|t| {
                let last_run_ago_secs = t.last_run.map(|lr| (now - lr).max(0) as u64);
                let last_fire_ago_secs = t.last_fire.map(|lf| (now - lf).max(0) as u64);
                let next_due_in_secs = match &t.definition.schedule {
                    // 相对语义：last_run + 间隔
                    Schedule::Every { secs } => match t.last_run {
                        Some(lr) => (lr + *secs as i64 - now).max(0) as u64,
                        None => 0, // 从未执行 = 立即到期
                    },
                    // 绝对时刻点：按日历算「下一个」，不是 last_run + 间隔
                    schedule => match schedule.next_occurrence_after(now) {
                        Some(ts) => (ts - now).max(0) as u64,
                        // 一次性已过 / 每周无选中日：没有「下次」
                        None => 0,
                    },
                };
                TaskStatus {
                    id: t.definition.id.clone(),
                    name: t.definition.name.clone(),
                    priority: t.definition.priority,
                    interval_secs: t.definition.interval_secs,
                    schedule: t.definition.schedule.describe(),
                    missed: t.definition.missed.describe(),
                    run_count: t.run_count,
                    last_run_ago_secs,
                    next_due_in_secs,
                    last_fire_ago_secs,
                    last_error: t.last_error.clone(),
                    last_skipped: t.last_skipped.clone(),
                }
            })
            .collect();
        statuses.sort_by(|a, b| a.id.cmp(&b.id));
        statuses
    }

    /// 执行指定任务。
    ///
    /// # 参数
    /// - `task_id`: 任务 ID
    /// - `ctx`: 任务上下文
    ///
    /// # 返回
    /// 任务执行结果
    pub async fn execute_task(&self, task_id: &str, ctx: &TaskContext) -> Option<TaskResult> {
        // 先取 handler 即释放写锁：长任务（如演化综述的 LLM 调用）执行期间
        // 不得阻塞 snapshot()/get_task_ids() 等读路径。
        let (name, handler) = {
            let mut tasks = self.tasks.write().await;
            let task = tasks.iter_mut().find(|t| t.definition.id == task_id)?;
            (
                task.definition.name.clone(),
                task.definition.handler.clone(),
            )
        };

        tracing::debug!("执行任务：{} ({})", name, task_id);

        // 标记执行中（顺序执行语义下至多一个；供状态查询展示"执行中"）
        *self.executing.write().await = Some(task_id.to_string());
        // 任务级超时：单任务 hang（LLM 请求无响应等）此前会拖死整个扫描循环
        // （串行执行 ⇒ 后续所有定时任务停摆）——超时即中断该任务并记失败
        let timeout_secs = self.task_timeout_secs;
        let result = match tokio::time::timeout(
            tokio::time::Duration::from_secs(timeout_secs),
            handler.execute(ctx),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                tracing::error!(
                    task = %task_id,
                    timeout_secs,
                    "任务执行超时（已中断，继续后续任务）"
                );
                TaskResult::failed(TianyanError::timeout(format!(
                    "scheduler: 任务 {task_id} 超过 {timeout_secs}s 未完成（已中断）"
                )))
            }
        };
        *self.executing.write().await = None;

        // 执行完成后补记运行统计 + last_run + last_error（重新获取写锁，短临界区）。
        // last_error：失败可追溯（洞察页此前只显示次数，失败完全不可见）。
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.iter_mut().find(|t| t.definition.id == task_id) {
                task.last_run = Some(Utc::now().timestamp());
                task.run_count += 1;
                task.last_error = result
                    .error
                    .as_ref()
                    .map(|e| truncate_error(&e.to_string()));
                // 业务级跳过：成功时覆盖（跳过写原因、正常执行清空），失败时保留旧值
                // （避免状态里留着过期信息，也不让失败把跳过原因抹掉）
                let skipped_now = result.skipped.as_deref().map(truncate_error);
                if result.success {
                    task.last_skipped = skipped_now;
                }
            }
        }

        if result.success {
            match result.skipped.as_deref() {
                Some(reason) => tracing::info!("任务已跳过：{} - {}", task_id, reason),
                None => tracing::info!(
                    "任务执行成功：{}，处理了 {} 个条目",
                    task_id,
                    result.processed_count
                ),
            }
        } else if let Some(ref error) = result.error {
            tracing::error!("任务执行失败：{} - {}", task_id, error);
        }

        Some(result)
    }

    /// 当前正在执行的任务 ID（顺序执行语义下至多一个；None = 空闲）。
    pub async fn executing_task_id(&self) -> Option<String> {
        self.executing.read().await.clone()
    }
}

/// 状态展示/持久化用的错误串截断上限（错误全文进 tracing 日志）。
const LAST_ERROR_MAX_CHARS: usize = 300;

fn truncate_error(msg: &str) -> String {
    let mut out: String = msg.chars().take(LAST_ERROR_MAX_CHARS).collect();
    if msg.chars().count() > LAST_ERROR_MAX_CHARS {
        out.push('…');
    }
    out
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::SummaryEngine;

    /// 构造调度器测试上下文（TestTask 不消费字段，Mock 服务足够）。
    fn make_context() -> TaskContext {
        let vfs: Arc<MockVfs> = Arc::new(MockVfs::new());
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let config = Arc::new(TianyanConfig::default());
        TaskContext::new(vfs, summary_engine, None, config)
    }

    /// 本轮扫描判定为「要处理」的任务 ID（含判定为跳过的）。
    async fn due_ids(s: &TaskScheduler) -> Vec<String> {
        s.plan_due().await.into_iter().map(|d| d.id).collect()
    }

    /// 构造本地时刻的 epoch 秒（`plan_one` 的纯函数测试用）。
    fn ts_of(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        use chrono::TimeZone as _;
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("时刻应可构造")
            .timestamp()
    }

    struct TestTask {
        /// 执行记录（跨实例共享，断言串行顺序用）。
        log: Arc<Mutex<Vec<String>>>,
    }

    impl TestTask {
        fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
            Self { log }
        }
    }

    #[async_trait::async_trait]
    impl TaskHandler for TestTask {
        async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
            self.log.lock().unwrap().push("ran".to_string());
            TaskResult::success(1)
        }

        fn name(&self) -> &str {
            "test_task"
        }
    }

    fn make_handler() -> Arc<dyn TaskHandler> {
        Arc::new(TestTask::new(Arc::new(Mutex::new(Vec::new()))))
    }

    #[test]
    fn test_interval_from_cron_patterns() {
        // */N 秒
        assert_eq!(interval_from_cron("*/30 * * * * *"), Some(30));
        // 0 */N 分钟
        assert_eq!(interval_from_cron("0 */30 * * * *"), Some(1800));
        assert_eq!(interval_from_cron("0 */1 * * * *"), Some(60));
        // 每天 H:M → 24h
        assert_eq!(interval_from_cron("0 30 9 * * *"), Some(86400));
        // 每 6 小时（内置任务旧表达式）→ 无法精确换算为间隔? 0 0 */6 * * * → 0 */6 小时
        // 注意：hour 字段是 */N 的情况
        assert_eq!(
            interval_from_cron("0 0 */6 * * *"),
            None,
            "hour 的 */N 属于「每 6 小时的整点」语义，不换算（回退默认间隔）"
        );
        // 无法识别
        assert_eq!(interval_from_cron("garbage"), None);
    }

    #[test]
    fn test_next_due_at() {
        // 从未执行 = 立即到期
        assert_eq!(next_due_at(None, 600, 1000), 1000);
        // last_run + interval
        assert_eq!(next_due_at(Some(500), 600, 1000), 1100);
        // 已过期 = max(now)
        assert_eq!(next_due_at(Some(100), 600, 1000), 1000);
    }

    #[tokio::test]
    async fn test_register_and_due() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new("a", "任务A", 3600, make_handler()))
            .await
            .unwrap();
        // 从未执行 → 到期
        assert_eq!(due_ids(&scheduler).await, vec!["a".to_string()]);
        // 执行后 3600s 内不再到期
        let ctx = Arc::new(make_context());
        scheduler.execute_task("a", &ctx).await;
        assert!(due_ids(&scheduler).await.is_empty());
    }

    /// **注册晚于调度器启动**的任务必须用自身的 last_run_at 初始化——否则
    /// `load_state()`（启动时执行，此时任务尚未注册）覆盖不到，`last_run`
    /// 为 None 会被判"从未执行 = 立即到期"，表现为**每次重启/重装都立刻
    /// 补跑一次**（实测 bug：定时智能体任务）。
    #[tokio::test]
    async fn test_register_with_initial_last_run_defers_due_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("scheduler_state.json");
        let scheduler = TaskScheduler::new().with_state_path(state_path.clone());

        let now = Utc::now().timestamp();
        scheduler
            .register_task_with_last_run(
                TaskDefinition::new("late", "启动后注册", 86400, make_handler()),
                Some(now),
            )
            .await
            .unwrap();

        // 刚跑过（间隔 1 天）→ 不到期（修复前会是 [late]）
        assert!(
            due_ids(&scheduler).await.is_empty(),
            "带 last_run 注册的任务不应被判定立即到期"
        );

        // 注册即落盘：state 文件里已有该条目（下次启动 load_state 可恢复）
        let raw = tokio::fs::read_to_string(&state_path).await.unwrap();
        assert!(
            raw.contains("late"),
            "注册后应立即写入 scheduler_state.json，实际：{raw}"
        );
    }

    /// **编辑已有任务**：注销后同一 id 重新注册必须能带上「当前时刻点已结算」的游标。
    ///
    /// 判别力：不带游标（旧路径）时，一条 `RunOnce` 的每日任务**每保存一次就立刻
    /// 补跑一次**——用户只是改了提示词，却被触发了一次执行。两条断言互为对照：
    /// 带游标 → 不到期；不带 → 立刻到期。
    #[tokio::test]
    async fn test_register_with_initial_last_fire_settles_current_occurrence() {
        let now = Utc::now().timestamp();
        let daily = Schedule::Daily {
            at: "15:30".to_string(),
        };
        let anchor = daily
            .most_recent_occurrence(now)
            .expect("日历类必有最近时刻点");
        let def = || {
            TaskDefinition::new("edited", "编辑后重注册", 86400, make_handler())
                .with_schedule(daily.clone(), MissedPolicy::RunOnce)
        };

        // 带游标：该时刻点已结算 → 不到期（即便 RunOnce 且已过很久）
        let settled = TaskScheduler::new();
        settled
            .register_task_with_state(def(), Some(anchor), Some(anchor))
            .await
            .unwrap();
        assert!(
            due_ids(&settled).await.is_empty(),
            "游标已结算的时刻点不得重复补跑（编辑 prompt 不应触发执行）"
        );

        // 对照：不传游标（旧行为）→ 立刻补跑
        let fresh = TaskScheduler::new();
        fresh
            .register_task_with_state(def(), Some(anchor), None)
            .await
            .unwrap();
        assert_eq!(
            due_ids(&fresh).await,
            vec!["edited".to_string()],
            "不带游标时保持旧的补跑语义（对照：证明上一条不是恒真）"
        );
    }

    /// **收盘场景回归**（本次改动的起因）：每天 15:30 跑，且只允许收盘后窗口内补跑。
    ///
    /// 判别力：旧模型（间隔 24h + 立即补跑）会在「次日盘中重启」时立刻执行——业务上
    /// 必须避免。这里锁住三条：
    /// 1. 准点（15:30）跑；
    /// 2. 昨天 15:30 的时刻点在次日 10:00（盘中）已超出 8h 窗口 → **跳过**；
    /// 3. 昨天那个时刻点已结算（`last_fire`）→ 次日 10:00 压根不再判定。
    #[test]
    fn calendar_task_never_fires_during_trading_hours() {
        let close = Schedule::Daily {
            at: "15:30".to_string(),
        };
        let within = MissedPolicy::Within {
            grace_secs: 8 * 3600,
        };
        let tick = 60;
        let at_close = ts_of(2026, 11, 4, 15, 30);
        let next_morning = ts_of(2026, 11, 5, 10, 0);

        // 1) 准点 → 跑
        assert!(
            matches!(
                plan_one(&close, within, None, None, at_close, tick),
                FireDecision::Fire { .. }
            ),
            "准点应触发"
        );
        // 2) 盘中（昨天 15:30 已过 18.5h > 8h 窗口）→ 跳过，而不是补跑
        assert_eq!(
            plan_one(&close, within, None, None, next_morning, tick),
            FireDecision::Skip { delay: 66600 },
            "盘中重启不得补跑收盘任务"
        );
        // 3) 昨天时刻点已结算 → 不再判定（防重复补跑）
        assert_eq!(
            plan_one(&close, within, None, Some(at_close), next_morning, tick),
            FireDecision::NotDue
        );
        // 对照：旧策略 RunOnce 会补跑（维护类任务要的正是这个行为）
        assert!(
            matches!(
                plan_one(
                    &close,
                    MissedPolicy::RunOnce,
                    None,
                    None,
                    next_morning,
                    tick
                ),
                FireDecision::Fire { .. }
            ),
            "RunOnce 应保留旧的「开机补跑」语义"
        );
        // 对照：Skip 策略只容忍一个 tick 的准点容差
        assert_eq!(
            plan_one(&close, MissedPolicy::Skip, None, None, at_close + 120, tick),
            FireDecision::Skip { delay: 120 }
        );
    }

    /// 间隔制不受 `missed` 影响：到点就跑、没到就不跑（维护类任务的既有语义）。
    #[test]
    fn interval_task_ignores_missed_policy() {
        let every = Schedule::Every { secs: 600 };
        let now = ts_of(2026, 11, 4, 15, 30);
        for m in [
            MissedPolicy::RunOnce,
            MissedPolicy::Skip,
            MissedPolicy::Within { grace_secs: 0 },
        ] {
            assert_eq!(
                plan_one(&every, m, Some(now - 601), None, now, 60),
                FireDecision::Fire { delay: 0 },
                "间隔制应到点即跑（missed={m:?}）"
            );
            assert_eq!(
                plan_one(&every, m, Some(now - 100), None, now, 60),
                FireDecision::NotDue,
                "未到间隔不应到期（missed={m:?}）"
            );
        }
    }

    /// 对照：不传 last_run（既有行为）→ 从未执行 → 立即到期（补跑语义）。
    #[tokio::test]
    async fn test_register_without_last_run_is_due_immediately() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new(
                "fresh",
                "新任务",
                86400,
                make_handler(),
            ))
            .await
            .unwrap();
        assert_eq!(due_ids(&scheduler).await, vec!["fresh".to_string()]);
    }

    #[tokio::test]
    async fn test_snapshot_fields() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new("b", "任务B", 120, make_handler()))
            .await
            .unwrap();
        scheduler
            .register_task(TaskDefinition::new("a", "任务A", 60, make_handler()))
            .await
            .unwrap();

        let snapshot = scheduler.snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].id, "a");
        assert_eq!(snapshot[0].interval_secs, 60);
        assert_eq!(snapshot[0].run_count, 0);
        assert!(snapshot[0].last_run_ago_secs.is_none());
        assert_eq!(snapshot[0].next_due_in_secs, 0); // 从未执行 = 立即到期
    }

    #[tokio::test]
    async fn test_scanner_executes_due_tasks_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("scheduler_state.json");
        let log = Arc::new(Mutex::new(Vec::new()));

        let scheduler = Arc::new(
            TaskScheduler::new()
                .with_tick_secs(1)
                .with_state_path(state_path.clone()),
        );
        scheduler
            .register_task(TaskDefinition::new(
                "t1",
                "任务1",
                1,
                Arc::new(TestTask::new(log.clone())),
            ))
            .await
            .unwrap();
        // 3600s 间隔的任务不应运行
        scheduler
            .register_task(TaskDefinition::new("t2", "任务2", 3600, make_handler()))
            .await
            .unwrap();

        let ctx = Arc::new(make_context());
        TaskScheduler::start_with_scheduler(scheduler.clone(), ctx)
            .await
            .unwrap();

        // t1 间隔 1s + tick 1s → 3 秒内应至少执行 2 次；
        // t2 间隔 3600s：从未执行 → 首个 tick 执行一次（首跑/初始化语义），之后不再到期
        tokio::time::sleep(tokio::time::Duration::from_millis(3200)).await;

        // 快照须在 shutdown 前取：shutdown 会清空任务表（释放 handler 断循环引用）
        let snapshot = scheduler.snapshot().await;
        scheduler.shutdown().await.unwrap();

        let runs = log.lock().unwrap().len();
        assert!(runs >= 2, "t1 应已执行多次，实际 {runs}");

        let t1 = snapshot.iter().find(|s| s.id == "t1").unwrap();
        assert!(t1.run_count >= 2);
        let t2 = snapshot.iter().find(|s| s.id == "t2").unwrap();
        assert_eq!(
            t2.run_count, 1,
            "从未执行任务首 tick 跑一次，之后进入间隔等待"
        );

        // 持久化文件已落盘且含 last_run
        let content = tokio::fs::read_to_string(&state_path).await.unwrap();
        assert!(content.contains("t1"));
    }

    #[tokio::test]
    async fn test_state_reload_enables_catch_up() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("scheduler_state.json");

        // 预写状态：t1 上次执行在 7200s 前（间隔 3600s → 已超期，应补跑）
        let stale = serde_json::json!({
            "t1": { "last_run": Utc::now().timestamp() - 7200, "run_count": 5 }
        });
        tokio::fs::write(&state_path, stale.to_string())
            .await
            .unwrap();

        let scheduler = Arc::new(
            TaskScheduler::new()
                .with_tick_secs(1)
                .with_state_path(state_path),
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        scheduler
            .register_task(TaskDefinition::new(
                "t1",
                "任务1",
                3600,
                Arc::new(TestTask::new(log.clone())),
            ))
            .await
            .unwrap();

        let ctx = Arc::new(make_context());
        TaskScheduler::start_with_scheduler(scheduler.clone(), ctx)
            .await
            .unwrap();

        // 首个扫描周期（≤2s）应补跑一次；补跑后 3600s 内不再跑
        tokio::time::sleep(tokio::time::Duration::from_millis(2200)).await;

        // 快照须在 shutdown 前取（shutdown 清空任务表）
        let snapshot = scheduler.snapshot().await;
        scheduler.shutdown().await.unwrap();

        let runs = log.lock().unwrap().len();
        assert_eq!(runs, 1, "超期任务应恰好补跑一次，实际 {runs}");
        assert_eq!(snapshot[0].run_count, 6, "run_count 跨重启延续（5+1）");
    }

    /// P1-11 回归：单任务超时被中断并记失败（此前 hang 任务拖死整个扫描循环）。
    #[tokio::test]
    async fn test_execute_task_times_out_and_records_failure() {
        struct HangTask;
        #[async_trait::async_trait]
        impl TaskHandler for HangTask {
            async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
                tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
                TaskResult::success(0)
            }
            fn name(&self) -> &str {
                "hang"
            }
        }

        let scheduler = TaskScheduler::new().with_task_timeout_secs(1);
        scheduler
            .register_task(TaskDefinition::new(
                "hang",
                "挂起任务",
                60,
                Arc::new(HangTask),
            ))
            .await
            .unwrap();
        let ctx = make_context();

        let started = std::time::Instant::now();
        let result = scheduler.execute_task("hang", &ctx).await.unwrap();
        assert!(!result.success, "超时任务应记为失败");
        assert!(
            result
                .error
                .as_ref()
                .map(|e| e.is_timeout())
                .unwrap_or(false),
            "错误应为 timeout 语义: {:?}",
            result.error
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "应在超时值附近返回（实际 {:?}）",
            started.elapsed()
        );

        // last_error 可追溯（洞察页此前只显示次数）
        let statuses = scheduler.snapshot().await;
        let st = statuses.iter().find(|s| s.id == "hang").unwrap();
        assert!(
            st.last_error.as_deref().unwrap_or("").contains("超时"),
            "last_error 应记录超时: {:?}",
            st.last_error
        );
    }

    /// P1-15 回归：损坏的状态文件被另存 .corrupt（保留证据），原子写不留 .tmp。
    #[tokio::test]
    async fn test_state_file_corrupt_quarantined_and_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scheduler_state.json");
        tokio::fs::write(&path, "{ 截断的 JSON").await.unwrap();

        let scheduler = TaskScheduler::new().with_state_path(path.clone());
        scheduler.load_state().await;
        assert!(!path.exists(), "损坏文件应被移走（另存 .corrupt）");
        assert!(
            dir.path().join("scheduler_state.json.corrupt").exists(),
            "损坏证据应保留"
        );

        // 原子写：落盘可解析、无 .tmp 残留
        scheduler.persist_state().await;
        assert!(path.exists(), "持久化应重建状态文件");
        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&raw).is_ok());
        assert!(
            !dir.path().join("scheduler_state.json.tmp").exists(),
            "不应残留临时文件"
        );
    }

    #[tokio::test]
    async fn test_register_duplicate_rejected() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new("dup", "任务", 60, make_handler()))
            .await
            .unwrap();
        assert!(scheduler
            .register_task(TaskDefinition::new("dup", "任务", 60, make_handler()))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_unregister_task() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new("gone", "任务", 60, make_handler()))
            .await
            .unwrap();
        assert!(scheduler.unregister_task("gone").await);
        assert!(!scheduler.unregister_task("gone").await);
        assert!(scheduler.get_task_ids().await.is_empty());
    }
}
