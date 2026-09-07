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
//! ## 与 [`crate::scheduler`]（cron 定时任务）的边界
//!
//! | 维度 | 本模块（agent 后台任务） | `crate::scheduler`（定时任务） |
//! |------|--------------------------|------------------------------|
//! | 驱动 | LLM 委托（delegate background / 后台命令），交互驱动 | cron 表达式，后台驱动 |
//! | 生命周期 | 一次性：创建 → 运行 → 完成/失败/取消 | 周期重复（或单次触发） |
//! | 持久化 | SQLite（`with_db` → `persist_upsert`；ADR-013） | VFS（`TaskStateStore`） |
//! | 唤醒 | 完成/失败经 `TaskWaker` 唤醒主 agent（ADR-013） | 无唤醒语义，纯后台执行 |
//! | 消费方 | 主 agent 对话（通知注入 + join 信号） | 系统维护任务（Summary/GC/进化等） |
//!
//! 两类任务概念不共享状态机与持久化载体——新读者请勿假设统一；
//! 未来若出现跨类需求（如定时委托），在两边各自的边界内扩展。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::common::error::{Result, TianyanError};
use crate::common::llm_judge::truncate_output;
use crate::common::types::StructuredMessage;
use crate::db::Database;
use crate::executor::{CommandTask, CommandTaskStatus};
use crate::notification::SharedNotificationSink;
use crate::session::SessionManager;

/// 默认最大并发后台任务数（ADR-026：同时运行上限）。
pub const DEFAULT_MAX_BACKGROUND_TASKS: usize = 20;

/// 默认后台任务排队上限（ADR-026：超出拒绝；排队中的任务 = Pending）。
pub const DEFAULT_MAX_BACKGROUND_QUEUE: usize = 40;

/// 通知中结果摘要的最大字符数。
const RESULT_SUMMARY_MAX_CHARS: usize = 2000;

/// 子智能体消息流事件通道（ADR-026：面板实时流式显示）。
///
/// core 层只定义通道契约；server 层实现广播（SSE 订阅者分发）。
/// 事件为 ChatStreamEvent 同构 JSON（chunk_type: thought/tool_call/
/// observation/answer），带 task_id 归集到面板对应任务。
#[async_trait]
pub trait TaskEventSink: Send + Sync {
    /// 发布子智能体消息事件。
    async fn emit(&self, task_id: &str, event: serde_json::Value);
}

/// 后台任务类型（统一注册表：委托 / 命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// 子代理委托（delegate_to_agent background）。
    #[default]
    Delegate,
    /// 后台命令（execute_command background，含就绪探测）。
    Command,
}

impl TaskKind {
    /// 序列化字符串表示。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delegate => "delegate",
            Self::Command => "command",
        }
    }
}

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
    /// 任务类型（delegate / command）。
    pub kind: TaskKind,
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
    /// 终端命令输出尾部（ADR-026：面板展开显示最近输出；委托任务为 None）。
    pub output_tail: Option<String>,
    /// 终端命令日志文件路径（ADR-028：日志分页查看；委托任务为 None）。
    pub log_file: Option<String>,
    /// 创建时间（epoch 毫秒）。
    pub created_at: i64,
    /// 完成时间（epoch 毫秒）。
    pub completed_at: Option<i64>,
    /// 注册序号（单调递增，注册表排序键）。
    ///
    /// `created_at` 为毫秒精度，同毫秒注册的任务无法区分先后；
    /// `seq` 保证快照/列表顺序确定（先注册在前）。
    pub seq: u64,
    /// 时序锚点：任务启动时链上消息数（= 下一条消息的 seq）。
    ///
    /// 回退到用户输入 U 时，锚点 > U.seq 的任务属于"回退点之后"，
    /// 应一并取消（方案 B：任务与链上时序点关联，精确取消）。
    pub anchor_seq: i64,
}

