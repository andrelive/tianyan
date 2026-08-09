//! 后台任务支持（fire-and-forget 委托）。
//!
//! 业界对标（opencode task(background=true) / oh-my-openagent
//! BackgroundManager / Claude Code background subagents）：
//! - 任务注册表：id → 状态机（Pending/Running/Completed/Failed/Cancelled）
//! - 并发上限：信号量限制同时运行的后台委托数（防失控扇出）
//! - 完成通知：任务完成时通过 [`TaskNotifier`] 通知父会话（默认实现把
//!   通知作为 System 消息持久化到会话——主 LLM 下一轮自然看到并继续，
//!   对应"noReply 注入 + 轮次边界投递"模式）
//! - 唤醒语义（ADR-013）：全部完成或失败时通过 [`TaskWaker`] 触发主 agent
//!   新一轮生成（shouldReply = allComplete || failure，oh-my-openagent 实证）；
//!   部分完成时仅静默注入，主 agent 不醒来（运行时强制，非模型自律）
//! - join 信号：通知携带"该会话剩余任务数"，全部完成时明确提示汇总
//!   （代码层只做计数与批量提示，最终聚合由主 LLM 完成——opencode 共识）
//! - 任务持久化（ADR-013）：可选 SqliteDb 后端——任务状态脱离调用栈成为
//!   独立实体，重启后可查询；中断的任务（Pending/Running）重启时标记为
//!   Failed，保证唤醒计数（remaining）不因重启失真
//!
//! 与 [`crate::scheduler`]（cron 定时任务）无关：本模块是 LLM 驱动的
//! agent 后台任务，不是定时调度。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
};
use crate::notification::SharedNotificationSink;
use crate::session::SessionManager;
use crate::vfs::backend::sqlite_db::SqliteDb;

/// 默认最大并发后台任务数。
pub const DEFAULT_MAX_BACKGROUND_TASKS: usize = 4;

/// 通知中结果摘要的最大字符数。
const RESULT_SUMMARY_MAX_CHARS: usize = 2000;

/// 后台任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已注册，等待并发许可。
    Pending,
    /// 运行中。
    Running,
    /// 已完成（含结果摘要）。
    Completed,
    /// 执行失败（含错误信息）。
    Failed,
    /// 已取消。
    Cancelled,
}

impl TaskStatus {
    /// 持久化用字符串表示（与 serde 的 snake_case 一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// 从持久化字符串还原。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl TaskStatus {
    /// 是否为终态。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }
}

/// 后台任务（注册表条目）。
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTask {
    /// 任务 ID（`bt_` 前缀）。
    pub id: String,
    /// 任务描述（原始委托任务文本，截断）。
    pub description: String,
    /// 状态。
    pub status: TaskStatus,
    /// 发起任务的父会话 ID。
    pub parent_session_id: String,
    /// 结果摘要（Completed 时）。
    pub result: Option<String>,
    /// 错误信息（Failed 时）。
    pub error: Option<String>,
    /// 创建时间（epoch 毫秒）。
    pub created_at: i64,
    /// 完成时间（epoch 毫秒）。
    pub completed_at: Option<i64>,
    /// 注册序号（单调递增，注册表排序键）。
    ///
    /// `created_at` 为毫秒精度，同毫秒注册的任务无法区分先后；
    /// `seq` 保证快照/列表顺序确定（先注册在前）。
    pub seq: u64,
}

/// 任务完成通知器。
///
/// 由 Agent 装配层注入（默认实现 [`SessionTaskNotifier`] 把通知持久化
/// 到父会话）；`remaining` 为父会话中仍进行中的任务数（join 信号）。
#[async_trait]
pub trait TaskNotifier: Send + Sync {
    /// 任务进入终态时调用（Completed / Failed / Cancelled）。
    async fn on_task_terminal(&self, session_id: &str, task: &BackgroundTask, remaining: usize);
}

