//! 定时智能体任务管理器。
//!
//! 任务定义持久化到 JSON；实际调度由核心 TaskScheduler 承担（完整 cron）——
//! 本管理器只负责：任务注册/注销、持久化、结果回写与列表展示。
//!
//! # 分层（避免循环引用）
//!
//! 依赖方向**单向向下**（manager 顶层 → registrar 中层 → handler 底层）：
//! - `ScheduledAgentTaskManager`（顶层：任务定义 + 持久化 + API）**不持有**
//!   scheduler/task_ctx——经 [`TaskRegistrar`] 接口注册（装配层注入实现）；
//! - `ScheduledAgentTaskHandler`（底层：执行器）**不依赖** manager 具体类型——
//!   经 [`TaskResultSink`] 接口回写结果（[`ResultSinkBridge`] 实持 [Weak] 打破
//!   循环：manager → registrar → scheduler → handler → (Weak) manager）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tianyan::agent::AgentCoordinator;
use tianyan::common::types::Message;
use tianyan::scheduler::{TaskContext, TaskDefinition, TaskHandler, TaskResult, TaskScheduler};
use tianyan::session::{SessionHeader, SessionManager};
use tianyan::TianyanError;
use tokio::sync::RwLock;

use crate::scheduled_tasks::types::UpdateScheduledTaskRequest;
use crate::scheduled_tasks::types::{CreateScheduledTaskRequest, ScheduledAgentTask};

/* ───────── 分层接口：结果回写（handler → manager，经接口 + Weak） ───────── */

/// 结果回写接口：handler 执行完任务后回写结果，**只依赖此接口**，
/// 不依赖 manager 具体类型——底层不反向依赖顶层（分层）。
#[async_trait::async_trait]
pub trait TaskResultSink: Send + Sync {
    /// 记录一次执行结果（更新 last_run_at / last_result / next_run_at）。
    async fn record_result(&self, task_id: &str, result: &str);
}

/// 结果回写桥：handler → 接口 → [Weak]<manager>。
///
/// [Weak] 不增加强引用计数：manager drop 后 `upgrade()` 返回 None（应用关停中
/// 结果丢失可接受），因此 manager → registrar → scheduler → handler → manager
/// 不构成循环引用。
pub struct ResultSinkBridge {
    manager: std::sync::Weak<ScheduledAgentTaskManager>,
}

impl ResultSinkBridge {
    /// 创建桥（持有 manager 的 Weak 引用）。
    pub fn new(manager: &Arc<ScheduledAgentTaskManager>) -> Self {
        Self {
            manager: Arc::downgrade(manager),
        }
    }
}

#[async_trait::async_trait]
impl TaskResultSink for ResultSinkBridge {
    async fn record_result(&self, task_id: &str, result: &str) {
        if let Some(manager) = self.manager.upgrade() {
            manager.record_result(task_id, result).await;
        }
    }
}

/* ───────── 分层接口：任务注册（manager → scheduler，经接口） ───────── */

/// 任务注册接口：manager 经此把任务注册进调度器，**不持有** scheduler/task_ctx。
///
/// 装配层实现 [`SchedulerRegistrar`] 并注入，依赖方向：manager → 接口 →调度器。
#[async_trait::async_trait]
pub trait TaskRegistrar: Send + Sync {
    /// 注册任务到调度器（handler 已构建；注册即起扫描判定）。
    ///
    /// `settled_fire`：注册时视为**已结算**的调度时刻点（epoch 秒）。注销会让
    /// 调度器丢掉内存游标（`last_fire`），编辑任务后重新注册时传「最近一个已过的
    /// 时刻点」即可保住「这个时刻点处理过了」——否则一条 `RunOnce` 的每日任务会
    /// 因为游标为空而立刻补跑一次（用户只是改了提示词）。新建 / 启动恢复传 `None`
    /// （保持既有的「错过就补跑一次」语义）。间隔制任务不受它影响（游标由
    /// `last_run` 承担）。
    async fn register(
        &self,
        task: &ScheduledAgentTask,
        handler: Arc<dyn TaskHandler>,
        settled_fire: Option<i64>,
    ) -> Result<(), String>;
    /// 从调度器注销任务（任务不存在时幂等）。
    async fn unregister(&self, task_id: &str);
}