impl BackgroundTask {
    /// CommandTask → BackgroundTask 同构映射（统一后台任务视图）。
    ///
    /// 后台终端命令（execute_command background）此前仅 agent 内
    /// task_status 可见，会话面板不可见——合并进同一列表后，面板与
    /// REST 取消入口对两类任务行为一致（状态映射 snake_case 同语义）。
    pub(crate) fn from_command_task(t: CommandTask) -> Self {
        let status = match t.status {
            CommandTaskStatus::Running => TaskStatus::Running,
            CommandTaskStatus::Completed => TaskStatus::Completed,
            CommandTaskStatus::Failed => TaskStatus::Failed,
            CommandTaskStatus::Cancelled => TaskStatus::Cancelled,
        };
        // 命令串截断（面板单行展示；委托侧描述同语义）
        let description: String = t.command.chars().take(160).collect();
        // 失败时携带退出码（面板终态展示用；Cancelled 无退出码）
        let error = match t.status {
            CommandTaskStatus::Failed => Some(format!("退出码 {}", t.exit_code.unwrap_or(-1))),
            _ => None,
        };
        Self {
            id: t.id,
            kind: TaskKind::Command,
            description,
            status,
            parent_session_id: t.parent_session_id,
            result: None,
            error,
            output_tail: Some(t.output_tail),
            log_file: t.log_file,
            created_at: t.created_at,
            completed_at: t.completed_at,
            seq: t.seq,
            anchor_seq: t.anchor_seq,
        }
    }
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

/// 子代理暂存的结果（ADR-030：submit_result 工具写入；complete 时优先使用）。
#[derive(Debug, Clone)]
pub struct StagedResult {
    /// 完整结果（子代理提交的最终报告/结论）。
    pub result: String,
    /// 可选一句话摘要（通知/唤醒轮用）。
    pub summary: Option<String>,
}

/// 后台任务管理器：注册表 + 并发限制 + 完成通知分发 + 唤醒 + 可选持久化。
#[derive(Clone)]
pub struct BackgroundTaskManager {
    tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    /// 子代理暂存结果（ADR-030：submit_result 写入；complete 时消费）。
    staged_results: Arc<Mutex<HashMap<String, StagedResult>>>,
    /// 运行许可（同时运行上限；ADR-026 双信号量排队模型）。
    run_sem: Arc<Semaphore>,
    /// 排队槽位（排队上限；超出拒绝）。
    queue_sem: Arc<Semaphore>,
    notifier: Option<Arc<dyn TaskNotifier>>,
    /// 唤醒器槽（Arc<Mutex> 支持构建后注入：Agent 完成后注册自引用转发器）。
    waker: Arc<Mutex<Option<Arc<dyn TaskWaker>>>>,
    /// 系统通知通道槽（全部完成/失败时桌面通知；Arc<Mutex> 支持构建后注入）。
    notification: Arc<Mutex<Option<SharedNotificationSink>>>,
    /// 任务结果自审器（G4；None 时不自审——通知直接注入）。
    task_reviewer: Option<Arc<dyn crate::executor::judge::TaskReviewer>>,
    /// 结构化 Trace 收集器（G6；None 时不记录任务 span）。
    trace_collector: Option<Arc<crate::observability::trace::TraceCollector>>,
    /// 任务状态事件通道（ADR-028：状态转移 emit；None 时静默）。
    event_sink: Option<Arc<dyn TaskEventSink>>,
    db: Option<Arc<Database>>,
    next_seq: Arc<AtomicU64>,
    /// 持久化加载只执行一次（惰性：首次 register/snapshot 前）。
    reloaded: Arc<AtomicBool>,
}

impl BackgroundTaskManager {
    /// 创建管理器（默认并发上限 + 排队上限）。
    pub fn new() -> Self {
        Self::with_concurrency(DEFAULT_MAX_BACKGROUND_TASKS, DEFAULT_MAX_BACKGROUND_QUEUE)
    }

    /// 创建管理器并指定并发上限（排队上限取并发上限，兼容旧调用）。
    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self::with_concurrency(max_concurrent, max_concurrent.max(1) * 2)
    }