/// 任务唤醒器（ADR-013：统一消息通知与唤醒原语）。
///
/// 与 [`TaskNotifier`]（注入消息，静默）解耦：唤醒 = 触发主 agent
/// 新一轮生成。判定规则在 [`BackgroundTaskManager::finish`]：
/// `should_wake = remaining == 0 || status == Failed`（oh-my-openagent 实证
/// 语义）——全部完成或失败才唤醒，部分完成保持静默（运行时强制，
/// 非模型自律）。由 Agent 装配层注入（Agent 侧转发到 process_wake）。
#[async_trait]
pub trait TaskWaker: Send + Sync {
    /// 唤醒指定会话的主 agent（触发一轮系统消息处理）。
    async fn wake(&self, session_id: &str);
}

/// 后台任务管理器：注册表 + 并发限制 + 完成通知分发 + 唤醒 + 可选持久化。
#[derive(Clone)]
pub struct BackgroundTaskManager {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    semaphore: Arc<Semaphore>,
    notifier: Option<Arc<dyn TaskNotifier>>,
    /// 唤醒器槽（Arc<Mutex> 支持构建后注入：Agent 完成后注册自引用转发器）。
    waker: Arc<Mutex<Option<Arc<dyn TaskWaker>>>>,
    /// 系统通知通道槽（全部完成/失败时桌面通知；Arc<Mutex> 支持构建后注入）。
    notification: Arc<Mutex<Option<SharedNotificationSink>>>,
    db: Option<SqliteDb>,
    next_seq: Arc<AtomicU64>,
    /// 持久化加载只执行一次（惰性：首次 register/snapshot 前）。
    reloaded: Arc<AtomicBool>,
}

impl BackgroundTaskManager {
    /// 创建管理器（默认并发上限）。
    pub fn new() -> Self {
        Self::with_max_concurrent(DEFAULT_MAX_BACKGROUND_TASKS)
    }

    /// 创建管理器并指定并发上限。
    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            notifier: None,
            waker: Arc::new(Mutex::new(None)),
            notification: Arc::new(Mutex::new(None)),
            db: None,
            next_seq: Arc::new(AtomicU64::new(0)),
            reloaded: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 设置完成通知器（Agent 装配层注入）。
    pub fn with_notifier(mut self, notifier: Arc<dyn TaskNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// 设置唤醒器（ADR-013：全部完成/失败时触发主 agent 新轮）。
    pub fn with_waker(mut self, waker: Arc<dyn TaskWaker>) -> Self {
        self.waker = Arc::new(Mutex::new(Some(waker)));
        self
    }

    /// 设置 SQLite 持久化后端（任务状态脱离调用栈；重启可查询可恢复）。
    pub fn with_db(mut self, db: SqliteDb) -> Self {
        self.db = Some(db);
        self
    }

    /// 设置唤醒器（构建后注入；Agent 构建完成后注册自引用转发器）。
    pub async fn set_waker(&self, waker: Arc<dyn TaskWaker>) {
        *self.waker.lock().await = Some(waker);
    }

    /// 设置系统通知通道（Agent 装配层注入；未注入时静默）。
    pub fn with_notification_sink(mut self, sink: SharedNotificationSink) -> Self {
        self.notification = Arc::new(Mutex::new(Some(sink)));
        self
    }

    /// 设置系统通知通道（构建后注入；运行时替换实现用）。
    pub async fn set_notification_sink(&self, sink: SharedNotificationSink) {
        *self.notification.lock().await = Some(sink);
    }

    /// 尝试获取并发许可；达到上限时立即报错（不阻塞）。
    pub async fn try_acquire(&self) -> std::result::Result<OwnedSemaphorePermit, TianyanError> {
        self.semaphore.clone().try_acquire_owned().map_err(|_| {
            TianyanError::Custom(
                "tool: 后台任务并发已达上限，请等待现有任务完成，或取消后重试".to_string(),
            )
        })
    }

    /// 注册新任务（Pending）并返回任务 ID。
    pub async fn register(&self, description: String, parent_session_id: String) -> String {
        self.ensure_reloaded().await;
        let id = format!("bt_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let task = BackgroundTask {
            id: id.clone(),
            description: truncate(&description, 200),
            status: TaskStatus::Pending,
            parent_session_id,
            result: None,
            error: None,
            created_at: now_ms(),
            completed_at: None,
            seq,
        };
        self.tasks.lock().await.insert(id.clone(), task.clone());
        self.persist_upsert(&task).await;
        id
    }

    /// 标记任务为运行中。
    pub async fn mark_running(&self, id: &str) {
        if let Some(t) = self.tasks.lock().await.get_mut(id) {
            t.status = TaskStatus::Running;
        }
    }

    /// 标记完成并触发通知。
    pub async fn complete(&self, id: &str, result: String) {
        self.finish(
            id,
            TaskStatus::Completed,
            Some(truncate(&result, RESULT_SUMMARY_MAX_CHARS)),
            None,
        )
        .await;
    }

    /// 标记失败并触发通知。
    pub async fn fail(&self, id: &str, error: String) {
        self.finish(id, TaskStatus::Failed, None, Some(truncate(&error, 500)))
            .await;
    }

    /// 取消任务并触发通知。
    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.ensure_reloaded().await;
        let exists = {
            let tasks = self.tasks.lock().await;
            tasks
                .get(id)
                .map(|t| t.status.is_terminal())
                .unwrap_or(false)
        };
        if exists {
            return Ok(());
        }
        self.finish(id, TaskStatus::Cancelled, None, None).await;
        Ok(())
    }

