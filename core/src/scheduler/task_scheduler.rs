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

use crate::common::error::TianyanError;
use crate::config::TianyanConfig;
use crate::memory::MemoryExtractor;
use crate::scheduler::TaskStateStore;
use crate::skills::SkillReviewer;
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
    /// 执行间隔（秒）：距上次执行完成 ≥ 此值即到期，下个扫描周期立即执行。
    pub interval_secs: u64,
    /// 任务处理器。
    pub handler: Arc<dyn TaskHandler>,
}

impl TaskDefinition {
    /// 创建新的任务定义。
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
            interval_secs,
            handler,
        }
    }
}

/// 已注册任务的信息（注册顺序 = 扫描顺序）。
struct RegisteredTask {
    definition: TaskDefinition,
    /// 上次执行完成时刻（epoch 秒；None = 从未执行，首个扫描周期即执行）。
    last_run: Option<i64>,
    /// 执行次数。
    run_count: u64,
    /// 最近一次执行的失败信息（None = 成功或从未执行）。
    last_error: Option<String>,
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
    /// 执行间隔（秒）。
    pub interval_secs: u64,
    /// 执行次数。
    pub run_count: u64,
    /// 距上次执行已过秒数（从未执行时为 None）。
    pub last_run_ago_secs: Option<u64>,
    /// 距下次到期秒数（已到期 = 0；从未执行 = 0，首个扫描周期即执行）。
    pub next_due_in_secs: u64,
    /// 最近一次执行的失败信息（None = 成功或从未执行）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// 持久化状态文件的单任务条目。
#[derive(Debug, Serialize, Deserialize)]
struct PersistedTaskState {
    last_run: i64,
    run_count: u64,
    /// 最近一次失败信息（旧状态文件无此字段，缺省 None）。
    #[serde(default)]
    last_error: Option<String>,
}

/// 扫描间隔默认值（秒）。
pub const DEFAULT_TICK_SECS: u64 = 60;

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
    /// 当前正在执行的任务 ID（顺序执行，至多一个；None = 空闲）。
    executing: RwLock<Option<String>>,
}

/// 计算下次到期时刻（epoch 秒）：last_run + interval；从未执行 = 立即到期。
pub fn next_due_at(last_run: Option<i64>, interval_secs: u64, now: i64) -> i64 {
    let due = last_run.map(|t| t + interval_secs as i64).unwrap_or(now);
    due.max(now)
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
            executing: RwLock::new(None),
        }
    }

    /// 设置扫描间隔（秒）。
    pub fn with_tick_secs(mut self, tick_secs: u64) -> Self {
        self.tick_secs = tick_secs.max(1);
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
            tracing::warn!(path = %path.display(), "调度器状态文件损坏（忽略，任务按从未执行处理）");
            return;
        };
        let mut tasks = self.tasks.write().await;
        for t in tasks.iter_mut() {
            if let Some(s) = map.get(&t.definition.id) {
                t.last_run = Some(s.last_run);
                t.run_count = s.run_count;
                t.last_error = s.last_error.clone();
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
                    t.last_run.map(|lr| {
                        (
                            t.definition.id.clone(),
                            PersistedTaskState {
                                last_run: lr,
                                run_count: t.run_count,
                                last_error: t.last_error.clone(),
                            },
                        )
                    })
                })
                .collect()
        };
        match serde_json::to_string(&map) {
            Ok(json) => {
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = tokio::fs::write(path, json).await {
                    tracing::error!(error = %e, path = %path.display(), "调度器状态持久化失败");
                }
            }
            Err(e) => tracing::error!(error = %e, "调度器状态序列化失败"),
        }
    }

    /// 当前到期的任务 ID（按注册顺序；顺序扫描 = 串行执行）。
    async fn due_task_ids(&self) -> Vec<String> {
        let now = Utc::now().timestamp();
        self.tasks
            .read()
            .await
            .iter()
            .filter(|t| {
                t.last_run
                    .map(|lr| now - lr >= t.definition.interval_secs as i64)
                    .unwrap_or(true) // 从未执行 = 到期（首跑/补跑）
            })
            .map(|t| t.definition.id.clone())
            .collect()
    }

    /// 单一扫描循环：顺序检查 → 到期即执行 → 持久化 → 睡到下个 tick。
    async fn run_scanner(scheduler: Arc<TaskScheduler>, ctx: Arc<TaskContext>) {
        loop {
            if !scheduler.is_running() {
                break;
            }
            let due = scheduler.due_task_ids().await;
            for id in due {
                if !scheduler.is_running() {
                    break;
                }
                // 串行执行：一个任务跑完才检查下一个（避免资源争抢）
                scheduler.execute_task(&id, &ctx).await;
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
        let mut tasks = self.tasks.write().await;
        if tasks.iter().any(|t| t.definition.id == definition.id) {
            return Err(TianyanError::Custom(format!(
                "内部错误：任务 ID 已存在：{}",
                definition.id
            )));
        }

        tracing::info!(
            "注册任务：{} ({})，间隔：{}s",
            definition.name,
            definition.id,
            definition.interval_secs
        );

        tasks.push(RegisteredTask {
            definition,
            last_run: None,
            run_count: 0,
            last_error: None,
        });
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
                let next_due_in_secs = match t.last_run {
                    Some(lr) => {
                        let due = lr + t.definition.interval_secs as i64;
                        (due - now).max(0) as u64
                    }
                    None => 0, // 从未执行 = 立即到期
                };
                TaskStatus {
                    id: t.definition.id.clone(),
                    name: t.definition.name.clone(),
                    priority: t.definition.priority,
                    interval_secs: t.definition.interval_secs,
                    run_count: t.run_count,
                    last_run_ago_secs,
                    next_due_in_secs,
                    last_error: t.last_error.clone(),
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
        let result = handler.execute(ctx).await;
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

    use crate::memory::ExtractionConfig;
    use crate::model::ChatService;
    use crate::test_utils::{MockChatService, MockVfs};
    use crate::vfs::SummaryEngine;

    /// 构造调度器测试上下文（TestTask 不消费字段，Mock 服务足够）。
    fn make_context() -> TaskContext {
        let vfs: Arc<MockVfs> = Arc::new(MockVfs::new());
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let summary_engine = Arc::new(SummaryEngine::new(chat.clone(), "test-model"));
        let memory_extractor = Arc::new(MemoryExtractor::new(
            chat.clone(),
            ExtractionConfig::default(),
        ));
        let skill_reviewer = Arc::new(SkillReviewer::new(
            chat,
            vfs.clone(),
            "test-model".to_string(),
        ));
        let config = Arc::new(TianyanConfig::default());
        TaskContext::new(
            vfs,
            summary_engine,
            memory_extractor,
            skill_reviewer,
            config,
        )
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
        assert_eq!(scheduler.due_task_ids().await, vec!["a".to_string()]);
        // 执行后 3600s 内不再到期
        let ctx = Arc::new(make_context());
        scheduler.execute_task("a", &ctx).await;
        assert!(scheduler.due_task_ids().await.is_empty());
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