    /// 创建管理器并指定运行上限与排队上限（ADR-026 双信号量排队模型）。
    pub fn with_concurrency(max_concurrent: usize, max_queue: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            staged_results: Arc::new(Mutex::new(HashMap::new())),
            run_sem: Arc::new(Semaphore::new(max_concurrent.max(1))),
            queue_sem: Arc::new(Semaphore::new(max_queue.max(max_concurrent.max(1)))),
            notifier: None,
            waker: Arc::new(Mutex::new(None)),
            notification: Arc::new(Mutex::new(None)),
            task_reviewer: None,
            trace_collector: None,
            event_sink: None,
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
    pub fn with_db(mut self, db: Arc<Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// 设置任务结果自审器（G4：完成通知注入前轻量自审；None 时不自审）。
    ///
    /// 未通过自审的任务结果前缀 `[自审未通过] {reason}`，由主 agent
    /// 下一轮看到后决策（复核/重新委托）——不自动重跑。
    pub fn with_task_reviewer(
        mut self,
        reviewer: Arc<dyn crate::executor::judge::TaskReviewer>,
    ) -> Self {
        self.task_reviewer = Some(reviewer);
        self
    }

    /// 设置任务状态事件通道（ADR-028：状态转移 emit；None 时静默）。
    pub fn with_event_sink(mut self, sink: Arc<dyn TaskEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// 设置结构化 Trace 收集器（G6：任务终态 span 记录）。
    pub fn with_trace_collector(
        mut self,
        collector: Arc<crate::observability::trace::TraceCollector>,
    ) -> Self {
        self.trace_collector = Some(collector);
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

    /// 获取排队槽位（ADR-026）：排队已满时立即报错（不阻塞）。
    ///
    /// 调用方拿到槽位后应注册任务（Pending），再等待运行许可（acquire_run）。
    pub async fn acquire_queue_slot(
        &self,
    ) -> std::result::Result<OwnedSemaphorePermit, TianyanError> {
        self.queue_sem.clone().try_acquire_owned().map_err(|_| {
            TianyanError::Custom(
                "tool: 后台任务排队已满，请等待现有任务完成，或取消后重试".to_string(),
            )
        })
    }

    /// 等待运行许可（ADR-026）：阻塞排队，许可释放后继续。
    pub async fn acquire_run(&self) -> std::result::Result<OwnedSemaphorePermit, TianyanError> {
        self.run_sem.clone().acquire_owned().await.map_err(|_| {
            TianyanError::Custom("tool: 后台任务调度器已关闭".to_string())
        })
    }

    /// 注册新任务（Pending）并返回任务 ID。
    pub async fn register(
        &self,
        kind: TaskKind,
        description: String,
        parent_session_id: String,
        anchor_seq: i64,
    ) -> String {
        self.ensure_reloaded().await;
        let id = format!("bt_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let task = BackgroundTask {
            kind,
            id: id.clone(),
            description: truncate_output(&description, 200),
            status: TaskStatus::Pending,
            parent_session_id,
            result: None,
            error: None,
            output_tail: None,
            log_file: None,
            created_at: now_ms(),
            completed_at: None,
            seq,
            anchor_seq,
        };
        self.tasks.lock().await.insert(id.clone(), task.clone());
        self.persist_upsert(&task).await;
        // ADR-028：状态转移事件（pending）——面板实时显示排队/运行中
        if let Some(sink) = &self.event_sink {
            sink.emit(
                &id,
                serde_json::json!({
                    "type": "task_status",
                    "task_id": id,
                    "kind": kind.as_str(),
                    "status": "pending",
                    "description": task.description,
                    "created_at": task.created_at,
                }),
            )
            .await;
        }
        id
    }

    /// 标记任务为运行中（内存 + SQLite）。
    pub async fn mark_running(&self, id: &str) {
        let updated = {
            let mut tasks = self.tasks.lock().await;
            match tasks.get_mut(id) {
                Some(t) => {
                    t.status = TaskStatus::Running;
                    Some(t.clone())
                }
                None => None,
            }
        };
        if let Some(t) = updated {
            self.persist_upsert(&t).await;
            // ADR-028：状态转移事件（running）
            if let Some(sink) = &self.event_sink {
                sink.emit(
                    &t.id,
                    serde_json::json!({
                        "type": "task_status",
                        "task_id": t.id,
                        "kind": t.kind.as_str(),
                        "status": "running",
                    }),
                )
                .await;
            }
        }
    }

    /// 暂存子代理提交的结果（ADR-030：submit_result 工具写入；complete 时
    /// 优先使用）。任务必须存在且未终态。
    pub async fn stage_result(
        &self,
        id: &str,
        result: String,
        summary: Option<String>,
    ) -> Result<()> {
        self.ensure_reloaded().await;
        let exists = {
            let tasks = self.tasks.lock().await;
            tasks.get(id).is_some()
        };
        if !exists {
            return Err(TianyanError::not_found(format!("后台任务不存在：{id}")));
        }
        self.staged_results
            .lock()
            .await
            .insert(id.to_string(), StagedResult { result, summary });
        Ok(())
    }

    /// 读取子代理暂存结果（ADR-030：submit_result 写入；不消费）。
    ///
    /// 前台 `run_subagent_loop` 在收尾时读取暂存结果作为最终结果
    /// （模型最后输出兜底）；后台 `complete` 消费同一暂存结果。
    pub async fn staged_result(&self, id: &str) -> Option<StagedResult> {
        self.staged_results.lock().await.get(id).cloned()
    }

    /// 标记完成并触发通知（G4：注入前先过自审门，未通过则结果带标记）。
    pub async fn complete(&self, id: &str, result: String) {
        // ADR-030：暂存结果（submit_result 写入）优先；否则用传入结果
        let staged = self.staged_results.lock().await.remove(id);
        let result = match staged {
            Some(s) => s.result,
            None => result,
        };
        let result = self.maybe_self_review(id, &result).await;
        self.finish(
            id,
            TaskStatus::Completed,
            Some(truncate_output(&result, RESULT_SUMMARY_MAX_CHARS)),
            None,
        )
        .await;
    }

    /// G4 循环内自审门：任务结果注入父会话前轻量 LLM 自审。
    ///
    /// 未通过（`NeedsChanges`/`Fail`）时在结果前加 `[自审未通过] {reason}`
    /// 标记——通知与 `task_status` 均携带，主 agent 下一轮看到后决策
    /// （复核/重新委托），**不自动重跑**。无自审器 / 空结果 / 自审通过
    /// 时原样返回（行为零变化）。
    async fn maybe_self_review(&self, id: &str, result: &str) -> String {
        let Some(reviewer) = &self.task_reviewer else {
            return result.to_string();
        };
        if result.trim().is_empty() {
            return result.to_string();
        }
        let Some(task) = self.get(id).await else {
            return result.to_string();
        };

        let judgment = reviewer.review(&task.description, result).await;
        if judgment.verdict.is_pass() {
            return result.to_string();
        }

        let marker = format!("[自审未通过] {}", truncate_output(&judgment.reason, 200));
        tracing::info!(
            task_id = %id,
            reason = %judgment.reason,
            "后台任务结果未通过自审（已标记，主 agent 复核）"
        );
        format!("{marker}\n{result}")
    }

    /// 标记失败并触发通知。
    pub async fn fail(&self, id: &str, error: String) {
        self.finish(
            id,
            TaskStatus::Failed,
            None,
            Some(truncate_output(&error, 500)),
        )
        .await;
    }

    /// 取消任务并触发通知。
    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.ensure_reloaded().await;
        // 内存未终态任务可取消；终态（内存已移除/落库）为幂等空操作
        let terminal = {
            let tasks = self.tasks.lock().await;
            tasks
                .get(id)
                .map(|t| t.status.is_terminal())
                .unwrap_or(false)
        };
        if terminal {
            return Ok(());
        }
        if self.tasks.lock().await.contains_key(id) {
            self.finish(id, TaskStatus::Cancelled, None, None).await;
        }
        Ok(())
    }

    /// 获取任务快照（内存未命中时查 SQLite——终态任务已落库）。
    pub async fn get(&self, id: &str) -> Option<BackgroundTask> {
        self.ensure_reloaded().await;
        if let Some(t) = self.tasks.lock().await.get(id).cloned() {
            return Some(t);
        }
        self.query_sqlite(id).await
    }

    /// 获取全部任务快照（内存未终态 + SQLite 全量合并，按 seq 排序）。
    pub async fn snapshot(&self) -> Vec<BackgroundTask> {
        self.ensure_reloaded().await;
        self.evict_expired().await;
        let mut tasks: Vec<BackgroundTask> = self.tasks.lock().await.values().cloned().collect();
        // 合并 SQLite 全量（去重：内存优先——未终态任务的最新状态在内存）
        let mut seen: std::collections::HashSet<String> =
            tasks.iter().map(|t| t.id.clone()).collect();
        for t in self.query_sqlite_all().await {
            if seen.insert(t.id.clone()) {
                tasks.push(t);
            }
        }
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
        // ADR-028：状态转移事件（终态）——面板实时更新，前端据此触发消息对齐
        if let Some(sink) = &self.event_sink {
            sink.emit(
                &task.id,
                serde_json::json!({
                    "type": "task_status",
                    "task_id": task.id,
                    "kind": task.kind.as_str(),
                    "status": task.status.as_str(),
                    "result": task.result,
                    "error": task.error,
                    "completed_at": task.completed_at,
                }),
            )
            .await;
        }
        // ADR-026 SQL 权威：有 db 时终态任务从内存移除（SQLite 保留；查询走 SQL），
        // 内存只保留未终态任务（Running ≤ 并发 + Pending 排队），有界。
        // 无 db（测试桩/未装配持久化）时终态保留内存——没有 SQLite 可落，移除即丢失。
        if self.db.is_some() {
            self.tasks.lock().await.remove(id);
        }

        // G6 结构化 Trace：任务终态 span（独立子树根，task_id 关联）
        if let Some(trace) = &self.trace_collector {
            let duration = task.completed_at.unwrap_or_else(now_ms) - task.created_at;
            trace.record_task(
                &session_id,
                &task.id,
                &task.description,
                task.result.as_deref().unwrap_or(""),
                duration,
                task.status == TaskStatus::Completed,
                task.error.clone(),
            );
        }

        if let Some(notifier) = &self.notifier {
            notifier
                .on_task_terminal(&session_id, &task, remaining)
                .await;
        }

        // ADR-013 唤醒语义：should_wake = allComplete || failure
        // （oh-my-openagent 实证：shouldReply = allComplete || isTaskFailure）。
        // 部分完成保持静默（消息已注入 transcript，等待语义靠消息积累）；
        // 全部完成或失败才触发主 agent 新一轮生成 + 桌面系统通知。
        // 例外：Cancelled（用户停止传播）不唤醒——用户主动停止后不应自动
        // 触发汇总轮（否则"停止"后又自动生成一轮，打扰）。
        let should_wake = task.status != TaskStatus::Cancelled
            && (remaining == 0 || task.status == TaskStatus::Failed);
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
                "SELECT id, kind, description, status, parent_session_id, result, error, created_at, completed_at, seq FROM background_tasks",
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "后台任务持久化加载失败");
                    return;
                }
            };
            stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let kind_str: String = row.get(1)?;
                let kind = if kind_str == "command" {
                    TaskKind::Command
                } else {
                    TaskKind::Delegate
                };
                let status_str: String = row.get(3)?;
                let status = TaskStatus::parse(&status_str).unwrap_or(TaskStatus::Failed); // 未知状态按失败处理
                Ok(BackgroundTask {
                    id,
                    kind,
                    description: row.get(2)?,
                    status,
                    parent_session_id: row.get(4)?,
                    result: row.get(5)?,
                    error: row.get(6)?,
                    output_tail: None,
            log_file: None,
                    created_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    seq: row.get(9)?,
                    anchor_seq: 0,
                })
            })
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        let now = now_ms();
        let mut interrupted: Vec<BackgroundTask> = Vec::new();
        let mut tasks = self.tasks.lock().await;
        for mut t in rows {
            // seq 续接：避免与历史任务重复（先取 seq，t 可能随后被移动）
            if t.seq >= self.next_seq.load(AtomicOrdering::SeqCst) {
                self.next_seq.store(t.seq + 1, AtomicOrdering::SeqCst);
            }
            if !t.status.is_terminal() {
                // 进程重启中断：标记 Failed 并落库（remaining 计数不误判）
                t.status = TaskStatus::Failed;
                t.error = Some("进程重启中断（任务随进程消亡）".to_string());
                t.completed_at = Some(now);
                interrupted.push(t.clone());
                tasks.insert(t.id.clone(), t);
            }
            // 终态任务不载入内存（ADR-026 SQL 权威：查询走 SQL）
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
            "INSERT INTO background_tasks (id, kind, description, status, parent_session_id, result, error, created_at, completed_at, seq)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                completed_at = excluded.completed_at",
            rusqlite::params![
                task.id,
                task.kind.as_str(),
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

    /// 按 ID 查询 SQLite（终态任务已从内存移除；未命中返回 None）。
    async fn query_sqlite(&self, id: &str) -> Option<BackgroundTask> {
        let db = self.db.as_ref()?;
        let conn = db.try_lock().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, description, status, parent_session_id, result, error, created_at, completed_at, seq
                 FROM background_tasks WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(rusqlite::params![id], map_task_row)
            .ok()
    }

    /// 查询 SQLite 全部任务（终态任务权威存储；与内存未终态合并见 snapshot）。
    async fn query_sqlite_all(&self) -> Vec<BackgroundTask> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        let conn = match db.try_lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT id, kind, description, status, parent_session_id, result, error, created_at, completed_at, seq
             FROM background_tasks",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], map_task_row)
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// TTL 逐出（ADR-026）：删除 3 天前的终态任务（惰性：snapshot 时触发）。
    async fn evict_expired(&self) {
        let Some(db) = &self.db else {
            return;
        };
        let conn = match db.try_lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        let cutoff = now_ms() - 3 * 24 * 3600 * 1000;
        if let Err(e) = conn.execute(
            "DELETE FROM background_tasks WHERE completed_at IS NOT NULL AND completed_at < ?1",
            rusqlite::params![cutoff as i64],
        ) {
            tracing::warn!(error = %e, "后台任务 TTL 逐出失败");
        }
    }
}

/// SQLite 行 → BackgroundTask（与 ensure_reloaded 共用映射）。
fn map_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackgroundTask> {
    let id: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let kind = if kind_str == "command" {
        TaskKind::Command
    } else {
        TaskKind::Delegate
    };
    let status_str: String = row.get(3)?;
    let status = TaskStatus::parse(&status_str).unwrap_or(TaskStatus::Failed); // 未知状态按失败处理
    Ok(BackgroundTask {
        id,
        kind,
        description: row.get(2)?,
        status,
        parent_session_id: row.get(4)?,
        result: row.get(5)?,
        error: row.get(6)?,
        output_tail: None,
        log_file: None,
        created_at: row.get(7)?,
        completed_at: row.get(8)?,
        seq: row.get(9)?,
        anchor_seq: 0,
    })
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
        let sm = StructuredMessage::system(session_id.to_string(), text);
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

/// 默认命令通知器：把完成通知作为 System 消息持久化到父会话。
///
/// 与 [`SessionTaskNotifier`] 同构：持久化后通知随会话历史进入下一次
/// 上下文组装（主 LLM 下一轮看到）；`waker` 槽由 Agent 构建完成后注入
/// （[`AgentWakeForwarder`] 经 `CommandWaker` 适配转发）。
pub struct SessionCommandNotifier {
    session_manager: Arc<dyn SessionManager>,
    waker: Arc<Mutex<Option<Arc<dyn TaskWaker>>>>,
}

impl SessionCommandNotifier {
    /// 创建通知器。
    pub fn new(session_manager: Arc<dyn SessionManager>) -> Self {
        Self {
            session_manager,
            waker: Arc::new(Mutex::new(None)),
        }
    }

    /// 设置唤醒器（Agent 构建完成后注入）。
    pub async fn set_waker(&self, waker: Arc<dyn TaskWaker>) {
        *self.waker.lock().await = Some(waker);
    }
}

#[async_trait]
impl crate::executor::CommandNotifier for SessionCommandNotifier {
    async fn on_command_terminal(&self, session_id: &str, task: &CommandTask, remaining: usize) {
        let text = build_command_notification_text(task, remaining);
        let sm = StructuredMessage::system(session_id.to_string(), text);
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, sm)
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "后台命令通知持久化失败");
        }
    }

    async fn on_command_ready(&self, session_id: &str, task: &CommandTask, note: &str) {
        let text = format!(
            "[后台服务{}] {}（{}）——{}",
            if note.contains("超时") {
                "未就绪"
            } else {
                "就绪"
            },
            truncate_output(&task.command, 120),
            task.id,
            note
        );
        let sm = StructuredMessage::system(session_id.to_string(), text);
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, sm)
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "后台服务就绪通知持久化失败");
        }
    }
}

