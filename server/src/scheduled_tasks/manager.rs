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
use tianyan::session::SessionManager;
use tianyan::TianyanError;
use tokio::sync::RwLock;

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
    /// 注册任务到调度器（handler 已构建；注册即起 cron 循环）。
    async fn register(
        &self,
        task: &ScheduledAgentTask,
        handler: Arc<dyn TaskHandler>,
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
        );
        sched
            .register_dynamic_task(definition, ctx)
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
        task_id: String,
        workspace: String,
        prompt: String,
        result_sink: Arc<dyn TaskResultSink>,
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
            task_id,
            workspace,
            prompt,
            result_sink,
        }
    }

    /// 确保该任务有绑定工作区的专属会话（首次创建，之后复用）。
    async fn ensure_session(&self) {
        let sid = format!("sched-task-{}", self.task_id);
        if self
            .session_manager
            .get_session(&sid)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        if let Ok(mut session) = self
            .session_manager
            .create_session(&sid, Message::user(&self.prompt))
            .await
        {
            if Path::new(&self.workspace).is_dir() {
                session.header.working_directory = Some(self.workspace.clone());
                let _ = self.session_manager.update_session(&session).await;
            }
            // 清空 create_session 预写消息（对齐 resolve_or_create_session）：
            // 会话历史统一由 agent.process_message 追加，避免同一条用户消息重复
            session.messages.clear();
            let _ = self.session_manager.rewrite_messages(&sid, &[]).await;
        }
    }

    /// 调用 agent 执行任务 prompt（绑定工作区），返回结果摘要。
    async fn run_agent(&self) -> Option<String> {
        self.ensure_session().await;
        let sid = format!("sched-task-{}", self.task_id);
        let msg = Message::user(&self.prompt);
        let agent = self.agent_lock.read().await.clone();
        match agent.process_message(&sid, &msg, None, None).await {
            Ok(resp) => {
                let content = resp.content.trim();
                if content.is_empty() {
                    Some("(空响应)".to_string())
                } else {
                    Some(content.chars().take(500).collect())
                }
            }
            Err(e) => Some(format!("执行失败: {e}")),
        }
    }
}

#[async_trait::async_trait]
impl TaskHandler for ScheduledAgentTaskHandler {
    async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
        let result = self.run_agent().await;
        let text = result.clone().unwrap_or_else(|| "(无结果)".to_string());
        // 结果经接口回写（manager 已销毁时 upgrade 失败跳过——应用关停中）
        self.result_sink.record_result(&self.task_id, &text).await;
        if result.is_some() {
            TaskResult::success(1)
        } else {
            TaskResult::failed(TianyanError::Custom("定时智能体任务执行失败".to_string()))
        }
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
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
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
                self.register(task).await;
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
        task.next_run_at = Some(chrono::Utc::now().timestamp() + task.interval_secs as i64);
        self.tasks
            .write()
            .await
            .insert(task.id.clone(), task.clone());
        self.save().await;
        if task.enabled {
            self.register(&task).await;
        }
        tracing::info!(
            name = %task.name,
            interval_secs = task.interval_secs,
            workspace = %task.workspace,
            "已创建定时智能体任务",
        );
        task
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

    /// 记录一次执行结果（handler 经接口回写；更新 last_run_at / last_result / next_run_at）。
    pub async fn record_result(&self, id: &str, result: &str) {
        let now = chrono::Utc::now().timestamp();
        {
            let mut tasks = self.tasks.write().await;
            if let Some(t) = tasks.get_mut(id) {
                t.last_run_at = Some(now);
                t.last_result = Some(result.to_string());
                t.next_run_at = Some(now + t.interval_secs as i64);
            }
        }
        self.save().await;
    }

    /// 结果回写接口（handler 用；桥实持 Weak 打破循环）。
    fn result_sink(self: &Arc<Self>) -> Arc<dyn TaskResultSink> {
        Arc::new(ResultSinkBridge::new(self))
    }

    /// 将单个任务注册进调度器（构建 handler，经 registrar 接口注册）。
    async fn register(self: &Arc<Self>, task: &ScheduledAgentTask) {
        let handler = Arc::new(ScheduledAgentTaskHandler::new(
            self.agent_lock.clone(),
            self.session_manager.clone(),
            task.id.clone(),
            task.workspace.clone(),
            task.prompt.clone(),
            self.result_sink(),
        ));
        if let Err(e) = self.registrar.register(task, handler).await {
            tracing::warn!(error = %e, task = %task.id, "定时智能体任务注册失败");
        }
    }

    /// 从调度器注销任务（经 registrar 接口）。
    async fn unregister(&self, id: &str) {
        self.registrar.unregister(id).await;
    }
}