    /// 获取任务快照。
    pub async fn get(&self, id: &str) -> Option<BackgroundTask> {
        self.ensure_reloaded().await;
        self.tasks.lock().await.get(id).cloned()
    }

    /// 获取全部任务快照（按创建时间排序）。
    pub async fn snapshot(&self) -> Vec<BackgroundTask> {
        self.ensure_reloaded().await;
        let mut tasks: Vec<BackgroundTask> = self.tasks.lock().await.values().cloned().collect();
        // 按注册序号排序（seq 单调递增，先注册在前；created_at 毫秒精度
        // 同毫秒无法区分，且 HashMap 迭代顺序随机）
        tasks.sort_by_key(|t| t.seq);
        tasks
    }

    /// 统一终态收尾：更新状态 → 计算剩余计数 → 通知 → 唤醒。
    async fn finish(
        &self,
        id: &str,
        status: TaskStatus,
        result: Option<String>,
        error: Option<String>,
    ) {
        let (session_id, remaining, task) = {
            let mut tasks = self.tasks.lock().await;
            let Some(t) = tasks.get_mut(id) else {
                return;
            };
            if t.status.is_terminal() {
                return; // 幂等：终态不重复通知
            }
            t.status = status;
            t.result = result;
            t.error = error;
            t.completed_at = Some(now_ms());
            let session = t.parent_session_id.clone();
            let task = t.clone();
            // 释放可变借用后再统计剩余计数
            let remaining = tasks
                .values()
                .filter(|x| x.parent_session_id == session && !x.status.is_terminal())
                .count();
            (session, remaining, task)
        };

        self.persist_upsert(&task).await;

        if let Some(notifier) = &self.notifier {
            notifier
                .on_task_terminal(&session_id, &task, remaining)
                .await;
        }

        // ADR-013 唤醒语义：should_wake = allComplete || failure
        // （oh-my-openagent 实证：shouldReply = allComplete || isTaskFailure）。
        // 部分完成保持静默（消息已注入 transcript，等待语义靠消息积累）；
        // 全部完成或失败才触发主 agent 新一轮生成 + 桌面系统通知。
        let should_wake = remaining == 0 || task.status == TaskStatus::Failed;
        if should_wake {
            // 系统通知：桌面原生提醒（非阻塞；sink 内部自行处理线程/队列）
            if let Some(sink) = &*self.notification.lock().await {
                let title = if task.status == TaskStatus::Failed {
                    "后台任务失败"
                } else {
                    "后台任务完成"
                };
                sink.notify(title, &build_notification_text(&task, remaining));
            }
            if let Some(waker) = &*self.waker.lock().await {
                waker.wake(&session_id).await;
            }
        }
    }