/// 调度器注册器：装配层实现的 [`TaskRegistrar`]。
///
/// 持有 scheduler/task_ctx 的共享句柄（与 AppState 装配顺序解耦：
/// 注册器先创建、后 bind，manager 无需感知调度器存在与否）。
pub struct SchedulerRegistrar {
    scheduler: Arc<RwLock<Option<Arc<TaskScheduler>>>>,
    task_ctx: Arc<RwLock<Option<Arc<TaskContext>>>>,
}

impl Default for SchedulerRegistrar {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerRegistrar {
    /// 创建注册器（scheduler/task_ctx 初始 None，装配后 bind）。
    pub fn new() -> Self {
        Self {
            scheduler: Arc::new(RwLock::new(None)),
            task_ctx: Arc::new(RwLock::new(None)),
        }
    }

    /// 绑定调度器与任务上下文（start_server 创建 scheduler 后调用；
    /// 无 Provider 时为 None，注册静默跳过）。
    pub async fn bind(
        &self,
        scheduler: Option<Arc<TaskScheduler>>,
        task_ctx: Option<Arc<TaskContext>>,
    ) {
        *self.scheduler.write().await = scheduler;
        *self.task_ctx.write().await = task_ctx;
    }
}

#[async_trait::async_trait]
impl TaskRegistrar for SchedulerRegistrar {
    async fn register(
        &self,
        task: &ScheduledAgentTask,
        handler: Arc<dyn TaskHandler>,
        settled_fire: Option<i64>,
    ) -> Result<(), String> {
        let Some(sched) = self.scheduler.read().await.clone() else {
            return Ok(()); // 调度器未装配（无 Provider）：静默跳过
        };
        let Some(ctx) = self.task_ctx.read().await.clone() else {
            return Ok(());
        };
        let definition = TaskDefinition::new(
            task.id.clone(),
            task.name.clone(),
            task.interval_secs,
            handler,
        )
        // 调度规格以任务自己的为准（`schedule` 显式设置优先，否则 = 间隔制），
        // 错过策略同理（缺省沿用旧的「开机就补跑」）
        .with_schedule(task.effective_schedule(), task.effective_missed());
        sched
            // 传任务自身持久化的上次运行时间：调度器启动时该任务尚未注册
            // （load_state 覆盖不到），不传会被判"从未执行"→ 每次重启立即补跑
            // 另传「已结算的时刻点」：编辑后重新注册时保住游标（见 trait 文档）
            .register_dynamic_task_with_state(definition, task.last_run_at, settled_fire, ctx)
            .await
            .map_err(|e| e.to_string())
    }

    async fn unregister(&self, task_id: &str) {
        if let Some(sched) = self.scheduler.read().await.clone() {
            sched.unregister_task(task_id).await;
        }
    }
}

/* ───────── 底层：执行器（handler，只依赖接口，不依赖 manager） ───────── */

/// 定时智能体任务处理器：到点由 TaskScheduler 调用，在绑定工作区执行 prompt。
pub struct ScheduledAgentTaskHandler {
    agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
    session_manager: Arc<dyn SessionManager>,
    /// 会话工作集注册表（ADR-035 §3：任务会话重建后失效缓存）。
    working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
    task_id: String,
    workspace: String,
    prompt: String,
    /// 结果回写接口（不依赖 manager 具体类型——分层；桥实持 Weak 打破循环）。
    result_sink: Arc<dyn TaskResultSink>,
}

impl ScheduledAgentTaskHandler {
    fn new(
        agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
        session_manager: Arc<dyn SessionManager>,
        working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
        task_id: String,
        workspace: String,
        prompt: String,
        result_sink: Arc<dyn TaskResultSink>,
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
            working_sets,
            task_id,
            workspace,
            prompt,
            result_sink,
        }
    }

    /// 任务专属会话 ID（**幂等**）：`sched-task-{task_id}`——task_id 本身已
    /// 带前缀时不再叠加。
    ///
    /// 历史实现无条件套前缀，task_id 又是 `sched-task-…`，于是产出
    /// `sched-task-sched-task-…` 双前缀（会话列表/tab 标题展示错乱，
    /// 且与任务 id 无法直观对应）。
    fn session_id_for(task_id: &str) -> String {
        if task_id.starts_with("sched-task-") {
            task_id.to_string()
        } else {
            format!("sched-task-{task_id}")
        }
    }

