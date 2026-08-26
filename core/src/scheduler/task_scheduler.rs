//! 统一任务调度器。
//!
//! 本模块提供基于 cron 表达式的任务调度功能，管理所有后台任务。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cron::Schedule;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::common::error::TianyanError;
use crate::config::TianyanConfig;
use crate::memory::MemoryExtractor;
use crate::scheduler::TaskStateStore;
use crate::skills::SkillReviewer;
use crate::vfs::{SummaryService, VirtualFileSystem};

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
}

impl TaskResult {
    /// 创建成功结果。
    pub fn success(processed_count: usize) -> Self {
        Self {
            success: true,
            processed_count,
            error: None,
        }
    }

    /// 创建失败结果。
    pub fn failed(error: impl Into<TianyanError>) -> Self {
        Self {
            success: false,
            processed_count: 0,
            error: Some(error.into()),
        }
    }
}

/// 任务上下文。
///
/// 提供给任务执行时访问核心服务。
#[derive(Clone)]
pub struct TaskContext {
    /// 虚拟文件系统。
    pub vfs: Arc<dyn VirtualFileSystem>,
    /// 摘要服务（`Arc<dyn SummaryService>`——测试可注入 MockSummaryEngine）。
    pub summary_engine: Arc<dyn SummaryService>,
    /// 记忆提取器。
    pub memory_extractor: Arc<MemoryExtractor>,
    /// 技能使用评审器（记忆提取同周期顺路复审技能使用效果）。
    pub skill_reviewer: Arc<SkillReviewer>,
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
        memory_extractor: Arc<MemoryExtractor>,
        skill_reviewer: Arc<SkillReviewer>,
        config: Arc<TianyanConfig>,
    ) -> Self {
        let task_state = Arc::new(TaskStateStore::new(vfs.clone()));
        Self {
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
            task_state,
        }
    }
}

/// 任务处理 trait。
///
/// 所有后台任务必须实现此 trait。
#[async_trait]
pub trait TaskHandler: Send + Sync {
    /// 执行任务。
    ///
    /// # 参数
    /// - `ctx`: 任务上下文
    ///
    /// # 返回
    /// 任务执行结果
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
    /// Cron 表达式。
    pub cron_expression: String,
    /// 任务处理器。
    pub handler: Arc<dyn TaskHandler>,
}

impl TaskDefinition {
    /// 创建新的任务定义。
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        cron_expression: impl Into<String>,
        handler: Arc<dyn TaskHandler>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            priority: TaskPriority::Normal,
            cron_expression: cron_expression.into(),
            handler,
        }
    }
}

/// 已注册任务的信息。
struct RegisteredTask {
    /// 任务定义。
    definition: TaskDefinition,
    /// 上次执行时间。
    last_run: Option<std::time::Instant>,
    /// 执行次数。
    run_count: u64,
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
    /// Cron 表达式。
    pub cron_expression: String,
    /// 执行次数。
    pub run_count: u64,
    /// 距上次执行已过秒数（从未执行时为 None）。
    pub last_run_ago_secs: Option<u64>,
}

/// 统一任务调度器。
///
/// 自研轻量实现（非 tokio-cron-scheduler 依赖）：每个任务一个 tokio
/// 循环，按完整 cron 表达式（`cron` crate 解析）计算下次触发时刻。
/// 支持启动时注册与运行期动态注册（`register_dynamic_task`）。
pub struct TaskScheduler {
    /// 已注册的任务。
    tasks: RwLock<HashMap<String, RegisteredTask>>,
    /// 运行状态。
    running: std::sync::atomic::AtomicBool,
    /// 任务执行句柄（task_id → 循环句柄；用于动态注销时单独取消）。
    handles: RwLock<HashMap<String, tokio::task::JoinHandle<()>>>,
}

/// 计算 cron 在 after_epoch 之后的下次触发时刻（epoch 秒；供显示/API 使用）。
pub fn next_run_at(cron: &str, after_epoch: i64) -> Option<i64> {
    let schedule = Schedule::from_str(cron).ok()?;
    let after = Utc.timestamp_opt(after_epoch, 0).single()?;
    let next = schedule.after(&after).next()?;
    Some(next.timestamp())
}