    /// 惰性持久化加载：首次使用时把历史任务载入内存。
    ///
    /// 进程重启后 Running/Pending 任务已随进程消亡，标记为 Failed
    /// （"进程重启中断"）——保证 remaining 计数不误判"仍有任务进行中"
    /// （否则全部完成唤醒永不触发）。
    async fn ensure_reloaded(&self) {
        if self.reloaded.load(AtomicOrdering::SeqCst) {
            return;
        }
        self.reloaded.store(true, AtomicOrdering::SeqCst);

        let Some(db) = &self.db else {
            return;
        };
        // 先完整读取（Statement/Connection 非 Send，不可跨 await；块内 drop）
        let rows: Vec<BackgroundTask> = {
            let conn = match db.try_lock() {
                Ok(c) => c,
                Err(_) => {
                    tracing::warn!("后台任务持久化加载跳过（数据库锁不可用）");
                    return;
                }
            };
            let mut stmt = match conn.prepare(
                "SELECT id, description, status, parent_session_id, result, error, created_at, completed_at, seq FROM background_tasks",
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "后台任务持久化加载失败");
                    return;
                }
            };
            stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let status_str: String = row.get(2)?;
                let status = TaskStatus::parse(&status_str).unwrap_or(TaskStatus::Failed); // 未知状态按失败处理
                Ok(BackgroundTask {
                    id,
                    description: row.get(1)?,
                    status,
                    parent_session_id: row.get(3)?,
                    result: row.get(4)?,
                    error: row.get(5)?,
                    created_at: row.get(6)?,
                    completed_at: row.get(7)?,
                    seq: row.get(8)?,
                })
            })
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        let now = now_ms();
        let mut interrupted: Vec<BackgroundTask> = Vec::new();
        let mut tasks = self.tasks.lock().await;
        for mut t in rows {
            if !t.status.is_terminal() {
                t.status = TaskStatus::Failed;
                t.error = Some("进程重启中断（任务随进程消亡）".to_string());
                t.completed_at = Some(now);
                interrupted.push(t.clone());
            }
            // seq 续接：避免与历史任务重复
            if t.seq >= self.next_seq.load(AtomicOrdering::SeqCst) {
                self.next_seq.store(t.seq + 1, AtomicOrdering::SeqCst);
            }
            tasks.insert(t.id.clone(), t);
        }
        drop(tasks);
        for t in interrupted {
            self.persist_upsert(&t).await;
        }
    }

    /// 写入/更新任务到 SQLite（失败仅告警：内存态仍是权威）。
    async fn persist_upsert(&self, task: &BackgroundTask) {
        let Some(db) = &self.db else {
            return;
        };
        let conn = match db.try_lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "后台任务持久化跳过（数据库锁不可用）");
                return;
            }
        };
        let result = conn.execute(
            "INSERT INTO background_tasks (id, description, status, parent_session_id, result, error, created_at, completed_at, seq)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                completed_at = excluded.completed_at",
            rusqlite::params![
                task.id,
                task.description,
                task.status.as_str(),
                task.parent_session_id,
                task.result,
                task.error,
                task.created_at,
                task.completed_at,
                task.seq as i64,
            ],
        );
        if let Err(e) = result {
            tracing::warn!(error = %e, task = %task.id, "后台任务持久化失败");
        }
    }
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认通知器：把完成通知作为 System 消息持久化到父会话。
///
/// 持久化后，通知随会话历史进入下一次上下文组装（主 LLM 下一轮看到）；
/// 前端会话历史同样可见。对应业界"完成时注入消息 + 轮次边界投递"。
pub struct SessionTaskNotifier {
    session_manager: Arc<dyn SessionManager>,
}

impl SessionTaskNotifier {
    /// 创建通知器。
    pub fn new(session_manager: Arc<dyn SessionManager>) -> Self {
        Self { session_manager }
    }
}

#[async_trait]
impl TaskNotifier for SessionTaskNotifier {
    async fn on_task_terminal(&self, session_id: &str, task: &BackgroundTask, remaining: usize) {
        let text = build_notification_text(task, remaining);
        let now = now_ms();
        let sm = StructuredMessage {
            id: format!("msg_{now}"),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text,
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now,
                completed: now,
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: false,
        };
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, sm)
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "后台任务通知持久化失败");
        }
    }
}

