//! 定时智能体任务管理器。
//!
//! 任务定义持久化到 JSON；实际调度由核心 TaskScheduler 承担（完整 cron）——
//! 本管理器只负责：任务注册/注销进调度器、持久化、结果回写与列表展示。

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

/// 定时智能体任务处理器：到点由 TaskScheduler 调用，在绑定工作区执行 prompt。
pub struct ScheduledAgentTaskHandler {
    agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
    session_manager: Arc<dyn SessionManager>,
    task_id: String,
    workspace: String,
    prompt: String,
    manager: Arc<ScheduledAgentTaskManager>,
}

impl ScheduledAgentTaskHandler {
    fn new(
        agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
        session_manager: Arc<dyn SessionManager>,
        task_id: String,
        workspace: String,
        prompt: String,
        manager: Arc<ScheduledAgentTaskManager>,
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
            task_id,
            workspace,
            prompt,
            manager,
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
        self.manager.record_result(&self.task_id, &text).await;
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

/// 定时智能体任务管理器：持久化 + 注册进 TaskScheduler + 列表/结果。
pub struct ScheduledAgentTaskManager {
    agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
    session_manager: Arc<dyn SessionManager>,
    file_path: PathBuf,
    tasks: Arc<RwLock<HashMap<String, ScheduledAgentTask>>>,
    scheduler: Arc<RwLock<Option<Arc<TaskScheduler>>>>,
    task_ctx: Arc<RwLock<Option<Arc<TaskContext>>>>,
}

impl ScheduledAgentTaskManager {
    /// 创建管理器（data_dir 用于持久化任务定义；agent_lock 随热重载更新）。
    pub fn new(
        agent_lock: Arc<RwLock<Arc<dyn AgentCoordinator>>>,
        session_manager: Arc<dyn SessionManager>,
        data_dir: &Path,
    ) -> Self {
        Self {
            agent_lock,
            session_manager,
            file_path: data_dir.join("scheduled_agent_tasks.json"),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            scheduler: Arc::new(RwLock::new(None)),
            task_ctx: Arc::new(RwLock::new(None)),
        }
    }

    /// 绑定调度器与任务上下文（start_server 创建后调用；无 Provider 时为 None）。
    pub async fn bind_scheduler(
        &self,
        scheduler: Option<Arc<TaskScheduler>>,
        task_ctx: Option<Arc<TaskContext>>,
    ) {
        *self.scheduler.write().await = scheduler;
        *self.task_ctx.write().await = task_ctx;
    }

    /// 从磁盘加载任务定义。
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
        for t in list {
            tasks.insert(t.id.clone(), t);
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
            req.cron.clone(),
            req.workspace.clone(),
            req.prompt.clone(),
        );
        task.next_run_at =
            tianyan::scheduler::next_run_at(&task.cron, chrono::Utc::now().timestamp());
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
            cron = %task.cron,
            workspace = %task.workspace,
            "已创建定时智能体任务"
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

    /// 记录一次执行结果（handler 调用；更新 last_run_at / last_result / next_run_at）。
    pub async fn record_result(&self, id: &str, result: &str) {
        let now = chrono::Utc::now().timestamp();
        {
            let mut tasks = self.tasks.write().await;
            if let Some(t) = tasks.get_mut(id) {
                t.last_run_at = Some(now);
                t.last_result = Some(result.to_string());
                t.next_run_at = tianyan::scheduler::next_run_at(&t.cron, now);
            }
        }
        self.save().await;
    }

    /// 将单个任务注册进调度器（构建 handler，注册即起 cron 循环）。
    async fn register(self: &Arc<Self>, task: &ScheduledAgentTask) {
        let Some(sched) = self.scheduler.read().await.clone() else {
            return;
        };
        let Some(ctx) = self.task_ctx.read().await.clone() else {
            return;
        };
        let handler = Arc::new(ScheduledAgentTaskHandler::new(
            self.agent_lock.clone(),
            self.session_manager.clone(),
            task.id.clone(),
            task.workspace.clone(),
            task.prompt.clone(),
            self.clone(),
        ));
        let definition = TaskDefinition::new(
            task.id.clone(),
            task.name.clone(),
            task.cron.clone(),
            handler,
        );
        if let Err(e) = sched.register_dynamic_task(definition, ctx).await {
            tracing::warn!(error = %e, task = %task.id, "定时智能体任务注册失败");
        }
    }

    /// 从调度器注销任务。
    async fn unregister(&self, id: &str) {
        if let Some(sched) = self.scheduler.read().await.clone() {
            sched.unregister_task(id).await;
        }
    }
}