    /// 生效会话 ID：**存量（历史双前缀）会话存在时沿用**——不迁移数据、
    /// 不割裂历史（旧会话继续可见可读）；新任务用规范 ID。
    async fn resolve_session_id(&self) -> String {
        let canonical = Self::session_id_for(&self.task_id);
        let legacy = format!("sched-task-{}", self.task_id);
        if legacy != canonical
            && self
                .session_manager
                .get_session(&legacy)
                .await
                .ok()
                .flatten()
                .is_some()
        {
            return legacy;
        }
        canonical
    }

    /// 确保该任务有绑定工作区的专属会话（首次创建，之后复用；返回会话 ID）。
    async fn ensure_session(&self) -> String {
        let sid = self.resolve_session_id().await;
        if self
            .session_manager
            .get_session(&sid)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return sid;
        }
        // 工作目录**与消息一次写入**（`create_session_with_header`），不留
        // 「会话已建但目录为空」的中间态——左栏按 `working_directory` 分组，
        // 空串归「默认」组，落在这个窗口里的刷新会把任务会话挂到「默认」下面。
        // 路径检查失败必须告警：早先是 `let _ =` 静默跳过，目录永远绑不上也查不到线索。
        let mut header = SessionHeader::default();
        if Path::new(&self.workspace).is_dir() {
            header.working_directory = Some(self.workspace.clone());
        } else {
            tracing::warn!(
                workspace = %self.workspace,
                session_id = %sid,
                "定时任务的工作目录不存在，任务会话不绑定目录"
            );
        }
        if self
            .session_manager
            .create_session_with_header(&sid, Message::user(&self.prompt), header)
            .await
            .is_ok()
        {
            // ADR-039 收口 ②：清空预写消息经工作集（唯一写入口；
            // 缓存同步推进 → 不再需要手动 remove 兜缓存分叉）
            if let Ok(ws) = self.working_sets.ensure(&sid).await {
                let _ = ws.rewrite(&[]).await;
            }
        }
        sid
    }

    /// 调用 agent 执行任务 prompt（绑定工作区），返回结果摘要。
    ///
    /// 返回 `Ok(摘要)`（成功；空响应归一为"(空响应)"）或 `Err(原因)`——
    /// **失败必须走 Err**：旧实现把失败也塞进 `Some("执行失败: …")`，使
    /// `execute()` 的失败分支不可达，调度器统计（run_count/last_error）
    /// 永远显示成功（T0-11：模型调用全挂时用户看不到失败信号）。
    async fn run_agent(&self) -> Result<String, String> {
        let sid = self.ensure_session().await;
        let msg = Message::user(&self.prompt);
        let agent = self.agent_lock.read().await.clone();
        match agent.process_message(&sid, &msg, None, None, None).await {
            Ok(resp) => {
                let content = resp.content.trim();
                Ok(if content.is_empty() {
                    "(空响应)".to_string()
                } else {
                    content.chars().take(500).collect()
                })
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 把**部分字段**更新应用到任务定义（纯函数：校验 + 落盘双写 + 重算 `next_run_at`）。
///
/// 抽成自由函数是为了可测：`update` 要碰调度器注册（装配件），而「哪些字段被改、
/// 改成什么、要不要同步 `interval_secs`、停用时清不清 `next_run_at`」这套语义与
/// 调度器无关。
///
/// `last_run_at` / `last_result` **不动**：它们记录真实历史，编辑定义不该改写它们，
/// 它们也不参与「是否该跑」的判定（那是调度器游标的职责）。
fn apply_update(
    old: &ScheduledAgentTask,
    req: &UpdateScheduledTaskRequest,
    now: i64,
) -> Result<ScheduledAgentTask, TianyanError> {
    let mut task = old.clone();
    if let Some(name) = req.name.as_deref() {
        let name = name.trim();
        if name.is_empty() {
            return Err(TianyanError::invalid_input(
                "server: scheduled_tasks: 任务名称不能为空",
            ));
        }
        task.name = name.to_string();
    }
    if let Some(prompt) = req.prompt.as_deref() {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(TianyanError::invalid_input(
                "server: scheduled_tasks: 任务指令不能为空",
            ));
        }
        task.prompt = prompt.to_string();
    }
    if let Some(workspace) = req.workspace.as_deref() {
        // 空串是合法值（= 默认目录），不做非空校验
        task.workspace = workspace.trim().to_string();
    }
    match (&req.schedule, req.interval_secs) {
        // `schedule` 优先（整体替换）；只给 `interval_secs` 时等价于间隔制
        (Some(schedule), _) => task.schedule = Some(schedule.clone()),
        (None, Some(secs)) => task.schedule = Some(tianyan::scheduler::Schedule::Every { secs }),
        (None, None) => {}
    }
    if let Some(missed) = req.missed {
        task.missed = Some(missed);
    }
    if let Some(enabled) = req.enabled {
        task.enabled = enabled;
    }
    // 间隔下限与创建 / 工具同口径：个人 PC 上更短的间隔只会刷屏
    if let tianyan::scheduler::Schedule::Every { secs } = task.effective_schedule() {
        if secs < 60 {
            return Err(TianyanError::invalid_input(
                "server: scheduled_tasks: 间隔最小 60 秒",
            ));
        }
    }
    // 落盘双写：`schedule` → `interval_secs`（旧版本读新文件仍能跑）
    task.sync_legacy_interval();
    task.next_run_at = if task.enabled {
        task.estimated_next_run(now)
    } else {
        None
    };
    Ok(task)
}

/// 执行结果 → （回写文本, 任务结果）映射单点。
///
/// 失败必须映射为 [`TaskResult::failed`]（T0-11）：调度器据此统计
/// `run_count` / `last_error`，失败走 success 会让失败完全静默。
fn map_run_outcome(run: Result<String, String>) -> (String, TaskResult) {
    match run {
        Ok(text) => (text, TaskResult::success(1)),
        Err(e) => (
            format!("执行失败: {e}"),
            TaskResult::failed(TianyanError::Custom(format!(
                "server: scheduled_tasks: 定时智能体任务执行失败：{e}"
            ))),
        ),
    }
}

/// 把任务清单渲染成一句话（「按名称找任务」失败时的提示用）。
///
/// 让模型**看到有哪些任务**再重试，比只回一句「找不到」有用得多——这也正是不再
/// 额外需要一个「列出定时任务」工具的理由。
fn describe_all(tasks: &[ScheduledAgentTask]) -> String {
    if tasks.is_empty() {
        return "（当前没有任何定时任务）".to_string();
    }
    tasks
        .iter()
        .map(|t| {
            format!(
                "「{}」id={} · {}{}",
                t.name,
                t.id,
                t.effective_schedule().describe(),
                if t.enabled { "" } else { " · 已停用" }
            )
        })
        .collect::<Vec<_>>()
        .join("；")
}

#[async_trait::async_trait]
impl TaskHandler for ScheduledAgentTaskHandler {
    async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
        // 失败可见化（T0-11）：摘要照写（用户能读到原因），但任务状态必须
        // 是 failed（否则调度器统计把失败当成功）。
        let (text, result) = map_run_outcome(self.run_agent().await);
        // 结果经接口回写（manager 已销毁时 upgrade 失败跳过——应用关停中）
        self.result_sink.record_result(&self.task_id, &text).await;
        result
    }

    fn name(&self) -> &str {
        "scheduled_agent_task"
    }
}

/* ───────── 顶层：任务管理器（经接口注册，不持有 scheduler/handler 依赖） ───────── */

/// 定时智能体任务管理器：持久化 + 注册进 TaskScheduler + 列表/结果。
pub struct ScheduledAgentTaskManager {
    agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
    session_manager: Arc<dyn SessionManager>,
    /// 会话工作集注册表（ADR-035 §3：任务会话重建（清空消息）后须失效缓存——
    /// 同一 sid 在 TTL 内重复运行时，旁路清空会留下残旧缓存）。
    working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
    file_path: PathBuf,
    tasks: Arc<RwLock<HashMap<String, ScheduledAgentTask>>>,
    /// 任务注册接口（装配层注入；不持有 scheduler——分层）。
    registrar: Arc<dyn TaskRegistrar>,
}

impl ScheduledAgentTaskManager {
    /// 创建管理器（data_dir 用于持久化任务定义；agent_lock 随热重载更新）。
    pub fn new(
        agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
        session_manager: Arc<dyn SessionManager>,
        data_dir: &Path,
        registrar: Arc<dyn TaskRegistrar>,
        working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
            working_sets,
            file_path: data_dir.join("scheduled_agent_tasks.json"),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            registrar,
        }
    }

    /// 从磁盘加载任务定义（含 ADR-024 迁移：旧 cron → interval_secs，回写迁移结果）。
    async fn load(&self) {
        let content = match std::fs::read_to_string(&self.file_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let list: Vec<ScheduledAgentTask> = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut tasks = HashMap::new();
        let mut migrated = false;
        for mut t in list {
            if t.interval_secs == 0 {
                // 旧 cron 模式 → 间隔换算；无法识别回退每日一次
                let translated = t
                    .cron
                    .as_deref()
                    .and_then(tianyan::scheduler::interval_from_cron);
                t.interval_secs = translated.unwrap_or(86400);
                if translated.is_none() {
                    tracing::warn!(
                        task = %t.name,
                        cron = ?t.cron,
                        fallback_secs = t.interval_secs,
                        "旧 cron 无法换算为间隔（ADR-024），回退每日一次"
                    );
                }
                migrated = true;
            }
            tasks.insert(t.id.clone(), t);
        }
        if migrated {
            tracing::info!("定时任务已从 cron 迁移为间隔制（ADR-024）");
            // 先放入内存再触发一次落盘（由 save 完成）
            *self.tasks.write().await = tasks;
            self.save().await;
            return;
        }
        *self.tasks.write().await = tasks;
    }

    /// 持久化任务定义到磁盘。
    async fn save(&self) {
        let tasks = self.tasks.read().await;
        let list: Vec<&ScheduledAgentTask> = tasks.values().collect();
        match serde_json::to_string_pretty(&list) {
            Ok(json) => {
                let _ = std::fs::write(&self.file_path, json);
            }
            Err(e) => tracing::error!(error = %e, "定时任务持久化失败"),
        }
    }

    /// 启动时加载持久化任务并注册进调度器（重启恢复）。
    pub async fn load_and_register(self: &Arc<Self>) {
        self.load().await;
        let tasks = self.tasks.read().await.clone();
        for task in tasks.values() {
            if task.enabled {
                // 启动恢复：不传已结算时刻点（保持既有的「错过就补跑一次」语义）
                self.register(task, None).await;
            }
        }
        tracing::info!(count = tasks.len(), "已恢复并注册定时智能体任务");
    }

    /// 创建定时任务（持久化 + 注册进调度器）。
    pub async fn create(self: &Arc<Self>, req: &CreateScheduledTaskRequest) -> ScheduledAgentTask {
        let mut task = ScheduledAgentTask::new(
            req.name.clone(),
            req.interval_secs,
            req.workspace.clone(),
            req.prompt.clone(),
        );
        // 新接口：显式调度规格 / 错过策略（旧客户端只给 interval_secs → 走间隔制）
        task.schedule = req.schedule.clone();
        task.missed = req.missed;
        // 落盘双写：`schedule` → `interval_secs`（旧版本读新文件仍能跑）
        task.sync_legacy_interval();
        task.next_run_at = task.estimated_next_run(chrono::Utc::now().timestamp());
        self.tasks
            .write()
            .await
            .insert(task.id.clone(), task.clone());
        self.save().await;
        if task.enabled {
            self.register(&task, None).await;
        }
        tracing::info!(
            name = %task.name,
            schedule = %task.effective_schedule().describe(),
            missed = %task.effective_missed().describe(),
            workspace = %task.workspace,
            "已创建定时智能体任务",
        );
        task
    }

    /// 更新定时任务（**部分字段**语义；不存在 → `not_found`，参数非法 → `invalid_input`）。
    ///
    /// 三件必须做对的事：
    /// 1. **重建 handler**：执行器在构造时就把 `workspace` / `prompt` 克隆走了，
    ///    改完不重新注册等于什么都没改（下次触发仍按旧指令跑）；
    /// 2. **不误触发**：注销会丢掉调度器的内存游标，调度规格没变时要把「当前时刻点
    ///    已结算」带回去（见 [`TaskRegistrar::register`]）——只改提示词不该让任务
    ///    多跑一次；改了规格则从零判定（用户期望新时刻表立即生效）；
    /// 3. **落盘双写**：`schedule` → `interval_secs`（旧版本读新文件仍能跑）。
    ///
    /// `last_run_at` / `last_result` 一律保留：它们记录的是**真实历史**，编辑定义
    /// 不改变「上次什么时候真的跑过」，也不该被用来当「是否该跑」的初值。
    pub async fn update(
        self: &Arc<Self>,
        id: &str,
        req: &UpdateScheduledTaskRequest,
    ) -> Result<ScheduledAgentTask, TianyanError> {
        let old = self.tasks.read().await.get(id).cloned().ok_or_else(|| {
            TianyanError::not_found(format!("server: scheduled_tasks: 定时任务不存在：{id}"))
        })?;

        let now = chrono::Utc::now().timestamp();
        // 字段语义（校验 / 双写 / 重算 next_run）在纯函数里，单独可测
        let task = apply_update(&old, req, now)?;
        // 调度规格是否变了：决定重新注册时要不要带上「当前时刻点已结算」
        let schedule_changed = task.effective_schedule() != old.effective_schedule();

        self.tasks
            .write()
            .await
            .insert(task.id.clone(), task.clone());
        self.save().await;

        // 先注销旧注册（其 handler 持旧 workspace/prompt 的克隆），再按 enabled 重注册
        self.unregister(id).await;
        if task.enabled {
            let settled = if schedule_changed {
                None // 新规格从零判定（该补跑就补跑）
            } else {
                task.effective_schedule().most_recent_occurrence(now)
            };
            self.register(&task, settled).await;
        }

        tracing::info!(
            id = %task.id,
            name = %task.name,
            schedule = %task.effective_schedule().describe(),
            missed = %task.effective_missed().describe(),
            enabled = task.enabled,
            schedule_changed,
            "已更新定时智能体任务",
        );
        Ok(task)
    }

    /// 删除定时任务（注销 + 移除持久化）。
    pub async fn delete(&self, id: &str) -> bool {
        self.unregister(id).await;
        let removed = self.tasks.write().await.remove(id).is_some();
        if removed {
            self.save().await;
        }
        removed
    }

    /// 列出全部定时任务（按创建时间升序）。
    pub async fn list(&self) -> Vec<ScheduledAgentTask> {
        let tasks = self.tasks.read().await;
        let mut v: Vec<ScheduledAgentTask> = tasks.values().cloned().collect();
        v.sort_by_key(|a| a.created_at);
        v
    }

    /// 按「id 或名称」定位任务（`update_scheduled_task` 工具用：智能体手里通常只有名称）。
    ///
    /// 规则：先精确匹配 id；再按名称匹配（忽略首尾空白与大小写）。**多义即报错**——
    /// 猜一条去改，比让模型再问一句的代价大得多。
    pub async fn resolve_key(&self, key: &str) -> Result<String, TianyanError> {
        let key = key.trim();
        let tasks = self.list().await;
        if tasks.iter().any(|t| t.id == key) {
            return Ok(key.to_string());
        }
        let lowered = key.to_lowercase();
        let hits: Vec<&ScheduledAgentTask> = tasks
            .iter()
            .filter(|t| t.name.trim().to_lowercase() == lowered)
            .collect();
        match hits.len() {
            1 => Ok(hits[0].id.clone()),
            0 => Err(TianyanError::not_found(format!(
                "server: scheduled_tasks: 找不到定时任务「{key}」；现有任务：{}",
                describe_all(&tasks)
            ))),
            _ => Err(TianyanError::invalid_input(format!(
                "server: scheduled_tasks: 有 {} 个任务都叫「{key}」，请改用 id 指定：{}",
                hits.len(),
                hits.iter()
                    .map(|t| format!("{}（id={}）", t.name, t.id))
                    .collect::<Vec<_>>()
                    .join("；")
            ))),
        }
    }

    /// 记录一次执行结果（handler 经接口回写；更新 last_run_at / last_result / next_run_at）。
    pub async fn record_result(&self, id: &str, result: &str) {
        let now = chrono::Utc::now().timestamp();
        {
            let mut tasks = self.tasks.write().await;
            if let Some(t) = tasks.get_mut(id) {
                t.last_run_at = Some(now);
                t.last_result = Some(result.to_string());
                // 下次时间按**调度规格**算：日历类任务不是「现在 + 间隔」
                t.next_run_at = t.estimated_next_run(now);
                // 一次性任务：跑完即停用（不再注册、不再补跑）
                if matches!(
                    t.effective_schedule(),
                    tianyan::scheduler::Schedule::Once { .. }
                ) {
                    t.enabled = false;
                    t.next_run_at = None;
                }
            }
        }
        self.save().await;
    }

    /// 结果回写接口（handler 用；桥实持 Weak 打破循环）。
    fn result_sink(self: &Arc<Self>) -> Arc<dyn TaskResultSink> {
        Arc::new(ResultSinkBridge::new(self))
    }

    /// 将单个任务注册进调度器（构建 handler，经 registrar 接口注册）。
    ///
    /// `settled_fire` 见 [`TaskRegistrar::register`]（编辑后重新注册时保住游标）。
    async fn register(self: &Arc<Self>, task: &ScheduledAgentTask, settled_fire: Option<i64>) {
        let handler = Arc::new(ScheduledAgentTaskHandler::new(
            self.agent_lock.clone(),
            self.session_manager.clone(),
            self.working_sets.clone(),
            task.id.clone(),
            task.workspace.clone(),
            task.prompt.clone(),
            self.result_sink(),
        ));
        if let Err(e) = self.registrar.register(task, handler, settled_fire).await {
            tracing::warn!(error = %e, task = %task.id, "定时智能体任务注册失败");
        }
    }

    /// 从调度器注销任务（经 registrar 接口）。
    async fn unregister(&self, id: &str) {
        self.registrar.unregister(id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T0-11（主回归）：执行失败必须映射为 `TaskResult::failed`。
    ///
    /// 判别力：旧实现 `run_agent` 把失败也塞进 `Some(...)`，`execute()` 的
    /// `result.is_some()` 恒真 → 失败分支不可达，调度器统计永远成功。
    #[test]
    fn test_failed_run_maps_to_failed_task_result() {
        let (text, result) = map_run_outcome(Err("模型调用失败：connect timeout".to_string()));
        assert!(!result.success, "失败执行必须映射为 failed（T0-11）");
        assert_eq!(result.processed_count, 0);
        let err = result.error.expect("failed 必须携带错误").to_string();
        assert!(
            err.contains("模型调用失败"),
            "错误原因必须透传，实际: {err}"
        );
        assert!(
            text.contains("执行失败"),
            "回写文本应可见失败，实际: {text}"
        );
    }

    /// 成功路径不受影响（摘要 + 计数 1）。
    #[test]
    fn test_success_run_maps_to_success_task_result() {
        let (text, result) = map_run_outcome(Ok("任务完成摘要".to_string()));
        assert!(result.success);
        assert_eq!(result.processed_count, 1);
        assert_eq!(text, "任务完成摘要");
        assert!(result.error.is_none());
    }

    /// 会话 ID 生成幂等：task_id 已带 `sched-task-` 前缀时不再叠加
    /// （历史 bug：`sched-task-sched-task-…` 双前缀）。
    #[test]
    fn test_session_id_for_is_idempotent() {
        assert_eq!(
            ScheduledAgentTaskHandler::session_id_for("sched-task-abc"),
            "sched-task-abc"
        );
        assert_eq!(
            ScheduledAgentTaskHandler::session_id_for("abc"),
            "sched-task-abc"
        );
    }

    /// 编辑测试样本：间隔制任务，已跑过一次（有历史）。
    fn sched_sample() -> ScheduledAgentTask {
        let mut t = ScheduledAgentTask::new(
            "收盘复盘".to_string(),
            1800,
            "F:\\trade".to_string(),
            "跑收盘管线".to_string(),
        );
        t.last_run_at = Some(1_700_000_000);
        t.last_result = Some("上次结果".to_string());
        t.next_run_at = Some(1_700_001_800);
        t
    }

    /// 只改指令：调度规格与历史都不动（改 prompt 不该改变「何时跑」）。
    #[test]
    fn update_only_prompt_keeps_schedule_and_history() {
        let old = sched_sample();
        let req = UpdateScheduledTaskRequest {
            prompt: Some("换成新的指令".to_string()),
            ..Default::default()
        };
        let now = 1_800_000_000;
        let task = apply_update(&old, &req, now).expect("只改指令应成功");
        assert_eq!(task.prompt, "换成新的指令");
        assert_eq!(task.name, old.name, "没传的字段保持原值");
        assert_eq!(task.workspace, old.workspace);
        assert_eq!(task.effective_schedule(), old.effective_schedule());
        assert_eq!(task.interval_secs, old.interval_secs);
        assert_eq!(task.last_run_at, old.last_run_at, "历史保留");
        assert_eq!(task.last_result, old.last_result);
        assert_eq!(task.next_run_at, Some(now + 1800), "next_run 按规格重算");
    }

    /// 改调度规格：日历类要**双写** `interval_secs`（旧版本读新文件仍能跑）。
    #[test]
    fn update_schedule_dual_writes_interval() {
        let old = sched_sample();
        let req = UpdateScheduledTaskRequest {
            schedule: Some(tianyan::scheduler::Schedule::Daily { at: "15:30".into() }),
            ..Default::default()
        };
        let task = apply_update(&old, &req, 1_800_000_000).expect("改调度应成功");
        assert_eq!(
            task.interval_secs,
            24 * 3600,
            "落盘双写等价间隔（旧版本可读）"
        );
        assert_eq!(
            task.effective_schedule(),
            tianyan::scheduler::Schedule::Daily { at: "15:30".into() }
        );
    }

    /// 只给 `interval_secs`（旧客户端）→ 等价于间隔制。
    #[test]
    fn update_interval_secs_means_every() {
        let old = sched_sample();
        let req = UpdateScheduledTaskRequest {
            interval_secs: Some(3600),
            ..Default::default()
        };
        let task = apply_update(&old, &req, 1_800_000_000).expect("改间隔应成功");
        assert_eq!(
            task.effective_schedule(),
            tianyan::scheduler::Schedule::Every { secs: 3600 }
        );
        assert_eq!(task.interval_secs, 3600);
    }

    /// 停用：清 `next_run_at`（列表显示「待定」），定义与历史保留。
    #[test]
    fn update_disable_clears_next_run_but_keeps_definition() {
        let old = sched_sample();
        let req = UpdateScheduledTaskRequest {
            enabled: Some(false),
            ..Default::default()
        };
        let task = apply_update(&old, &req, 1_800_000_000).expect("停用应成功");
        assert!(!task.enabled);
        assert!(task.next_run_at.is_none(), "停用后不该再显示下次时间");
        assert_eq!(task.prompt, old.prompt, "停用不动定义");
        assert_eq!(task.last_run_at, old.last_run_at);
    }

    /// 非法输入：空名称 / 空指令 / 间隔过短 → `invalid_input`（handler 映射 400）。
    #[test]
    fn update_rejects_invalid_inputs() {
        let old = sched_sample();
        let now = 1_800_000_000;
        for req in [
            UpdateScheduledTaskRequest {
                name: Some("   ".into()),
                ..Default::default()
            },
            UpdateScheduledTaskRequest {
                prompt: Some(String::new()),
                ..Default::default()
            },
            UpdateScheduledTaskRequest {
                interval_secs: Some(10),
                ..Default::default()
            },
            UpdateScheduledTaskRequest {
                schedule: Some(tianyan::scheduler::Schedule::Every { secs: 59 }),
                ..Default::default()
            },
        ] {
            let err = apply_update(&old, &req, now).expect_err("非法输入必须报错");
            assert!(
                err.is_invalid_input(),
                "应判定为 invalid_input，实际：{err}"
            );
        }
    }

    /// 工作区允许空串（= 默认目录）：这是合法值，不是「未提供」。
    #[test]
    fn update_accepts_empty_workspace() {
        let old = sched_sample();
        let req = UpdateScheduledTaskRequest {
            workspace: Some(String::new()),
            ..Default::default()
        };
        let task = apply_update(&old, &req, 1_800_000_000).expect("空工作区应合法");
        assert!(task.workspace.is_empty());
    }

    /// 「按 id 或名称定位」的**歧义**与**候选提示**（工具侧 `task` 参数靠它）。
    ///
    /// 纯函数层面测 `describe_all`：定位失败时错误必须带上现有任务清单——模型据此
    /// 重试，而不是收到一句「找不到」就再无抓手。
    #[test]
    fn describe_all_lists_tasks_with_id_and_schedule() {
        let a = sched_sample();
        let mut b = sched_sample();
        b.id = "sched-task-b".to_string();
        b.name = "早盘巡检".to_string();
        b.enabled = false;
        let text = describe_all(&[a.clone(), b]);
        assert!(text.contains(&a.name), "必须列出名称：{text}");
        assert!(text.contains(&a.id), "必须列出 id（重试要用它）：{text}");
        assert!(text.contains("每 1800 秒"), "必须给出当前调度：{text}");
        assert!(text.contains("已停用"), "停用状态要标出来：{text}");
        assert_eq!(
            describe_all(&[]),
            "（当前没有任何定时任务）",
            "空清单也要有可读提示"
        );
    }
}