/// 计算从当前时刻到 cron 下次触发的时间间隔（秒）。
fn next_delay_secs(cron: &str) -> Option<u64> {
    let schedule = Schedule::from_str(cron).ok()?;
    let now = Utc::now();
    let next = schedule.after(&now).next()?;
    let secs = (next - now).num_seconds();
    Some(secs.max(1) as u64)
}

/// 单个任务的 cron 循环：计算下次触发时刻 → sleep 到点 → 执行 → 重复。
async fn run_task_loop(sched: Arc<TaskScheduler>, ctx: Arc<TaskContext>, task_id: String) {
    loop {
        let delay = {
            let tasks = sched.tasks.read().await;
            let cron = match tasks.get(&task_id) {
                Some(t) => &t.definition.cron_expression,
                None => return, // 任务已注销
            };
            match next_delay_secs(cron) {
                Some(d) => d,
                None => {
                    tracing::warn!(task = %task_id, cron = %cron, "cron parse failed, stopping task");
                    return;
                }
            }
        };
        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
        if !sched.is_running() {
            break;
        }
        if sched.execute_task(&task_id, &ctx).await.is_none() {
            break; // 任务已注销
        }
    }
}

/// 为单个任务启动 cron 循环（startup 与运行期动态注册共用）。
async fn spawn_task_loop(scheduler: Arc<TaskScheduler>, ctx: Arc<TaskContext>, task_id: String) {
    let handle = tokio::spawn(run_task_loop(scheduler.clone(), ctx, task_id.clone()));
    scheduler
        .handles
        .write()
        .await
        .insert(task_id.clone(), handle);
    tracing::info!("启动任务调度：{}", task_id);
}