/// 构建命令通知文本（join 信号：携带剩余计数；不含输出正文——输出经
/// task_status / 日志文件按需读取，避免大输出污染会话上下文）。
pub fn build_command_notification_text(task: &CommandTask, remaining: usize) -> String {
    let status_label = match task.status {
        CommandTaskStatus::Completed => "完成",
        CommandTaskStatus::Failed => "失败",
        CommandTaskStatus::Cancelled => "已取消",
        _ => "结束",
    };
    let exit = match (task.status, task.exit_code) {
        (CommandTaskStatus::Completed, Some(0)) => String::new(),
        (_, Some(code)) => format!("，退出码 {code}"),
        (_, None) => String::new(),
    };
    let mut text = format!(
        "[后台命令{status_label}] {}（{}{}）",
        truncate_output(&task.command, 120),
        task.id,
        exit
    );
    if let Some(path) = &task.log_file {
        text.push_str(&format!("\n日志：{path}（输出尾部可用 task_status 查询）"));
    }
    if remaining > 0 {
        text.push_str(&format!(
            "\n该会话还有 {remaining} 个后台命令进行中，全部完成后请汇总各任务结果。"
        ));
    } else {
        text.push_str("\n该会话的所有后台命令均已完成，请汇总并继续原有工作。");
    }
    text
}

