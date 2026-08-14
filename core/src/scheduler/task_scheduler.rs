//! 统一任务调度器。
//!
//! 本模块提供基于 cron 表达式的任务调度功能，管理所有后台任务。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::common::error::TianyanError;
use crate::config::TianyanConfig;
use crate::memory::MemoryExtractor;
use crate::scheduler::TaskStateStore;
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
        config: Arc<TianyanConfig>,
    ) -> Self {
        let task_state = Arc::new(TaskStateStore::new(vfs.clone()));
        Self {
            vfs,
            summary_engine,
            memory_extractor,
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
/// 间隔循环；cron 表达式仅支持 `*/N` 间隔语义（详见 [`parse_cron_interval`]）。
pub struct TaskScheduler {
    /// 已注册的任务。
    tasks: RwLock<HashMap<String, RegisteredTask>>,
    /// 运行状态。
    running: std::sync::atomic::AtomicBool,
    /// 任务执行句柄（用于取消）。
    handles: RwLock<Vec<tokio::task::JoinHandle<()>>>,
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
            handles: RwLock::new(Vec::new()),
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
            let sched = scheduler.clone();
            let ctx = ctx.clone();
            let task_info = match scheduler.get_task_info(&task_id).await {
                Some(info) => info,
                None => {
                    tracing::warn!("任务 {} 信息不存在，可能已被注销，跳过", task_id);
                    continue;
                }
            };

            // 解析 cron 表达式获取间隔（简化实现，使用固定间隔）
            // 格式：秒 分钟 小时 日期 月份 星期

            // "0 */5 * * * *" = 每 5 分钟
            // "0 */10 * * * *" = 每 10 分钟
            let interval_secs = parse_cron_interval(&task_info.1);

            tracing::info!(
                "启动任务调度：{} ({}), 间隔：{} 秒",
                task_info.0,
                task_id,
                interval_secs
            );

            let handle = tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

                loop {
                    interval.tick().await;

                    if !sched.is_running() {
                        break;
                    }

                    match sched.execute_task(&task_id, &ctx).await {
                        None => tracing::warn!(task = %task_id, "定时任务未找到"),
                        Some(_result) => {}
                    }
                }
            });

            scheduler.handles.write().await.push(handle);
        }

        tracing::info!("任务调度器已启动");
        Ok(())
    }

    /// 停止所有任务。
    pub async fn shutdown(&self) -> crate::common::error::Result<()> {
        self.stop().await?;

        // 取消所有任务句柄
        let mut handles = self.handles.write().await;
        for handle in handles.drain(..) {
            handle.abort();
        }

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
        let mut tasks = self.tasks.write().await;

        let task = tasks.get_mut(task_id)?;
        let handler = task.definition.handler.clone();

        tracing::debug!("执行任务：{} ({})", task.definition.name, task_id);

        let result = handler.execute(ctx).await;

        task.last_run = Some(std::time::Instant::now());
        task.run_count += 1;

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

/// 解析 cron 表达式获取间隔秒数（自研简化实现，非完整 cron 语义）。
///
/// 支持 6 字段格式（秒 分钟 小时 日 月 星期），但仅 `*/N` 字段参与
/// 间隔计算：
///
/// - `"0 */5 * * * *"` = 每 5 分钟 = 300 秒
/// - `"0 */10 * * * *"` = 每 10 分钟 = 600 秒
///
/// 语义边界（有意收缩，勿按完整 cron 预期）：
/// - 固定值字段（如秒位 `0`/`30`）只表达相位，被忽略（首个 tick 不
///   对齐相位）；
/// - 日/月/星期字段不支持，恒被忽略；
/// - 无任何 `*/N` 字段或解析失败 → 回退默认 300 秒并告警。
fn parse_cron_interval(cron: &str) -> u64 {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() != 6 {
        tracing::warn!(
            cron = %cron,
            expected_parts = 6,
            actual_parts = parts.len(),
            "Cron 表达式格式不支持，回退到默认 300 秒间隔"
        );
        return 300;
    }

    // 6 字段格式: 秒 分钟 小时 日 月 星期
    let fields: [(u64, &str); 6] = [
        (1, parts[0]),     // 秒 → 秒
        (60, parts[1]),    // 分钟 → 秒
        (3600, parts[2]),  // 小时 → 秒
        (86400, parts[3]), // 日 → 秒
        (0, parts[4]),     // 月 (暂不支持)
        (0, parts[5]),     // 星期 (暂不支持)
    ];

    let mut interval: Option<u64> = None;

    for (multiplier, field) in &fields {
        if *multiplier == 0 {
            continue; // 跳过月/星期字段
        }
        if let Some(val_str) = field.strip_prefix("*/") {
            if let Ok(val) = val_str.parse::<u64>() {
                let secs = val * multiplier;
                interval = Some(match interval {
                    Some(existing) => existing.min(secs),
                    None => secs,
                });
            }
        } else if *field != "*" {
            // 固定值: 不作为间隔处理
            if let Ok(_val) = field.parse::<u64>() {
                continue;
            }
        }
    }

    match interval {
        Some(secs) if secs > 0 => secs,
        _ => {
            tracing::warn!(
                cron = %cron,
                "Cron 表达式解析失败，回退到默认 300 秒间隔"
            );
            300
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