/// 构建通知文本（join 信号：携带剩余任务计数）。
pub fn build_notification_text(task: &BackgroundTask, remaining: usize) -> String {
    let status_label = match task.status {
        TaskStatus::Completed => "完成",
        TaskStatus::Failed => "失败",
        TaskStatus::Cancelled => "已取消",
        _ => "结束",
    };
    let detail = match (&task.result, &task.error) {
        (Some(r), _) => format!("结果：{r}"),
        (_, Some(e)) => format!("错误：{e}"),
        _ => String::new(),
    };

    let mut text = format!(
        "[后台任务{status_label}] {}（{}）",
        task.description, task.id
    );
    if !detail.is_empty() {
        text.push_str(&format!("\n{detail}"));
    }
    if remaining > 0 {
        text.push_str(&format!(
            "\n该会话还有 {remaining} 个后台任务进行中，全部完成后请汇总各任务结果。"
        ));
    } else {
        text.push_str("\n该会话的所有后台任务均已完成，请汇总各任务结果并继续原有工作。");
    }
    text
}

/// 截断文本（保留头尾）。
fn truncate(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let head = &text[..max_chars * 2 / 3];
    let tail_start = text.len().saturating_sub(max_chars / 3);
    let tail = &text[tail_start..];
    format!(
        "{}...\n[内容被截断，省略 {} 字符]...\n{}",
        head,
        text.len() - head.len() - tail.len(),
        tail
    )
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 供取消逻辑追踪运行句柄（保留字段位，防止未来扩展时遗漏）。
#[allow(dead_code)]
struct TaskHandle {
    _started: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_status_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(3000);
        let t = truncate(&long, 2000);
        assert!(t.contains("[内容被截断"));
    }

    #[test]
    fn test_notification_text_with_remaining() {
        let task = BackgroundTask {
            id: "bt_test".to_string(),
            description: "搜索资料".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("找到 3 篇".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
        };
        let text = build_notification_text(&task, 2);
        assert!(text.contains("[后台任务完成]"));
        assert!(text.contains("bt_test"));
        assert!(text.contains("找到 3 篇"));
        assert!(text.contains("还有 2 个后台任务进行中"));
    }

    #[test]
    fn test_notification_text_all_complete() {
        let task = BackgroundTask {
            id: "bt_x".to_string(),
            description: "分析日志".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("无异常".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
        };
        let text = build_notification_text(&task, 0);
        assert!(text.contains("所有后台任务均已完成"));
        assert!(text.contains("汇总各任务结果并继续"));
    }

    #[tokio::test]
    async fn test_register_and_complete() {
        let manager = BackgroundTaskManager::new();
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        assert!(id.starts_with("bt_"));
        assert_eq!(manager.get(&id).await.unwrap().status, TaskStatus::Pending);

        manager.mark_running(&id).await;
        assert_eq!(manager.get(&id).await.unwrap().status, TaskStatus::Running);

        manager.complete(&id, "done".to_string()).await;
        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result.as_deref(), Some("done"));
        assert!(task.completed_at.is_some());

        // 幂等：重复 complete 不覆盖
        manager.complete(&id, "again".to_string()).await;
        assert_eq!(
            manager.get(&id).await.unwrap().result.as_deref(),
            Some("done")
        );
    }

    #[tokio::test]
    async fn test_fail_and_cancel() {
        let manager = BackgroundTaskManager::new();
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.fail(&id, "boom".to_string()).await;
        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.error.as_deref(), Some("boom"));

        let id2 = manager.register("t2".to_string(), "s1".to_string()).await;
        manager.cancel(&id2).await.unwrap();
        assert_eq!(
            manager.get(&id2).await.unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn test_concurrency_limit() {
        let manager = BackgroundTaskManager::with_max_concurrent(2);
        let p1 = manager.try_acquire().await.unwrap();
        let p2 = manager.try_acquire().await.unwrap();
        let p3 = manager.try_acquire().await;
        assert!(p3.is_err(), "第 3 个许可应被拒绝");
        drop(p1);
        let p3 = manager.try_acquire().await;
        assert!(p3.is_ok(), "释放后应可获取");
        drop(p2);
        drop(p3.unwrap());
    }

    #[tokio::test]
    async fn test_notifier_called_with_remaining() {
        struct CountingNotifier(Arc<std::sync::Mutex<Vec<(String, usize)>>>);
        #[async_trait]
        impl TaskNotifier for CountingNotifier {
            async fn on_task_terminal(
                &self,
                _session_id: &str,
                task: &BackgroundTask,
                remaining: usize,
            ) {
                self.0.lock().unwrap().push((task.id.clone(), remaining));
            }
        }

        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let manager =
            BackgroundTaskManager::new().with_notifier(Arc::new(CountingNotifier(calls.clone())));

        let id1 = manager.register("a".to_string(), "s1".to_string()).await;
        let id2 = manager.register("b".to_string(), "s1".to_string()).await;
        manager.complete(&id1, "r1".to_string()).await;
        manager.complete(&id2, "r2".to_string()).await;

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2);
        // 先完成 id1 时剩余 1 个；再完成 id2 时剩余 0
        let first = recorded.iter().find(|(id, _)| *id == id1).unwrap();
        let second = recorded.iter().find(|(id, _)| *id == id2).unwrap();
        assert_eq!(first.1, 1, "id1 完成时 id2 仍在进行");
        assert_eq!(second.1, 0, "id2 完成时无剩余任务（join 信号）");
    }

    #[tokio::test]
    async fn test_snapshot_sorted() {
        let manager = BackgroundTaskManager::new();
        let id1 = manager.register("a".to_string(), "s1".to_string()).await;
        let id2 = manager.register("b".to_string(), "s1".to_string()).await;
        let snap = manager.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].id, id1);
        assert_eq!(snap[1].id, id2);
    }

    #[tokio::test]
    async fn test_session_notifier_persists_system_message() {
        use crate::session::SessionManager;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        // 手写 mock：记录 add_structured_message 调用
        struct RecordingSessionManager {
            calls: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl SessionManager for RecordingSessionManager {
            async fn create_session(
                &self,
                _id: &str,
                _message: crate::common::types::Message,
            ) -> Result<crate::session::Session> {
                unreachable!()
            }
            async fn get_session(&self, _id: &str) -> Result<Option<crate::session::Session>> {
                Ok(None)
            }
            async fn update_session(&self, _session: &crate::session::Session) -> Result<()> {
                Ok(())
            }
            async fn add_message(
                &self,
                _session_id: &str,
                _message: crate::common::types::Message,
            ) -> Result<()> {
                Ok(())
            }
            async fn add_structured_message(
                &self,
                _session_id: &str,
                msg: StructuredMessage,
            ) -> Result<()> {
                assert_eq!(msg.role, MessageRole::System, "通知应为 System 消息");
                assert!(
                    msg.parts.iter().any(|p| matches!(p, Part::Text { .. })),
                    "通知应含文本 part"
                );
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(())
            }
            async fn rewrite_messages(
                &self,
                _session_id: &str,
                _messages: &[StructuredMessage],
            ) -> Result<()> {
                Ok(())
            }
            async fn list_sessions(&self) -> Result<Vec<crate::session::Session>> {
                Ok(Vec::new())
            }
            async fn delete_session(&self, _id: &str) -> Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let notifier = SessionTaskNotifier::new(Arc::new(RecordingSessionManager {
            calls: calls.clone(),
        }));
        let task = BackgroundTask {
            id: "bt_t".to_string(),
            description: "任务".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("结果".to_string()),
            error: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
        };
        notifier.on_task_terminal("s1", &task, 0).await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "应持久化一条通知");
    }

    // ── ADR-013：唤醒语义 + 持久化 ─────────────────────────────

    /// 记录唤醒次数的测试唤醒器。
    struct RecordingWaker {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl TaskWaker for RecordingWaker {
        async fn wake(&self, _session_id: &str) {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_wake_on_all_complete_only() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new().with_waker(Arc::new(RecordingWaker {
            calls: calls.clone(),
        }));
        let id1 = manager.register("t1".to_string(), "s1".to_string()).await;
        let id2 = manager.register("t2".to_string(), "s1".to_string()).await;
        manager.complete(&id1, "r1".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            0,
            "部分完成不唤醒（静默注入）"
        );
        manager.complete(&id2, "r2".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "全部完成唤醒一次（shouldReply = allComplete）"
        );
    }

    #[tokio::test]
    async fn test_wake_on_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new().with_waker(Arc::new(RecordingWaker {
            calls: calls.clone(),
        }));
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.fail(&id, "boom".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "失败即唤醒（汇总失败信息）"
        );
    }

    // ── 系统通知通道（全部完成/失败时桌面通知） ───────────────

    /// 记录通知次数的测试通知通道。
    struct RecordingSink {
        calls: Arc<AtomicUsize>,
    }
    impl crate::notification::NotificationSink for RecordingSink {
        fn notify(&self, _title: &str, _body: &str) {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_notification_sink_only_on_should_wake() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager =
            BackgroundTaskManager::new().with_notification_sink(Arc::new(RecordingSink {
                calls: calls.clone(),
            }));
        let id1 = manager.register("t1".to_string(), "s1".to_string()).await;
        let id2 = manager.register("t2".to_string(), "s1".to_string()).await;
        manager.complete(&id1, "r1".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            0,
            "部分完成不发系统通知（仅静默注入消息）"
        );
        manager.complete(&id2, "r2".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "全部完成发一次系统通知"
        );
    }

    #[tokio::test]
    async fn test_notification_sink_on_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager =
            BackgroundTaskManager::new().with_notification_sink(Arc::new(RecordingSink {
                calls: calls.clone(),
            }));
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.fail(&id, "boom".to_string()).await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "失败触发系统通知");
    }

    #[tokio::test]
    async fn test_notification_sink_set_after_build() {
        // 构建后注入（set_notification_sink）：与 waker 槽同模式
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new();
        manager
            .set_notification_sink(Arc::new(RecordingSink {
                calls: calls.clone(),
            }))
            .await;
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.complete(&id, "r".to_string()).await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "构建后注入同样生效");
    }

    #[tokio::test]
    async fn test_wake_scoped_per_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new().with_waker(Arc::new(RecordingWaker {
            calls: calls.clone(),
        }));
        let a = manager.register("ta".to_string(), "s1".to_string()).await;
        let b = manager.register("tb".to_string(), "s2".to_string()).await;
        manager.complete(&a, "ra".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "s1 全部完成 → 唤醒 s1"
        );
        manager.complete(&b, "rb".to_string()).await;
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            2,
            "s2 全部完成 → 唤醒 s2"
        );
    }

    #[tokio::test]
    async fn test_task_persistence_reload() {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        let manager = BackgroundTaskManager::new().with_db(db.clone());
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.mark_running(&id).await;
        manager.complete(&id, "done".to_string()).await;

        // 模拟重启：新 manager 共享同一 db
        let manager2 = BackgroundTaskManager::new().with_db(db);
        let tasks = manager2.snapshot().await;
        assert_eq!(tasks.len(), 1, "重启后任务可查询");
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].result.as_deref(), Some("done"));
        // seq 续接：新注册任务序号不与历史冲突
        let new_id = manager2.register("t2".to_string(), "s1".to_string()).await;
        let tasks = manager2.snapshot().await;
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.id == new_id && t.seq > 0));
    }

    #[tokio::test]
    async fn test_persistence_reload_interrupted_becomes_failed() {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        let manager = BackgroundTaskManager::new().with_db(db.clone());
        let id = manager.register("t1".to_string(), "s1".to_string()).await;
        manager.mark_running(&id).await; // 重启前仍在运行

        let manager2 = BackgroundTaskManager::new().with_db(db);
        let tasks = manager2.snapshot().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].status,
            TaskStatus::Failed,
            "中断任务标记为失败（remaining 计数不误判）"
        );
        assert!(
            tasks[0].error.as_deref().unwrap_or("").contains("重启"),
            "错误信息说明中断原因"
        );
    }
}