/// 当前时间（epoch 毫秒）。
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{MessageRole, Part};
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_status_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_from_command_task_mapping() {
        let t = CommandTask {
            id: "cmd_7".to_string(),
            command: "python -m src.main --config long.yaml".to_string(),
            cwd: Some("F:\\work".to_string()),
            parent_session_id: "session-1".to_string(),
            status: CommandTaskStatus::Running,
            exit_code: None,
            pid: Some(1234),
            log_file: Some("cmd_7.log".to_string()),
            output_tail: String::new(),
            created_at: 1000,
            completed_at: None,
            seq: 7,
            anchor_seq: 0,
            ready: false,
            ready_note: None,
        };
        let b = BackgroundTask::from_command_task(t);
        assert_eq!(b.id, "cmd_7");
        assert_eq!(b.kind, TaskKind::Command);
        assert_eq!(b.status, TaskStatus::Running);
        assert_eq!(b.parent_session_id, "session-1");
        assert_eq!(b.seq, 7);
        assert_eq!(b.completed_at, None);

        // 状态映射：四态一一对应
        let mk = |s: CommandTaskStatus| {
            let t = CommandTask {
                status: s,
                exit_code: Some(1),
                completed_at: Some(2000),
                ..CommandTask {
                    id: String::new(),
                    command: String::new(),
                    cwd: None,
                    parent_session_id: String::new(),
                    status: CommandTaskStatus::Running,
                    exit_code: None,
                    pid: None,
                    log_file: None,
                    output_tail: String::new(),
                    created_at: 0,
                    completed_at: None,
                    seq: 0,
                    anchor_seq: 0,
                    ready: false,
                    ready_note: None,
                }
            };
            BackgroundTask::from_command_task(t)
        };
        assert_eq!(
            mk(CommandTaskStatus::Completed).status,
            TaskStatus::Completed
        );
        assert_eq!(mk(CommandTaskStatus::Failed).status, TaskStatus::Failed);
        assert_eq!(
            mk(CommandTaskStatus::Cancelled).status,
            TaskStatus::Cancelled
        );

        // 失败携带退出码（面板终态展示），其余终态无 error
        let failed = mk(CommandTaskStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("退出码 1"));
        assert_eq!(mk(CommandTaskStatus::Completed).error, None);
        assert_eq!(mk(CommandTaskStatus::Cancelled).error, None);

        // 长命令描述截断到 160 字符
        let long = CommandTask {
            command: "x".repeat(500),
            parent_session_id: "s".to_string(),
            status: CommandTaskStatus::Running,
            seq: 1,
            ..CommandTask {
                id: String::new(),
                command: String::new(),
                cwd: None,
                parent_session_id: String::new(),
                status: CommandTaskStatus::Running,
                exit_code: None,
                pid: None,
                log_file: None,
                output_tail: String::new(),
                created_at: 0,
                completed_at: None,
                seq: 0,
                anchor_seq: 0,
                ready: false,
                ready_note: None,
            }
        };
        let b = BackgroundTask::from_command_task(long);
        assert_eq!(b.description.chars().count(), 160);
    }

    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(3000);
        let t = truncate_output(&long, 2000);
        assert!(t.contains("[内容被截断"));
    }

    #[test]
    fn test_notification_text_with_remaining() {
        let task = BackgroundTask {
            kind: TaskKind::Delegate,
            id: "bt_test".to_string(),
            description: "搜索资料".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("找到 3 篇".to_string()),
            error: None,
            output_tail: None,
            log_file: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
            anchor_seq: 0,
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
            kind: TaskKind::Delegate,
            id: "bt_x".to_string(),
            description: "分析日志".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("无异常".to_string()),
            error: None,
            output_tail: None,
            log_file: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
            anchor_seq: 0,
        };
        let text = build_notification_text(&task, 0);
        assert!(text.contains("所有后台任务均已完成"));
        assert!(text.contains("汇总各任务结果并继续"));
    }

    #[tokio::test]
    async fn test_register_and_complete() {
        let manager = BackgroundTaskManager::new();
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
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
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
        manager.fail(&id, "boom".to_string()).await;
        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.error.as_deref(), Some("boom"));

        let id2 = manager
            .register(TaskKind::Delegate, "t2".to_string(), "s1".to_string(), 0)
            .await;
        manager.cancel(&id2).await.unwrap();
        assert_eq!(
            manager.get(&id2).await.unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn test_concurrency_limit() {
        // ADR-026 双信号量：排队槽位（满则拒绝）+ 运行许可（阻塞排队）
        let manager = BackgroundTaskManager::with_concurrency(2, 4);

        // 排队槽位：4 个可拿，第 5 个拒绝
        let q1 = manager.acquire_queue_slot().await.unwrap();
        let q2 = manager.acquire_queue_slot().await.unwrap();
        let q3 = manager.acquire_queue_slot().await.unwrap();
        let q4 = manager.acquire_queue_slot().await.unwrap();
        let q5 = manager.acquire_queue_slot().await;
        assert!(q5.is_err(), "第 5 个排队槽位应被拒绝");
        drop(q1);
        let q5 = manager.acquire_queue_slot().await;
        assert!(q5.is_ok(), "释放后应可获取");
        drop(q2);
        drop(q3);
        drop(q4);
        drop(q5.unwrap());

        // 运行许可：2 个可拿，第 3 个阻塞（timeout 验证）
        let r1 = manager.acquire_run().await.unwrap();
        let r2 = manager.acquire_run().await.unwrap();
        let r3 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            manager.acquire_run(),
        );
        assert!(r3.await.is_err(), "第 3 个运行许可应阻塞排队");
        drop(r1);
        let r3 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            manager.acquire_run(),
        );
        assert!(r3.await.is_ok(), "释放后应可获取");
        drop(r2);
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

        let id1 = manager
            .register(TaskKind::Delegate, "a".to_string(), "s1".to_string(), 0)
            .await;
        let id2 = manager
            .register(TaskKind::Delegate, "b".to_string(), "s1".to_string(), 0)
            .await;
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
        let id1 = manager
            .register(TaskKind::Delegate, "a".to_string(), "s1".to_string(), 0)
            .await;
        let id2 = manager
            .register(TaskKind::Delegate, "b".to_string(), "s1".to_string(), 0)
            .await;
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
            kind: TaskKind::Delegate,
            id: "bt_t".to_string(),
            description: "任务".to_string(),
            status: TaskStatus::Completed,
            parent_session_id: "s1".to_string(),
            result: Some("结果".to_string()),
            error: None,
            output_tail: None,
            log_file: None,
            created_at: 0,
            completed_at: Some(1),
            seq: 0,
            anchor_seq: 0,
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
        let id1 = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
        let id2 = manager
            .register(TaskKind::Delegate, "t2".to_string(), "s1".to_string(), 0)
            .await;
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
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
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
        let id1 = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
        let id2 = manager
            .register(TaskKind::Delegate, "t2".to_string(), "s1".to_string(), 0)
            .await;
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
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
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
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
        manager.complete(&id, "r".to_string()).await;
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1, "构建后注入同样生效");
    }

    #[tokio::test]
    async fn test_wake_scoped_per_session() {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new().with_waker(Arc::new(RecordingWaker {
            calls: calls.clone(),
        }));
        let a = manager
            .register(TaskKind::Delegate, "ta".to_string(), "s1".to_string(), 0)
            .await;
        let b = manager
            .register(TaskKind::Delegate, "tb".to_string(), "s2".to_string(), 0)
            .await;
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
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let manager = BackgroundTaskManager::new().with_db(db.clone());
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
        manager.mark_running(&id).await;
        manager.complete(&id, "done".to_string()).await;

        // 模拟重启：新 manager 共享同一 db
        let manager2 = BackgroundTaskManager::new().with_db(db);
        let tasks = manager2.snapshot().await;
        assert_eq!(tasks.len(), 1, "重启后任务可查询");
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].result.as_deref(), Some("done"));
        // seq 续接：新注册任务序号不与历史冲突
        let new_id = manager2
            .register(TaskKind::Delegate, "t2".to_string(), "s1".to_string(), 0)
            .await;
        let tasks = manager2.snapshot().await;
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.id == new_id && t.seq > 0));
    }

    #[tokio::test]
    async fn test_persistence_reload_interrupted_becomes_failed() {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        let manager = BackgroundTaskManager::new().with_db(db.clone());
        let id = manager
            .register(TaskKind::Delegate, "t1".to_string(), "s1".to_string(), 0)
            .await;
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

    // ── G4 循环内自审门 ───────────────────────────────────────

    use crate::executor::judge::{Judgment, TaskReviewer, Verdict};

    /// 固定判定结果的 mock 自审器。
    struct FixedReviewer {
        verdict: Verdict,
        reason: String,
        reviewed: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TaskReviewer for FixedReviewer {
        async fn review(&self, _description: &str, _result: &str) -> Judgment {
            self.reviewed.fetch_add(1, Ordering::SeqCst);
            Judgment {
                verdict: self.verdict.clone(),
                reason: self.reason.clone(),
            }
        }
    }

    #[tokio::test]
    async fn test_complete_marks_failed_review() {
        // 自审未通过 → 结果带 [自审未通过] 标记
        let reviewed = Arc::new(AtomicUsize::new(0));
        let manager = BackgroundTaskManager::new().with_task_reviewer(Arc::new(FixedReviewer {
            verdict: Verdict::NeedsChanges {
                suggestion: "继续处理".to_string(),
            },
            reason: "只完成了一半".to_string(),
            reviewed: reviewed.clone(),
        }));
        let id = manager
            .register(TaskKind::Delegate, "整理日志".to_string(), "s1".to_string(), 0)
            .await;
        manager
            .complete(&id, "已处理 6/12 个文件".to_string())
            .await;

        let task = manager.get(&id).await.unwrap();
        let result = task.result.unwrap();
        assert!(result.contains("[自审未通过]"), "应带自审标记: {result}");
        assert!(result.contains("只完成了一半"));
        assert!(result.contains("已处理 6/12 个文件"), "原始结果应保留");
        assert_eq!(reviewed.load(Ordering::SeqCst), 1, "自审器应被调用一次");
    }

    #[tokio::test]
    async fn test_complete_passed_review_unchanged() {
        // 自审通过 → 结果原样
        let manager = BackgroundTaskManager::new().with_task_reviewer(Arc::new(FixedReviewer {
            verdict: Verdict::Pass,
            reason: "结果完整".to_string(),
            reviewed: Arc::new(AtomicUsize::new(0)),
        }));
        let id = manager
            .register(TaskKind::Delegate, "任务".to_string(), "s1".to_string(), 0)
            .await;
        manager.complete(&id, "完成".to_string()).await;

        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.result.as_deref(), Some("完成"));
    }

    #[tokio::test]
    async fn test_complete_without_reviewer_unchanged() {
        // 未注入自审器 → 行为零变化
        let manager = BackgroundTaskManager::new();
        let id = manager
            .register(TaskKind::Delegate, "任务".to_string(), "s1".to_string(), 0)
            .await;
        manager.complete(&id, "结果".to_string()).await;

        let task = manager.get(&id).await.unwrap();
        assert_eq!(task.result.as_deref(), Some("结果"));
    }
}