impl TaskScheduler {
    /// 创建新的任务调度器。
    ///
    /// # 返回
    /// 新的调度器实例
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            running: std::sync::atomic::AtomicBool::new(false),
            handles: RwLock::new(HashMap::new()),
        }
    }

    /// 启动 cron 调度的任务。
    ///
    /// # 参数
    /// - `scheduler`: 调度器 Arc 引用
    /// - `ctx`: 任务上下文
    ///
    /// # 返回
    /// - `Ok(())`: 启动成功
    /// - `Err`: 启动失败
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

        tracing::info!("任务调度器启动中...");

        let task_ids = scheduler.get_task_ids().await;

        for task_id in task_ids {
            spawn_task_loop(scheduler.clone(), ctx.clone(), task_id).await;
        }

        tracing::info!("任务调度器已启动");
        Ok(())
    }

    /// 停止所有任务。
    pub async fn shutdown(&self) -> crate::common::error::Result<()> {
        self.stop().await?;

        // 取消所有任务句柄
        let mut handles = self.handles.write().await;
        for handle in handles.values() {
            handle.abort();
        }
        handles.clear();

        tracing::info!("任务调度器已关闭");
        Ok(())
    }

    /// 注册任务。
    ///
    /// # 参数
    /// - `definition`: 任务定义
    ///
    /// # 返回
    /// - `Ok(())`: 注册成功
    /// - `Err`: 注册失败（如 ID 已存在）
    pub async fn register_task(
        &self,
        definition: TaskDefinition,
    ) -> crate::common::error::Result<()> {
        let mut tasks = self.tasks.write().await;

        if tasks.contains_key(&definition.id) {
            return Err(TianyanError::Custom(format!(
                "内部错误：任务 ID 已存在：{}",
                definition.id
            )));
        }

        tracing::info!(
            "注册任务：{} ({})，Cron：{}",
            definition.name,
            definition.id,
            definition.cron_expression
        );

        tasks.insert(
            definition.id.clone(),
            RegisteredTask {
                definition,
                last_run: None,
                run_count: 0,
            },
        );

        Ok(())
    }

    /// 运行期动态注册任务：注册即起 cron 循环（用于用户动态创建的任务）。
    pub async fn register_dynamic_task(
        self: &Arc<Self>,
        definition: TaskDefinition,
        ctx: Arc<TaskContext>,
    ) -> crate::common::error::Result<()> {
        let id = definition.id.clone();
        self.register_task(definition).await?;
        if self.is_running() {
            spawn_task_loop(self.clone(), ctx, id).await;
        }
        Ok(())
    }

    /// 注销任务：移除定义并取消其循环。
    pub async fn unregister_task(&self, task_id: &str) -> bool {
        let removed = self.tasks.write().await.remove(task_id).is_some();
        if removed {
            if let Some(handle) = self.handles.write().await.remove(task_id) {
                handle.abort();
            }
        }
        removed
    }

    /// 停止调度器。
    ///
    /// 停止所有任务的执行。
    ///
    /// # 返回
    /// - `Ok(())`: 停止成功
    /// - `Err`: 停止失败
    pub async fn stop(&self) -> crate::common::error::Result<()> {
        if !self
            .running
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }

        tracing::info!("任务调度器停止中...");
        tracing::info!("任务调度器已停止");
        Ok(())
    }

    /// 检查调度器是否正在运行。
    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取所有已注册的任务 ID。
    pub async fn get_task_ids(&self) -> Vec<String> {
        let tasks = self.tasks.read().await;
        tasks.keys().cloned().collect()
    }

    /// 获取任务信息。
    pub async fn get_task_info(&self, task_id: &str) -> Option<(String, String, u64)> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|t| {
            (
                t.definition.name.clone(),
                t.definition.cron_expression.clone(),
                t.run_count,
            )
        })
    }

    /// 获取全部任务的运行状态快照。
    ///
    /// # 返回
    /// 按任务 ID 排序的任务状态列表（含执行统计与上次执行时间）。
    pub async fn snapshot(&self) -> Vec<TaskStatus> {
        let tasks = self.tasks.read().await;
        let mut statuses: Vec<TaskStatus> = tasks
            .iter()
            .map(|(id, t)| TaskStatus {
                id: id.clone(),
                name: t.definition.name.clone(),
                priority: t.definition.priority,
                cron_expression: t.definition.cron_expression.clone(),
                run_count: t.run_count,
                last_run_ago_secs: t.last_run.map(|i| i.elapsed().as_secs()),
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
        // 不得阻塞 snapshot()/get_task_ids() 等读路径（修复：状态查询被任务卡死）。
        let (name, handler) = {
            let mut tasks = self.tasks.write().await;
            let task = tasks.get_mut(task_id)?;
            (
                task.definition.name.clone(),
                task.definition.handler.clone(),
            )
        };

        tracing::debug!("执行任务：{} ({})", name, task_id);

        let result = handler.execute(ctx).await;

        // 执行完成后补记运行统计（重新获取写锁，短临界区）
        {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(task_id) {
                task.last_run = Some(std::time::Instant::now());
                task.run_count += 1;
            }
        }

        if result.success {
            tracing::info!(
                "任务执行成功：{}，处理了 {} 个条目",
                task_id,
                result.processed_count
            );
        } else if let Some(ref error) = result.error {
            tracing::error!("任务执行失败：{} - {}", task_id, error);
        }

        Some(result)
    }
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_next_run_at_interval_minutes() {
        // 真 cron 对齐：00:00 之后下一个 */30 分点是 00:30
        assert_eq!(next_run_at("0 */30 * * * *", 0), Some(1800));
        // 00:01:01 之后下一个每 5 分钟对齐点（0,5,10...）是 00:05:00
        assert_eq!(next_run_at("0 */5 * * * *", 61), Some(300));
    }

    #[test]
    fn test_next_run_at_daily() {
        let ts = |y, mo, d, h, mi| {
            Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap()
                .timestamp()
        };
        assert_eq!(
            next_run_at("0 30 9 * * *", ts(2026, 8, 25, 9, 29)),
            Some(ts(2026, 8, 25, 9, 30))
        );
        assert_eq!(
            next_run_at("0 30 9 * * *", ts(2026, 8, 25, 9, 31)),
            Some(ts(2026, 8, 26, 9, 30))
        );
    }

    struct TestTask;

    #[async_trait]
    impl TaskHandler for TestTask {
        async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
            TaskResult::success(1)
        }

        fn name(&self) -> &str {
            "test_task"
        }
    }

    #[tokio::test]
    async fn test_task_scheduler_new() {
        let scheduler = TaskScheduler::new();
        assert!(!scheduler.is_running());
    }

    #[tokio::test]
    async fn test_task_scheduler_stop_idempotent() {
        let scheduler = TaskScheduler::new();
        assert!(!scheduler.is_running());
        // 未运行时 stop 幂等（running 标志仅在 start_with_scheduler 时置位）
        scheduler.stop().await.unwrap();
        assert!(!scheduler.is_running());
    }

    #[tokio::test]
    async fn test_register_task() {
        let scheduler = TaskScheduler::new();
        let task = TaskDefinition::new("test", "测试任务", "0 */5 * * * *", Arc::new(TestTask));

        scheduler.register_task(task).await.unwrap();

        let ids = scheduler.get_task_ids().await;
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "test");
    }

    #[tokio::test]
    async fn test_snapshot_reports_task_status() {
        let scheduler = TaskScheduler::new();
        scheduler
            .register_task(TaskDefinition::new(
                "b",
                "任务B",
                "0 */10 * * * *",
                Arc::new(TestTask),
            ))
            .await
            .unwrap();
        scheduler
            .register_task(TaskDefinition::new(
                "a",
                "任务A",
                "0 */5 * * * *",
                Arc::new(TestTask),
            ))
            .await
            .unwrap();

        // 未执行前：run_count=0、last_run=None；按 ID 排序输出
        let snapshot = scheduler.snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].id, "a");
        assert_eq!(snapshot[1].id, "b");
        assert_eq!(snapshot[0].name, "任务A");
        assert_eq!(snapshot[0].cron_expression, "0 */5 * * * *");
        assert_eq!(snapshot[0].priority, TaskPriority::Normal);
        assert_eq!(snapshot[0].run_count, 0);
        assert!(snapshot[0].last_run_ago_secs.is_none());

        // 模拟执行后的内部状态（execute_task 的 run_count/last_run 更新逻辑与此同源）
        {
            let mut tasks = scheduler.tasks.write().await;
            let t = tasks.get_mut("a").unwrap();
            t.run_count = 3;
            t.last_run = Some(std::time::Instant::now());
        }
        let snapshot = scheduler.snapshot().await;
        assert_eq!(snapshot[0].run_count, 3);
        assert!(snapshot[0].last_run_ago_secs.is_some());
    }

    #[tokio::test]
    async fn test_register_duplicate_task() {
        let scheduler = TaskScheduler::new();
        let task1 = TaskDefinition::new("test", "测试任务1", "0 */5 * * * *", Arc::new(TestTask));
        let task2 = TaskDefinition::new("test", "测试任务2", "0 */10 * * * *", Arc::new(TestTask));

        scheduler.register_task(task1).await.unwrap();
        let result = scheduler.register_task(task2).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_task_priority_ordering() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::High > TaskPriority::Normal);
        assert!(TaskPriority::Normal > TaskPriority::Low);
    }

    #[tokio::test]
    async fn test_task_result() {
        let success = TaskResult::success(5);
        assert!(success.success);
        assert_eq!(success.processed_count, 5);
        assert!(success.error.is_none());

        let failed = TaskResult::failed(TianyanError::Custom("scheduler：测试错误".to_string()));
        assert!(!failed.success);
        assert_eq!(failed.processed_count, 0);
        match &failed.error {
            Some(err) => assert_eq!(err.to_string(), "scheduler：测试错误"),
            None => panic!("失败结果应携带错误信息"),
        }
    }

    #[tokio::test]
    async fn test_task_status_json_contract() {
        // TaskStatus 序列化契约锁定：/api/v1/scheduler/status 输出的字段集合与形状
        // （id/name/priority/cron_expression/run_count/last_run_ago_secs），不包含 error 字段。
        let status = TaskStatus {
            id: "summary_generation".to_string(),
            name: "摘要生成".to_string(),
            priority: TaskPriority::Normal,
            cron_expression: "0 */5 * * * *".to_string(),
            run_count: 3,
            last_run_ago_secs: Some(42),
        };
        let expected = serde_json::json!({
            "id": "summary_generation",
            "name": "摘要生成",
            "priority": "Normal",
            "cron_expression": "0 */5 * * * *",
            "run_count": 3,
            "last_run_ago_secs": 42,
        });
        assert_eq!(serde_json::to_value(&status).unwrap(), expected);
        assert!(serde_json::to_value(&status)
            .unwrap()
            .get("error")
            .is_none());
    }
}
