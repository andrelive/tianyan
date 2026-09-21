use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Semaphore};

use crate::common::error::{Result, TianyanError};
use crate::executor::truncate;

pub(super) const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 30;

/// 后台命令任务 ID 前缀（`cmd_`；与委托后台任务的 `bt_` 区分）。
pub const COMMAND_TASK_PREFIX: &str = "cmd_";

/// 后台命令并发上限（防失控扇出；超出时排队等待许可）。
pub const DEFAULT_MAX_CONCURRENT_COMMANDS: usize = 8;

/// 注册表保留上限：超出逐出最旧的终态任务（防无限增长）。
pub const MAX_RETAINED_TASKS: usize = 100;

/// 内存输出尾部上限（完整输出写入日志文件，内存仅保留尾部供状态快照）。
const OUTPUT_TAIL_MAX_BYTES: usize = 32 * 1024;

/// 后台命令任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTaskStatus {
    /// 运行中。
    Running,
    /// 正常退出（exit code 0）。
    Completed,
    /// 非零退出或异常终止。
    Failed,
    /// 已由 kill 终止。
    Cancelled,
}

impl CommandTaskStatus {
    /// 字符串表示（与 serde 的 snake_case 一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 后台命令终态通知器（Agent 装配层注入；默认实现把通知持久化到父会话）。
///
/// `remaining` 为父会话中仍运行中的命令任务数（join 信号）。
#[async_trait]
pub trait CommandNotifier: Send + Sync {
    /// 命令任务进入终态时调用（Completed / Failed / Cancelled）。
    async fn on_command_terminal(&self, session_id: &str, task: &CommandTask, remaining: usize);
    /// 就绪探测通过/超时通知（默认静默；实现方写入父会话）。
    async fn on_command_ready(&self, _session_id: &str, _task: &CommandTask, _note: &str) {}
}

/// 后台命令唤醒器（ADR-013 语义：全部完成/失败时触发主 agent 新一轮生成）。
///
/// 由 Agent 装配层注入（Agent 侧转发到 process_wake）；判定规则在
/// [`CommandManager`] 的 watcher：`should_wake = remaining == 0 || Failed`。
#[async_trait]
pub trait CommandWaker: Send + Sync {
    /// 唤醒指定会话的主 agent。
    async fn wake(&self, session_id: &str);
}

/// 后台命令事件通道（ADR-028：状态转移/输出增量 emit；None 时静默）。
///
/// 定义在 executor 层（不依赖 agent 层，避免循环依赖）；server 装配层
/// 用适配器接到统一事件通道。
#[async_trait]
pub trait CommandEventSink: Send + Sync {
    /// 发布命令任务事件（JSON：type=task_status / command_output）。
    async fn emit(&self, task_id: &str, event: Value);
}

/// 后台命令任务快照（状态查询 / 列表返回）。
#[derive(Debug, Clone, Serialize)]
pub struct CommandTask {
    /// 任务 ID（`cmd_` 前缀）。
    pub id: String,
    /// 原始命令。
    pub command: String,
    /// 工作目录。
    pub cwd: Option<String>,
    /// 发起任务的父会话 ID（完成通知归属）。
    pub parent_session_id: String,
    /// 状态。
    pub status: CommandTaskStatus,
    /// 退出码（Cancelled 时为 None）。
    pub exit_code: Option<i32>,
    /// 进程 PID（spawn 成功后 Some）。
    pub pid: Option<u32>,
    /// 日志文件路径（完整输出；配置了 logs_dir 时 Some）。
    pub log_file: Option<String>,
    /// 输出尾部（截断，UTF-8 安全）。
    pub output_tail: String,
    /// 创建时间（epoch 毫秒）。
    pub created_at: i64,
    /// 完成时间（epoch 毫秒）。
    pub completed_at: Option<i64>,
    /// 注册序号（单调递增，排序键）。
    pub seq: u64,
    /// 时序锚点：任务启动时链上消息数（= 下一条消息的 seq）。
    ///
    /// 回退到用户输入 U 时，锚点 > U.seq 的任务属于"回退点之后"，
    /// 应一并取消（方案 B：任务与链上时序点关联，精确取消）。
    pub anchor_seq: i64,
    /// 任务归属工作目录（创建会话的生效工作目录快照；U5：task_status 默认
    /// 视野按它过滤。命令的实际运行目录见 `cwd`——模型可显式指定，与归属
    /// 可能不同）。
    pub working_directory: Option<String>,
    /// 就绪探测是否已通过（服务可用的信号）。
    pub ready: bool,
    /// 就绪/超时说明（如 "端口 3000 已监听" / "就绪探测超时"）。
    pub ready_note: Option<String>,
    /// 是否仍在会话「待完成」计数中（session_counts）。
    ///
    /// 就绪（或探测超时）后置 false——**长驻服务的「完成」= 就绪**（B 方案）：
    /// "服务可用 / 等待结束"即任务目标达成、不再计入 remaining；否则服务
    /// 续跑会让 allComplete 永不达成，唤醒永久停滞（需人工停服破局）。
    /// 终态时仅对 counted=true 的任务递减（幂等，防重复/负数）。
    pub counted: bool,
}

/// 后台命令就绪探测规格（execute_command(background).ready 解析）。
#[derive(Debug, Clone, Default)]
pub struct ReadySpec {
    /// 就绪判定端口（TCP 连接成功即就绪）。
    pub port: Option<u16>,
    /// 就绪判定日志关键词（日志尾部出现即就绪）。
    pub pattern: Option<String>,
    /// 首次探测等待（指数退避起点，毫秒；缺省 [`DEFAULT_READY_INITIAL_DELAY_MS`]）。
    pub initial_delay_ms: u64,
    /// 探测总超时（毫秒；缺省 [`DEFAULT_READY_TIMEOUT_MS`]）。
    pub timeout_ms: u64,
}

/// 就绪探测默认首次等待（毫秒）。
pub const DEFAULT_READY_INITIAL_DELAY_MS: u64 = 500;

/// 就绪探测默认总超时（毫秒）——60s：超时仅表示"探测没等到信号"（服务继续跑），
/// 让主 agent 尽快知情比长时间干等有用（旧值 300s 使失败场景沉默 5 分钟）。
pub const DEFAULT_READY_TIMEOUT_MS: u64 = 60_000;

/// 注册表内条目（保留 + 内部标志）。
struct TaskEntry {
    task: CommandTask,
    cancelled: bool,
}

/// 单次就绪探测的结果（内部控制流；不对外暴露）。
enum ProbeOutcome {
    /// 进程已终态：停止探测（watcher 已发终态通知）。
    ProcessFinished,
    /// 未就绪：按退避继续探测。
    NotReady,
    /// 已就绪：附判定说明（写入 `ready_note` 并通知）。字段为判定说明。
    Ready(String),
}

/// 后台命令管理器（fire-and-forget 进程原语，对标 DSH 后台任务语义）。
///
/// - **立即返回**：`spawn_background` 只负责拉起进程并返回任务快照，调用方不被阻塞；
/// - **输出观测**：stdout/stderr 追加写入日志文件（logs_dir 存在时）+ 内存尾部缓冲，
///   通过 `get`/`list` 非阻塞读取尾部，完整输出用日志文件路径 + read_file 读取；
/// - **显式终止**：`kill` 按 PID 杀进程树（Windows taskkill /T；Unix 进程组 kill），
///   终态任务为幂等空操作；
/// - **防失控**：并发上限信号量 + 注册表保留上限（逐出最旧终态任务）；
/// - **不持久化（有意设计，ADR-026 §5「D 边界澄清」）**：注册表纯进程内存
///   （`MAX_RETAINED_TASKS=100` 逐出最旧非 Running），**不落 SQLite**——长会话
///   会累积大量前后台命令，全部落库并长期可见会争夺用户注意力；命令任务
///   的存活期 = 应用进程生命周期（重启即清空，属预期行为）。**勿"顺手"加持久化。**
///
/// 与 [`crate::agent::background::BackgroundTaskManager`]（委托任务）相互独立：
/// 本管理器只负责进程生命周期，不涉及会话通知/唤醒（完成通知属后续演进）。
#[derive(Clone)]
pub struct CommandManager {
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    seq: Arc<AtomicU64>,
    logs_dir: Option<PathBuf>,
    semaphore: Arc<Semaphore>,
    /// 完成通知器（Agent 装配层注入；None 时静默）。
    notifier: Option<Arc<dyn CommandNotifier>>,
    /// 唤醒器槽（ADR-013 语义：全部完成/失败时唤醒主 agent；构建后注入）。
    waker: Arc<Mutex<Option<Arc<dyn CommandWaker>>>>,
    /// 会话内运行中命令任务计数（remaining 信号）。
    session_counts: Arc<Mutex<HashMap<String, usize>>>,
    /// 命令事件通道（ADR-028：状态转移/输出增量；None 时静默）。
    event_sink: Option<Arc<dyn CommandEventSink>>,
}

impl CommandManager {
    /// 创建管理器。logs_dir 为后台命令日志目录（None 时仅内存尾部缓冲，不落盘）。
    pub fn new(logs_dir: Option<PathBuf>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
            logs_dir,
            semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_COMMANDS)),
            notifier: None,
            waker: Arc::new(Mutex::new(None)),
            session_counts: Arc::new(Mutex::new(HashMap::new())),
            event_sink: None,
        }
    }

    /// 指定并发上限（ADR-026：可配置；默认 DEFAULT_MAX_CONCURRENT_COMMANDS）。
    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Self {
        self.semaphore = Arc::new(Semaphore::new(max_concurrent.max(1)));
        self
    }

    /// 设置命令事件通道（ADR-028：状态转移/输出增量 emit；None 时静默）。
    pub fn with_event_sink(mut self, sink: Arc<dyn CommandEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// 后台启动命令：立即返回任务快照，进程在独立任务中运行。
    ///
    /// 输出写日志文件 + 内存尾部；进程退出后由 watcher 任务更新终态，
    /// 并触发完成通知（注入父会话）+ 唤醒（ADR-013 语义）。
    ///
    /// `session_id` 为发起命令的父会话（完成通知归属）。
    ///
    /// 本函数只做编排——日志准备 / 进程启动 / 输出泵 / 任务注册 / watcher /
    /// 就绪探测各自独立成段（见下方私有 helper）。
    pub async fn spawn_background(
        &self,
        session_id: &str,
        command: &str,
        cwd: Option<&str>,
        ready: Option<ReadySpec>,
        anchor_seq: i64,
        working_directory: Option<String>,
    ) -> Result<CommandTask> {
        // 并发许可：排队等待（防失控扇出；许可随 watcher 任务结束自动释放）
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| TianyanError::Custom("executor: 后台命令调度器已关闭".to_string()))?;

        let id = format!(
            "{}{}",
            COMMAND_TASK_PREFIX,
            self.seq.fetch_add(1, AtomicOrdering::SeqCst)
        );
        let now = now_ms();
        let log_file = self.prepare_command_log(&id, command, cwd).await;

        let mut child = Self::spawn_shell_child(command, cwd)?;
        let pid = child.id();

        let tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let file = Self::open_log_writer(log_file.as_ref()).await;
        let (out_reader, err_reader) = self.spawn_output_drains(&mut child, &tail, &file, &id);

        let task = CommandTask {
            id: id.clone(),
            command: command.to_string(),
            cwd: cwd.map(str::to_string),
            parent_session_id: session_id.to_string(),
            status: CommandTaskStatus::Running,
            exit_code: None,
            pid,
            log_file: log_file.map(|p| p.to_string_lossy().into_owned()),
            output_tail: String::new(),
            created_at: now,
            completed_at: None,
            seq: self.seq.load(AtomicOrdering::SeqCst) - 1,
            anchor_seq,
            working_directory,
            ready: false,
            ready_note: None,
            counted: true,
        };
        self.register_spawned_task(&task, session_id, command, pid, now)
            .await;

        self.spawn_watcher(
            child,
            out_reader,
            err_reader,
            tail.clone(),
            file,
            id.clone(),
            permit,
        );
        if let Some(spec) = ready {
            self.spawn_ready_probe(spec, tail, id);
        }

        Ok(task)
    }

    /// 生成日志文件路径并预写命令回显头（未配置日志目录时返回 `None`）。
    async fn prepare_command_log(
        &self,
        id: &str,
        command: &str,
        cwd: Option<&str>,
    ) -> Option<PathBuf> {
        let path = self
            .logs_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.log")))?;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let header = format!(
            "> $ {}{}\n",
            cwd.map(|d| format!("({d}) ")).unwrap_or_default(),
            command
        );
        let _ = tokio::fs::write(&path, header).await;
        Some(path)
    }

    /// 构造并启动 shell 子进程（stdout/stderr 管道 + `kill_on_drop`；
    /// Unix 下独立进程组，便于按进程树终止）。
    fn spawn_shell_child(command: &str, cwd: Option<&str>) -> Result<tokio::process::Child> {
        let mut cmd = build_shell_command(command);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        cmd.spawn()
            .map_err(|e| TianyanError::Custom(format!("executor: 后台命令启动失败：{e}")))
    }

    /// 打开日志文件追加句柄（失败仅告警——命令照常运行，仅少一份持久日志）。
    async fn open_log_writer(path: Option<&PathBuf>) -> Arc<Mutex<Option<tokio::fs::File>>> {
        let file: Arc<Mutex<Option<tokio::fs::File>>> = Arc::new(Mutex::new(None));
        let Some(path) = path else {
            return file;
        };
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            Ok(f) => {
                *file.lock().await = Some(f);
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "打开后台命令日志文件失败");
            }
        }
        file
    }

    /// 启动 stdout/stderr 输出泵（内存尾部 + 日志文件 + 输出增量事件）。
    fn spawn_output_drains(
        &self,
        child: &mut tokio::process::Child,
        tail: &Arc<Mutex<String>>,
        file: &Arc<Mutex<Option<tokio::fs::File>>>,
        id: &str,
    ) -> (
        Option<tokio::task::JoinHandle<()>>,
        Option<tokio::task::JoinHandle<()>>,
    ) {
        let out = child.stdout.take().map(|r| {
            tokio::spawn(drain_output(
                r,
                tail.clone(),
                file.clone(),
                id.to_string(),
                self.event_sink.clone(),
            ))
        });
        let err = child.stderr.take().map(|r| {
            tokio::spawn(drain_output(
                r,
                tail.clone(),
                file.clone(),
                id.to_string(),
                self.event_sink.clone(),
            ))
        });
        (out, err)
    }

    /// 注册刚启动的任务：快照入库 + 会话计数 + 容量淘汰 + running 状态事件。
    async fn register_spawned_task(
        &self,
        task: &CommandTask,
        session_id: &str,
        command: &str,
        pid: Option<u32>,
        now: i64,
    ) {
        self.tasks.lock().await.insert(
            task.id.clone(),
            TaskEntry {
                task: task.clone(),
                cancelled: false,
            },
        );
        *self
            .session_counts
            .lock()
            .await
            .entry(session_id.to_string())
            .or_insert(0) += 1;
        self.evict_if_needed().await;

        // ADR-028：状态转移事件（running）——面板实时显示
        if let Some(sink) = &self.event_sink {
            sink.emit(
                &task.id,
                json!({
                    "type": "task_status",
                    "task_id": task.id,
                    "kind": "command",
                    "status": "running",
                    "command": command,
                    "pid": pid,
                    "created_at": now,
                }),
            )
            .await;
        }
    }

    /// 启动 watcher 任务：等进程退出 → 收输出泵 → 落终态 → 通知/唤醒。
    ///
    /// 并发许可（`permit`）**移交本任务持有**，随任务结束（进程退出 → 收输出泵 →
    /// 落终态 → 通知）释放。T1-5 修复：此前它是 `spawn_background` 的局部变量，
    /// 函数返回即被提前释放——命令仍在运行但许可已归还，并发上限形同虚设。
    #[allow(clippy::too_many_arguments)]
    fn spawn_watcher(
        &self,
        mut child: tokio::process::Child,
        out_reader: Option<tokio::task::JoinHandle<()>>,
        err_reader: Option<tokio::task::JoinHandle<()>>,
        tail: Arc<Mutex<String>>,
        file: Arc<Mutex<Option<tokio::fs::File>>>,
        id: String,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            // T1-5：许可随本任务存活——命令真正结束（进程退出并收尾）才释放
            let _permit = permit;
            // 注册子进程（关停时统一清理；watcher 结束自动注销）
            let _child_guard = ChildGuard::new(child.id());
            let wait_result = child.wait().await;
            if let Some(h) = out_reader {
                let _ = h.await;
            }
            if let Some(h) = err_reader {
                let _ = h.await;
            }
            let exit_code = wait_result.ok().and_then(|s| s.code());

            let snapshot = manager
                .finalize_finished_task(&id, exit_code, &tail, &file)
                .await;
            // 锁外收尾：remaining 计数 → 完成通知 → 唤醒（ADR-013）
            if let Some(task) = snapshot {
                manager.notify_task_terminal(task).await;
            }
        });
    }

    /// 落终态：写退出标记（内存尾部 + 日志文件）→ 更新状态 → 返回任务快照。
    ///
    /// 锁顺序固定为 tasks → tail → file（与输出泵一致，避免死锁）。
    async fn finalize_finished_task(
        &self,
        id: &str,
        exit_code: Option<i32>,
        tail: &Arc<Mutex<String>>,
        file: &Arc<Mutex<Option<tokio::fs::File>>>,
    ) -> Option<CommandTask> {
        let mut tasks = self.tasks.lock().await;
        let is_cancelled = tasks.get(id).map(|e| e.cancelled).unwrap_or(false);
        let marker = if is_cancelled {
            "--- terminated (killed) ---\n".to_string()
        } else {
            format!("--- exit code: {} ---\n", exit_code.unwrap_or(-1))
        };
        {
            let mut t = tail.lock().await;
            t.push_str(&marker);
            crate::common::truncate::truncate_keep_tail_bytes(&mut t, OUTPUT_TAIL_MAX_BYTES);
            let mut f = file.lock().await;
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(marker.as_bytes()).await;
            }
        }
        if let Some(entry) = tasks.get_mut(id) {
            if !is_cancelled {
                entry.task.status = if exit_code == Some(0) {
                    CommandTaskStatus::Completed
                } else {
                    CommandTaskStatus::Failed
                };
                entry.task.exit_code = exit_code;
            }
            entry.task.completed_at = Some(now_ms());
            entry.task.output_tail = tail.lock().await.clone();
        }
        tasks.get(id).map(|e| e.task.clone())
    }

    /// 终态收尾（锁外）：remaining 计数 → 完成通知 → 终态事件 → 唤醒。
    ///
    /// `should_wake = allComplete || failure`（部分完成保持静默，ADR-013）。
    async fn notify_task_terminal(&self, task: CommandTask) {
        let session_id = task.parent_session_id.clone();
        let remaining = {
            let mut counts = self.session_counts.lock().await;
            // counted=false 的任务（就绪/超时已释放）不再递减——防重复/负数。
            let cur = counts.get(&session_id).copied().unwrap_or(0);
            let n = if task.counted {
                cur.saturating_sub(1)
            } else {
                cur
            };
            counts.insert(session_id.clone(), n);
            n
        };
        if let Some(notifier) = &self.notifier {
            notifier
                .on_command_terminal(&session_id, &task, remaining)
                .await;
        }
        // ADR-028：状态转移事件（终态）——面板实时更新
        if let Some(sink) = &self.event_sink {
            sink.emit(
                &task.id,
                json!({
                    "type": "task_status",
                    "task_id": task.id,
                    "kind": "command",
                    "status": task.status.as_str(),
                    "exit_code": task.exit_code,
                    "completed_at": task.completed_at,
                }),
            )
            .await;
        }
        if remaining == 0 || task.status == CommandTaskStatus::Failed {
            if let Some(waker) = self.waker.lock().await.clone() {
                waker.wake(&session_id).await;
            }
        }
    }

    /// 启动就绪探测任务：端口监听 / 日志关键词命中即通知就绪。
    ///
    /// 指数退避（起点 `initial_delay_ms`，×2）至总超时后通知未就绪——
    /// 不杀进程（服务可能仍在启动，退出由 watcher 另行通知）。
    fn spawn_ready_probe(&self, spec: ReadySpec, tail: Arc<Mutex<String>>, task_id: String) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut delay_ms = spec.initial_delay_ms.max(50);
            let start = now_ms();
            let timeout_ms = spec.timeout_ms.max(delay_ms);
            loop {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                match Self::probe_readiness(&manager, &task_id, &spec, &tail).await {
                    // 进程已退出：停止探测（watcher 已发终态通知）
                    ProbeOutcome::ProcessFinished => break,
                    ProbeOutcome::NotReady => {}
                    ProbeOutcome::Ready(note) => {
                        Self::mark_ready(&manager, &task_id, Some(note.clone())).await;
                        Self::release_wait_count(&manager, &task_id).await;
                        Self::notify_ready(&manager, &task_id, &note).await;
                        break;
                    }
                }
                // 超时：通知未就绪（不杀进程）
                if now_ms() - start >= timeout_ms as i64 {
                    let note = format!("就绪探测超时（{} ms）", timeout_ms);
                    Self::mark_ready(&manager, &task_id, Some(note.clone())).await;
                    // 超时同样"等待结束"：释放计数（避免探测失败的服务
                    // 永久占住 remaining 造成唤醒停滞）；服务本体继续跑。
                    Self::release_wait_count(&manager, &task_id).await;
                    Self::notify_ready(&manager, &task_id, &note).await;
                    break;
                }
                // 指数退避：×2（上限 30s）
                delay_ms = (delay_ms * 2).min(30_000);
            }
        });
    }

    /// 单次就绪探测：进程终态 → 停止；端口连通 / 关键词命中 → 就绪。
    async fn probe_readiness(
        manager: &CommandManager,
        task_id: &str,
        spec: &ReadySpec,
        tail: &Arc<Mutex<String>>,
    ) -> ProbeOutcome {
        let terminal = manager
            .tasks
            .lock()
            .await
            .get(task_id)
            .map(|e| e.task.status != CommandTaskStatus::Running)
            .unwrap_or(true);
        if terminal {
            return ProbeOutcome::ProcessFinished;
        }
        // 判定 1：端口 TCP 连接成功
        let port_ok = match spec.port {
            Some(port) => tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok(),
            None => false,
        };
        // 判定 2：日志尾部关键词
        let pattern_ok = if let Some(p) = spec.pattern.as_deref() {
            if p.is_empty() {
                false
            } else {
                tail.lock().await.contains(p)
            }
        } else {
            false
        };
        if port_ok || pattern_ok {
            let note = if port_ok {
                format!("端口 {} 已监听", spec.port.unwrap_or(0))
            } else {
                format!(
                    "日志出现关键词 \"{}\"",
                    spec.pattern.as_deref().unwrap_or("")
                )
            };
            return ProbeOutcome::Ready(note);
        }
        ProbeOutcome::NotReady
    }

    /// 标记任务就绪状态（就绪 / 超时说明）。
    async fn mark_ready(manager: &CommandManager, task_id: &str, note: Option<String>) {
        if let Some(entry) = manager.tasks.lock().await.get_mut(task_id) {
            entry.task.ready = true;
            entry.task.ready_note = note;
        }
    }

    /// 就绪/超时通知 + 唤醒（就绪是重要事件：主 agent 可开始后续工作；
    /// 超时同样需要主 agent 知情）。
    async fn notify_ready(manager: &CommandManager, task_id: &str, note: &str) {
        let task = manager.get(task_id).await;
        let Some(task) = task else {
            return;
        };
        let session_id = task.parent_session_id.clone();
        if let Some(notifier) = &manager.notifier {
            notifier.on_command_ready(&session_id, &task, note).await;
        }
        if let Some(waker) = manager.waker.lock().await.clone() {
            waker.wake(&session_id).await;
        }
    }

    /// 就绪/超时后：把任务移出会话「待完成」计数（幂等）。
    ///
    /// **B 方案（长驻服务的「完成」= 就绪）**：就绪（或探测超时=等待结束）
    /// 后该任务不再计入 `session_counts`——服务进程照跑，但"全部完成"
    /// 不再被它挡住；若其余任务均已完成，立即触发唤醒（主 agent 可收尾）。
    /// 就绪后的服务仍受 watcher 监视：失败/退出照常出终态通知（失败唤醒）。
    async fn release_wait_count(manager: &CommandManager, task_id: &str) {
        let session_id = {
            let mut tasks = manager.tasks.lock().await;
            let Some(entry) = tasks.get_mut(task_id) else {
                return;
            };
            if !entry.task.counted {
                return; // 幂等：已释放
            }
            entry.task.counted = false;
            entry.task.parent_session_id.clone()
        };
        let remaining = {
            let mut counts = manager.session_counts.lock().await;
            let n = counts
                .get(&session_id)
                .copied()
                .unwrap_or(0)
                .saturating_sub(1);
            counts.insert(session_id.clone(), n);
            n
        };
        if remaining == 0 {
            if let Some(waker) = manager.waker.lock().await.clone() {
                waker.wake(&session_id).await;
            }
        }
    }

    /// 按任务 ID 获取快照（None 表示不存在）。
    pub async fn get(&self, task_id: &str) -> Option<CommandTask> {
        self.tasks.lock().await.get(task_id).map(|e| e.task.clone())
    }

    /// 全部任务快照（按注册序号升序）。
    pub async fn list(&self) -> Vec<CommandTask> {
        let tasks = self.tasks.lock().await;
        let mut v: Vec<CommandTask> = tasks.values().map(|e| e.task.clone()).collect();
        v.sort_by_key(|t| t.seq);
        v
    }

    /// 终止运行中的后台命令（杀进程树）。终态任务为幂等空操作。
    pub async fn kill(&self, task_id: &str) -> Result<()> {
        let pid = {
            let mut tasks = self.tasks.lock().await;
            let entry = tasks
                .get_mut(task_id)
                .ok_or_else(|| TianyanError::Custom(format!("executor: 任务不存在：{task_id}")))?;
            if entry.task.status != CommandTaskStatus::Running {
                return Ok(());
            }
            entry.cancelled = true;
            entry.task.status = CommandTaskStatus::Cancelled;
            entry.task.completed_at = Some(now_ms());
            entry.task.pid
        };
        if let Some(pid) = pid {
            kill_process_tree(pid).await;
        }
        Ok(())
    }

    /// 设置完成通知器（Agent 装配层注入；None 时静默）。
    pub fn with_notifier(mut self, notifier: Arc<dyn CommandNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// 设置唤醒器（构建后注入；Agent 构建完成后注册自引用转发器）。
    pub async fn set_waker(&self, waker: Arc<dyn CommandWaker>) {
        *self.waker.lock().await = Some(waker);
    }

    /// 注册表保留上限：逐出最旧的终态任务。
    async fn evict_if_needed(&self) {
        let mut tasks = self.tasks.lock().await;
        if tasks.len() <= MAX_RETAINED_TASKS {
            return;
        }
        let mut candidates: Vec<(u64, String)> = tasks
            .iter()
            .filter(|(_, e)| e.task.status != CommandTaskStatus::Running)
            .map(|(id, e)| (e.task.seq, id.clone()))
            .collect();
        candidates.sort_by_key(|(seq, _)| *seq);
        let excess = tasks.len() - MAX_RETAINED_TASKS;
        for (_, id) in candidates.into_iter().take(excess) {
            tasks.remove(&id);
        }
    }
}

/// 跨平台 shell 包装命令构造（Unix: `sh -c`，Windows: `powershell`）。
///
/// Windows：隐藏子进程控制台窗口（CREATE_NO_WINDOW），避免执行命令时
/// 弹出黑窗口一闪而过；Unix 无操作。
///
/// `mut` 仅 Windows 分支需要（`creation_flags` 取 `&mut self`）——Linux 上
/// 该分支被裁掉，`unused_mut` 会撞 CI 的 `-D warnings`（2026-09-17 CI 实测）。
#[cfg_attr(not(windows), allow(unused_mut))]
pub(crate) fn hide_console_window(mut cmd: tokio::process::Command) -> tokio::process::Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    cmd
}

fn build_shell_command(command: &str) -> tokio::process::Command {
    // 执行底层由 `crate::executor::shell` 的全局 spec 决定（auto 探测 /
    // 用户配置——ADR-037）；输出编码注入（PS 系 UTF-8）与 Windows 黑窗抑制
    // 在 `ShellSpec::build_command` 内统一处理。
    crate::executor::shell::current().build_command(command)
}

/// 按 PID 杀进程树（Windows: taskkill /T /F；Unix: 进程组 kill -9 -pgid）。
///
/// 运行中的命令子进程注册表（前台 + 后台统一）——服务关停时按进程树统一清理。
///
/// 为什么需要：父子进程无 Job Object 绑定，父进程退出不会带走子进程；用户实测
/// 退出后任务管理器仍残留 cargo 等子进程。取消标志只能覆盖“读它的”前台命令，
/// 后台命令（`spawn_background`）不被会话取消槽覆盖，必须显式杀。
static RUNNING_CHILDREN: std::sync::OnceLock<dashmap::DashMap<u32, ()>> =
    std::sync::OnceLock::new();

/// 取全局子进程注册表（首次访问初始化）。
fn running_children() -> &'static dashmap::DashMap<u32, ()> {
    RUNNING_CHILDREN.get_or_init(dashmap::DashMap::new)
}

/// 注册子进程 pid。
fn register_child(pid: u32) {
    running_children().insert(pid, ());
}

/// 注销子进程 pid。
fn unregister_child(pid: u32) {
    running_children().remove(&pid);
}

/// 子进程注册守卫：持有期间 pid 在注册表中，drop 即注销。
struct ChildGuard(Option<u32>);

impl ChildGuard {
    fn new(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            register_child(pid);
        }
        Self(pid)
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            unregister_child(pid);
        }
    }
}

/// 杀掉**所有**运行中的命令子进程（按进程树）。返回处理的进程数。
///
/// 服务关停流程调用（`AppState::shutdown`）——退出不必等命令跑完，
/// 也不把子进程留给用户手动清理。
pub async fn kill_all_running_children() -> usize {
    let pids: Vec<u32> = running_children().iter().map(|e| *e.key()).collect();
    for pid in &pids {
        kill_process_tree(*pid).await;
    }
    pids.len()
}

/// Unix 侧依赖 spawn 时 `process_group(0)`：子进程成为新进程组组长，
/// 负 PID 信号作用于整个组——shell 派生的服务进程一并终止（对齐
/// opencode #30868 教训：只杀 shell 会留孤儿服务进程）。
async fn kill_process_tree(pid: u32) {
    if cfg!(target_os = "windows") {
        match hide_console_window(tokio::process::Command::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(s) if !s.success() => tracing::warn!(pid, "taskkill 未报告成功（进程可能已退出）"),
            Err(e) => tracing::warn!(pid, error = %e, "终止进程树失败"),
            Ok(_) => {}
        }
        return;
    }
    // Unix：三重保险（进程组 → 进程本身 → 直接子进程）。
    // WSL/Linux 实测：`kill -9 -<pgid>` 可能返回成功但目标仍存活（该环境
    // 进程组语义不可靠）——逐级补刀，任一命中即可；pkill 缺失时忽略其错误。
    let mut killed = false;
    for args in [
        vec!["-9".to_string(), format!("-{pid}")],
        vec!["-9".to_string(), pid.to_string()],
    ] {
        match tokio::process::Command::new("kill")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(s) if s.success() => killed = true,
            Ok(_) => {}
            Err(e) => tracing::warn!(pid, error = %e, "kill 执行失败"),
        }
    }
    let _ = tokio::process::Command::new("pkill")
        .args(["-9", "-P", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    if !killed {
        tracing::warn!(pid, "终止进程树：kill 未报告成功");
    }
}

/// 输出增量事件的最小合并间隔（ADR-028：时间窗节流——高频命令（构建/日志
/// 滚动）每读一块就 emit 会造成事件风暴，无谓占用有界事件通道）。
const OUTPUT_EVENT_FLUSH_MS: i64 = 100;

/// 读取子进程输出流：追加到内存尾部缓冲（截断）+ 日志文件。
///
/// 锁顺序固定为 tail → file（与 watcher 的标记写入一致，避免死锁）。
/// ADR-028：输出增量按时间窗（[`OUTPUT_EVENT_FLUSH_MS`]）合并后经 event_sink
/// emit——尽力而为（通道满丢弃），完整性由日志文件保证；**流结束必须 flush
/// 残留增量**，否则面板会丢尾块。
async fn drain_output<R>(
    mut reader: R,
    tail: Arc<Mutex<String>>,
    file: Arc<Mutex<Option<tokio::fs::File>>>,
    task_id: String,
    event_sink: Option<Arc<dyn CommandEventSink>>,
) where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 8192];
    // 跨块的多字节字符残片（≤3 字节）：本块尾部被切断的字符留到下一块拼接，
    // 避免 8KB 读边界把中文替换为 U+FFFD（每块末尾必现"半字符"的根因）。
    let mut carry: Vec<u8> = Vec::new();
    let mut pending = String::new();
    let mut last_emit = now_ms();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                // 原始字节写日志文件（字节流保真，不受解码影响）
                let mut f = file.lock().await;
                if let Some(f) = f.as_mut() {
                    if f.write_all(chunk).await.is_err() {
                        tracing::warn!("写入后台命令日志失败");
                        break;
                    }
                }
                drop(f);
                // 文本视图：carry + 本块 → 可完整解码前缀（不完整序列残片留 carry）
                carry.extend_from_slice(chunk);
                let (text, rest) = decode_utf8_keep_tail(&carry);
                carry = rest;
                if !text.is_empty() {
                    {
                        let mut t = tail.lock().await;
                        t.push_str(&text);
                        crate::common::truncate::truncate_keep_tail_bytes(
                            &mut t,
                            OUTPUT_TAIL_MAX_BYTES,
                        );
                    }
                    // ADR-028：输出增量事件（时间窗合并；实时终端视图，尽力而为）
                    pending.push_str(&text);
                }
                let now = now_ms();
                if now - last_emit >= OUTPUT_EVENT_FLUSH_MS {
                    flush_output_event(&event_sink, &task_id, &mut pending).await;
                    last_emit = now;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "读取后台命令输出失败");
                break;
            }
        }
    }
    // 收尾 1：流以不完整序列结束（真截断/非法字节）→ lossy 兜底一次
    if !carry.is_empty() {
        let text = String::from_utf8_lossy(&carry).into_owned();
        if !text.is_empty() {
            let mut t = tail.lock().await;
            t.push_str(&text);
            crate::common::truncate::truncate_keep_tail_bytes(&mut t, OUTPUT_TAIL_MAX_BYTES);
            drop(t);
            pending.push_str(&text);
        }
    }
    // 收尾 2：残留增量必须发出（含写日志失败 / 读错误提前 break 的路径）
    flush_output_event(&event_sink, &task_id, &mut pending).await;
}

/// 解码字节流的可完整解码前缀：返回（已解码文本，残余字节）。
///
/// - 合法文本全部解码；
/// - 尾部不完整的多字节序列（跨块被切断，≤3 字节）→ 留作残余，待下个块拼接；
/// - 非法序列（非"被切断"）→ 立即 lossy 替换（�）并继续推进，不积累残余。
fn decode_utf8_keep_tail(bytes: &[u8]) -> (String, Vec<u8>) {
    let mut text = String::new();
    let mut rest: &[u8] = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                text.push_str(s);
                return (text, Vec::new());
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // 安全：valid_up_to 保证前缀是合法 UTF-8
                text.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                match e.error_len() {
                    // 尾部不完整序列：保留残余（可能跨块被切分的多字节字符）
                    None => return (text, rest[valid..].to_vec()),
                    // 非法序列：lossy 替换并跳过，不积累
                    Some(bad) => {
                        text.push('\u{FFFD}');
                        rest = &rest[valid + bad..];
                    }
                }
            }
        }
    }
}

/// 发送累积的输出增量并清空缓冲（空内容不发；无 sink 时只清空）。
async fn flush_output_event(
    event_sink: &Option<Arc<dyn CommandEventSink>>,
    task_id: &str,
    pending: &mut String,
) {
    if pending.is_empty() {
        return;
    }
    if let Some(sink) = event_sink {
        sink.emit(
            task_id,
            json!({
                "type": "command_output",
                "task_id": task_id,
                "delta": pending.as_str(),
            }),
        )
        .await;
    }
    pending.clear();
}

/// 当前 epoch 毫秒。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn read_reader_to_vec<R>(mut reader: R) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    if let Err(e) = reader.read_to_end(&mut buf).await {
        tracing::warn!(error = %e, "读取命令输出失败");
    }
    buf
}

pub(super) fn extract_command_base(cmd: &str) -> String {
    let name = cmd.split_whitespace().next().unwrap_or(cmd);
    let file_name = name.rsplit(&['\\', '/'][..]).next().unwrap_or(name);
    let pure = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".bat"))
        .or_else(|| file_name.strip_suffix(".cmd"))
        .unwrap_or(file_name);
    pure.to_lowercase()
}

/// 完整命令输出落盘（best effort）：`{log_dir}/exec-{uuid8}.log`。
///
/// 格式：`> $ (cwd) command` header + stdout 全文（stderr 非空时追加 `[stderr]` 分节）。
/// 目录未配置或写失败时返回 None（仅告警，不影响命令结果与截断路径）。
async fn write_full_output_log(
    log_dir: Option<&Path>,
    command: &str,
    cwd: Option<&str>,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    let dir = log_dir?;
    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "命令输出落盘：目录创建失败");
        return None;
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join(format!("exec-{}.log", &id[..8]));
    let mut content = format!(
        "> $ {}{}\n",
        cwd.map(|d| format!("({d}) ")).unwrap_or_default(),
        command
    );
    content.push_str(stdout);
    if !stderr.is_empty() {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str("[stderr]\n");
        content.push_str(stderr);
    }
    match tokio::fs::write(&path, content).await {
        Ok(()) => Some(path.to_string_lossy().into_owned()),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "命令输出落盘失败（结果不受影响）");
            None
        }
    }
}

/// 命令等待结果（`Cancelled` 与 `TimedOut` 分开——取消不是超时）。
enum CommandWaitOutcome {
    Exited(std::process::ExitStatus),
    Failed(String),
    TimedOut,
    Cancelled,
}

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
///
/// 跨平台：Unix (Linux/macOS) 用 `sh -c`，Windows 用 `powershell -NoProfile -NonInteractive -Command`。
/// 超时后**杀进程树**（不只 shell）：Windows taskkill /T /F，Unix 进程组 kill，
/// 避免派生服务进程残留为孤儿（opencode #30868 同款问题）。
///
/// 输出经统一截断层**尾部截断**（50KB / 2000 行；`stdout_truncated`/
/// `stderr_truncated` 标志），防大输出（实测单条 45 万字符）压垮上下文。
///
/// `log_dir` 配置时：**截断前**将完整输出（header + stdout + stderr 分节）
/// 落盘（best effort），结果附 `log_file` / `*_total_bytes`；截断标记指引
/// 路径——模型可经 `read_file` 的 offset/limit 回读完整内容（消除"截断即丢失"）。
pub async fn execute_command_action(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    log_dir: Option<&Path>,
) -> Result<Value> {
    execute_command_action_cancellable(command, cwd, timeout_secs, log_dir, None).await
}

/// 同 [`execute_command_action`]，但支持**外部取消**（用户「停止」按钮）。
///
/// `cancel` 置位时：杀进程树 → 返回 `executor: 命令已取消`（**不**标 timeout——
/// 取消不是超时）。取消检查粒度 50ms；正常退出路径仍是零延迟等待。
pub async fn execute_command_action_cancellable(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    log_dir: Option<&Path>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Value> {
    let timeout = timeout_secs.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);

    let mut cmd = build_shell_command(command);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let started_at = tokio::time::Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| TianyanError::Custom(format!("executor: 命令执行失败：{}", e)))?;

    let stdout_handle = child
        .stdout
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));

    // 注册子进程（关停时按进程树统一清理；命令结束 drop 自动注销）
    let _child_guard = ChildGuard::new(child.id());

    // 等待退出：正常退出 / 超时 / **用户取消** 三路竞争。
    // 「停止」此前只在轮顶与 chunk 循环生效——长命令执行期间点停止毫无反应。
    //
    // 实现用 **try_wait 轮询**而非 select!：Linux 上 select! + `child.wait()`
    // 组合实测取消标志置位后仍等满命令时长（WSL/CI：30s 未中断，Windows 未复现），
    // 轮询实现跨平台语义一致、不依赖 waker 语义（代价：正常结束最多多 20ms 观测延迟）。
    let outcome = loop {
        if let Some(flag) = cancel.as_ref() {
            if flag.load(AtomicOrdering::Relaxed) {
                break CommandWaitOutcome::Cancelled;
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break CommandWaitOutcome::Exited(status),
            Ok(None) => {}
            Err(e) => break CommandWaitOutcome::Failed(format!("{e}")),
        }
        if started_at.elapsed() >= Duration::from_secs(timeout) {
            break CommandWaitOutcome::TimedOut;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    match outcome {
        CommandWaitOutcome::Exited(status) => {
            let stdout_bytes = if let Some(handle) = stdout_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stdout 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let stderr_bytes = if let Some(handle) = stderr_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stderr 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            // 输出体量治理：命令输出原样可达数十万字符——尾部截断（错误/摘要
            // 通常在尾部；50KB / 2000 行双上限），防单条结果压垮上下文。
            let stdout_text = String::from_utf8_lossy(&stdout_bytes).to_string();
            let stderr_text = String::from_utf8_lossy(&stderr_bytes).to_string();
            // 截断前先把完整输出落盘（best effort）——截断标记附路径指引，
            // 模型可按需回读完整内容（read_file 分页），截掉的部分不再"永久丢失"。
            let log_file =
                write_full_output_log(log_dir, command, cwd, &stdout_text, &stderr_text).await;
            let note = match &log_file {
                Some(path) => {
                    format!("，完整输出已保存至 {path}（可用 read_file 的 offset/limit 分页读取）")
                }
                None => String::new(),
            };
            let out_trunc = truncate::truncate_tail_noted(&stdout_text, &note);
            let err_trunc = truncate::truncate_tail_noted(&stderr_text, &note);
            Ok(json!({
                "stdout": out_trunc.text,
                "stderr": err_trunc.text,
                "stdout_truncated": out_trunc.truncated,
                "stderr_truncated": err_trunc.truncated,
                "stdout_total_bytes": out_trunc.total_bytes,
                "stderr_total_bytes": err_trunc.total_bytes,
                "log_file": log_file,
                "exit_code": status.code().unwrap_or(-1),
            }))
        }
        CommandWaitOutcome::Failed(e) => Err(TianyanError::Custom(format!(
            "executor: 命令执行失败：{}",
            e
        ))),
        CommandWaitOutcome::TimedOut => {
            // 杀进程树（taskkill /T 或 kill 逐级补刀），再回收子进程
            if let Some(pid) = child.id() {
                kill_process_tree(pid).await;
            }
            // 回收加 2s 兜底：杀信号已发，但某些环境 wait 不返回
            if tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                tracing::warn!("等待超时子进程退出超时（2s），放弃回收");
            }
            Err(TianyanError::timeout("executor: 执行超时"))
        }
        CommandWaitOutcome::Cancelled => {
            // 同超时路径：先杀进程树再回收（2s 兜底），避免派生进程残留为孤儿
            if let Some(pid) = child.id() {
                kill_process_tree(pid).await;
            }
            if tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                tracing::warn!("等待被取消命令退出超时（2s），放弃回收");
            }
            Err(TianyanError::Custom(
                "executor: 命令已取消（用户停止）".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    /// 全局子进程注册表测试互斥。
    ///
    /// 背景（2026-09-19 WSL/CI 实测）：`kill_all_running_children` 按注册表杀
    /// **全部**子进程（生产语义 = 关停清理）——并行测试下会误杀兄弟测试的命令，
    /// 导致取消类测试偶发走 Exited 分支（exit_code=-1）、落盘类测试命令跑不完
    /// （stderr 空 → 无分节）。凡启动真实子进程或操作注册表的测试都持锁串行，
    /// 其余测试保持并行。
    static CHILD_REGISTRY_LOCK: Mutex<()> = Mutex::const_new(());

    /// 有限轮询等待任务进入终态（避免测试卡死）。
    async fn wait_terminal(manager: &CommandManager, id: &str) -> CommandTask {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let t = manager.get(id).await.expect("任务应存在");
            if t.status != CommandTaskStatus::Running {
                return t;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "后台任务未在期限内进入终态"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn test_execute_command_cancelled_by_flag() {
        // T1：工具执行期可中断——取消标志置位后命令应在轮询粒度内被杀死并返回取消错误
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            flag.store(true, AtomicOrdering::Relaxed);
        });
        // 确定性慢命令（不依赖 ping 的网络行为/耗时）
        let cmd = if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let started = tokio::time::Instant::now();
        let err = execute_command_action_cancellable(cmd, None, Some(60), None, Some(cancel))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            err.to_string().contains("已取消"),
            "取消应返回取消错误：{err}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "取消应在 5s 内生效（实际 {elapsed:?}）"
        );
        assert!(!err.is_timeout(), "取消不应被分类为超时");
    }

    #[tokio::test]
    async fn test_running_child_registry_register_and_unregister() {
        // T1：关停清理的基础——子进程运行期注册、结束后自动注销，且可定向终止。
        // 不调用全局 kill_all：并行测试下它会误杀同批其他测试的子进程。
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let before: std::collections::HashSet<u32> =
            running_children().iter().map(|e| *e.key()).collect();
        // 确定性慢命令 + **局部取消标志**结束自己（不调 kill_process_tree：
        // 并行测试下按 pid 差集取到的可能是别人的进程，会误杀其他测试的命令）
        let cancel = Arc::new(AtomicBool::new(false));
        let cmd = if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let cancel_for_task = cancel.clone();
        let handle = tokio::spawn(async move {
            execute_command_action_cancellable(cmd, None, Some(60), None, Some(cancel_for_task))
                .await
        });
        // 等本测试的子进程注册（相对基线取差集，不受并发测试影响）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut own: Vec<u32> = Vec::new();
        while own.is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            own = running_children()
                .iter()
                .map(|e| *e.key())
                .filter(|p| !before.contains(p))
                .collect();
        }
        assert!(
            !own.is_empty(),
            "运行中的命令子进程应注册到注册表（差集非空）"
        );

        // 用**局部**取消标志结束自己的命令（不碰全局进程状态）
        cancel.store(true, AtomicOrdering::Relaxed);
        let _ = handle.await.expect("任务应正常结束");

        // 注销：结束后自己的 pid 应移出注册表
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while own.iter().any(|p| running_children().contains_key(p))
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        for pid in &own {
            assert!(
                !running_children().contains_key(pid),
                "命令结束后应自动注销 pid={pid}"
            );
        }
    }

    #[tokio::test]
    async fn test_kill_all_running_children_counts_registered() {
        // kill_all 的契约：把注册项计入处理数（真实杀进程行为由定向测试与
        // 超时路径覆盖）。用不可能的假 pid 验证遍历/计数，避免误杀并行测试。
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 持锁期间注册表内没有兄弟测试的进程
        let fake: u32 = 999_999_999;
        register_child(fake);
        let killed = kill_all_running_children().await;
        unregister_child(fake);
        assert!(killed >= 1, "应把注册项计入处理数：{killed}");
        assert!(!running_children().contains_key(&fake), "假 pid 应已注销");
    }

    #[tokio::test]
    async fn test_execute_command() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let result = execute_command_action("echo Hello", None, Some(5), None).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output["stdout"].as_str().unwrap_or("").contains("Hello"));
    }

    #[tokio::test]
    async fn test_execute_command_output_truncated() {
        // 输出体量治理：超限输出按尾部截断（统一截断标记 + 显式标志）。
        // Windows 走 powershell、Unix 走 sh——各自构造产生 >50KB 输出的命令。
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let cmd = if cfg!(target_os = "windows") {
            "1..3000 | ForEach-Object { 'A' * 30 }"
        } else {
            "yes AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA | head -n 3000"
        };
        let output = execute_command_action(cmd, None, Some(30), None)
            .await
            .unwrap();
        let stdout = output["stdout"].as_str().unwrap_or("");
        assert_eq!(
            output["stdout_truncated"].as_bool(),
            Some(true),
            "超限输出应标记截断"
        );
        assert!(
            output["log_file"].is_null(),
            "未配置日志目录时不应返回 log_file"
        );
        assert!(
            output["stdout_total_bytes"].as_u64().unwrap_or(0) > 50 * 1024,
            "应返回原始总量，供判断截掉多少"
        );
        assert!(
            stdout.contains("输出已截断"),
            "应包含统一截断标记：{stdout:?}"
        );
        assert!(
            stdout.len() < 60 * 1024,
            "截断后体量应受限于预算：{}",
            stdout.len()
        );
    }
    #[tokio::test]
    async fn test_execute_command_spills_full_output_when_dir_configured() {
        // 截断 + 落盘闭环：大输出尾部截断后，完整输出（含被截掉的头部）在
        // 落盘文件中可回读（read_file 分页）；截断标记指引路径。
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let dir = tempfile::tempdir().unwrap();
        let cmd = if cfg!(target_os = "windows") {
            "1..3000 | ForEach-Object { $p = if ($_ -le 1500) { 'HEADER' } else { 'TAIL' }; \"$p-\" + ('x' * 40) + \"-$_\" }; [Console]::Error.WriteLine('ERR-MARKER-42')"
        } else {
            "for i in $(seq 1 3000); do if [ $i -le 1500 ]; then p=HEADER; else p=TAIL; fi; echo \"$p-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx-$i\"; done; echo 'ERR-MARKER-42' 1>&2"
        };
        let output = execute_command_action(cmd, None, Some(30), Some(dir.path()))
            .await
            .unwrap();
        assert_eq!(
            output["stdout_truncated"].as_bool(),
            Some(true),
            "超限应截断"
        );
        let total = output["stdout_total_bytes"].as_u64().unwrap();
        assert!(total > 50 * 1024, "应返回原始总量：{total}");
        let log_file = output["log_file"]
            .as_str()
            .expect("配置目录后应返回 log_file");
        let content = tokio::fs::read_to_string(log_file).await.unwrap();
        // 判别核心：被截掉的头部（HEADER）只在落盘文件里、不在返回文本里
        assert!(content.contains("HEADER-"), "落盘文件应含被截掉的头部行");
        assert!(
            content.len() > total as usize,
            "文件应含完整 stdout（及 header）：{} > {total}",
            content.len()
        );
        assert!(
            content.contains("[stderr]") && content.contains("ERR-MARKER-42"),
            "落盘文件应含 stderr 分节"
        );
        let stdout = output["stdout"].as_str().unwrap();
        assert!(stdout.contains("完整输出已保存至"), "截断标记应指引路径");
        assert!(!stdout.contains("HEADER-"), "返回文本不应含被截掉的头部");
    }

    #[tokio::test]
    async fn test_command_timeout_is_timeout_error() {
        // 子进程运行时长超过超时阈值：Windows 用 ping 计数（约 4s），Unix 用 sleep
        // 确定性慢命令（不依赖 ping 的网络行为——CI/并发下 ping 可能快速失败）
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let cmd = if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        };
        // 并行全量下 Windows PowerShell 偶发启动即失败（exit 1、无输出）——那与
        // "超时语义失效"不同（后者每次都会返回 Ok），故最多重试 3 次：任一次
        // 得到 timeout 即通过；三次都提前退出才算失败（判别力保留）。
        let mut last: Option<String> = None;
        for _ in 0..3 {
            match execute_command_action(cmd, None, Some(1), None).await {
                Err(e) if e.is_timeout() => return,
                Ok(v) => last = Some(format!("提前退出: {v}")),
                Err(e) => last = Some(format!("非超时错误: {e}")),
            }
        }
        panic!("三次执行均未超时（超时语义可能失效）：{last:?}");
    }

    #[tokio::test]
    async fn test_ready_probe_pattern_marks_ready() {
        // 日志关键词就绪探测：进程输出匹配 pattern 后任务标记 ready 并通知。
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let dir = tempfile::tempdir().unwrap();
        let manager = CommandManager::new(Some(dir.path().to_path_buf()));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1);
        let notifier = TestReadyNotifier { tx };
        let manager = manager.with_notifier(Arc::new(notifier));

        // 输出 READY 后持续运行的命令（模拟长驻服务：就绪后不退出）
        let cmd = if cfg!(target_os = "windows") {
            "echo READY_SIGNAL; Start-Sleep -Seconds 30"
        } else {
            "echo READY_SIGNAL && sleep 30"
        };
        let spec = ReadySpec {
            port: None,
            pattern: Some("READY_SIGNAL".to_string()),
            initial_delay_ms: 100,
            timeout_ms: 10_000,
        };
        let task = manager
            .spawn_background("sess-r", cmd, None, Some(spec), 0, None)
            .await
            .unwrap();

        // 等待就绪通知（探测循环异步执行）
        let note = tokio::time::timeout(Duration::from_secs(15), rx.recv())
            .await
            .expect("就绪通知超时");
        assert!(
            note.as_deref().unwrap_or("").contains("READY_SIGNAL"),
            "通知应包含日志关键词: {:?}",
            note
        );
        let t = manager.get(&task.id).await.unwrap();
        assert!(t.ready, "任务应标记为已就绪");
        assert_eq!(
            t.status,
            CommandTaskStatus::Running,
            "长驻服务就绪后仍应运行中"
        );

        // 收尾：杀掉进程
        manager.kill(&task.id).await.unwrap();
    }

    /// 测试就绪通知器（channel 转发通知内容）。
    struct TestReadyNotifier {
        tx: tokio::sync::mpsc::Sender<String>,
    }
    #[async_trait]
    impl CommandNotifier for TestReadyNotifier {
        async fn on_command_terminal(&self, _s: &str, _t: &CommandTask, _r: usize) {}
        async fn on_command_ready(&self, _s: &str, _t: &CommandTask, note: &str) {
            let _ = self.tx.send(note.to_string()).await;
        }
    }

    #[tokio::test]
    async fn test_spawn_background_echo_completes() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let dir = tempfile::tempdir().unwrap();
        let manager = CommandManager::new(Some(dir.path().to_path_buf()));
        let task = manager
            .spawn_background("sess-1", "echo Hello", None, None, 0, None)
            .await
            .unwrap();
        assert!(
            task.id.starts_with(COMMAND_TASK_PREFIX),
            "ID 应带 cmd_ 前缀"
        );
        assert!(task.pid.is_some(), "spawn 后应有 PID");
        assert!(task.log_file.is_some(), "配置 logs_dir 后应有日志文件");

        let t = wait_terminal(&manager, &task.id).await;
        assert_eq!(t.status, CommandTaskStatus::Completed);
        assert_eq!(t.exit_code, Some(0));
        assert!(t.output_tail.contains("Hello"), "输出尾部应含命令输出");
        assert!(t.completed_at.is_some());

        let log = tokio::fs::read_to_string(t.log_file.clone().unwrap())
            .await
            .unwrap();
        assert!(log.contains("Hello"), "日志文件应含完整输出");
    }

    #[tokio::test]
    async fn test_background_kill_marks_cancelled() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let manager = CommandManager::new(None);
        let cmd = if cfg!(target_os = "windows") {
            "ping -n 20 127.0.0.1"
        } else {
            "sleep 30"
        };
        let task = manager
            .spawn_background("sess-1", cmd, None, None, 0, None)
            .await
            .unwrap();
        manager.kill(&task.id).await.unwrap();

        let t = manager.get(&task.id).await.unwrap();
        assert_eq!(t.status, CommandTaskStatus::Cancelled);
        // 终态幂等：再次 kill 为 no-op 不报错
        manager.kill(&task.id).await.unwrap();
    }

    /// 回归测试（T1-5）：并发许可必须随命令生命周期持有，上限真正生效。
    ///
    /// 旧实现把许可留在 `spawn_background` 局部作用域——函数返回即 drop，
    /// 命令仍在运行但许可已归还，第二个 spawn 直接穿过（上限形同虚设）。
    /// 判别力：注入「提前释放」（许可不随 watcher 存活）时本测试必红。
    #[tokio::test]
    async fn test_max_concurrent_commands_gates_spawn() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let manager = CommandManager::new(None).with_max_concurrent(1);
        let long_cmd = if cfg!(target_os = "windows") {
            "ping -n 5 127.0.0.1"
        } else {
            "sleep 4"
        };
        let first = manager
            .spawn_background("sess-c", long_cmd, None, None, 0, None)
            .await
            .unwrap();

        // 许可被 first 占着（进程仍在运行）→ 第二个 spawn 必须排队，不得立即返回
        let queued = tokio::time::timeout(
            Duration::from_millis(500),
            manager.spawn_background("sess-c", "echo B", None, None, 0, None),
        )
        .await;
        assert!(
            queued.is_err(),
            "并发上限=1 且首个命令仍在运行时，第二个命令应排队等待许可"
        );

        // first 结束 → 许可释放 → 第二个应能启动
        wait_terminal(&manager, &first.id).await;
        let second = tokio::time::timeout(
            Duration::from_secs(10),
            manager.spawn_background("sess-c", "echo B", None, None, 0, None),
        )
        .await
        .expect("首个命令结束后许可应释放，第二个命令应可启动")
        .unwrap();
        let t = wait_terminal(&manager, &second.id).await;
        assert_eq!(t.status, CommandTaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_background_list_and_unknown() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        let manager = CommandManager::new(None);
        let task = manager
            .spawn_background("sess-1", "echo A", None, None, 0, None)
            .await
            .unwrap();
        let list = manager.list().await;
        assert!(list.iter().any(|t| t.id == task.id), "列表应包含新任务");
        assert!(
            manager.get("cmd_nope").await.is_none(),
            "未知 ID 应返回 None"
        );
    }

    #[tokio::test]
    async fn test_background_completion_notifies_with_remaining() {
        // 完成通知：终态触发 on_command_terminal，remaining 计数随任务数递减
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        #[derive(Clone)]
        struct CountingNotifier {
            calls: Arc<Mutex<Vec<(String, String, usize)>>>,
        }
        #[async_trait]
        impl CommandNotifier for CountingNotifier {
            async fn on_command_terminal(
                &self,
                session_id: &str,
                task: &CommandTask,
                remaining: usize,
            ) {
                self.calls
                    .lock()
                    .await
                    .push((session_id.to_string(), task.id.clone(), remaining));
            }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let manager = CommandManager::new(None).with_notifier(Arc::new(CountingNotifier {
            calls: calls.clone(),
        }));

        let t1 = manager
            .spawn_background("sess-1", "echo A", None, None, 0, None)
            .await
            .unwrap();
        let t2 = manager
            .spawn_background("sess-1", "echo B", None, None, 0, None)
            .await
            .unwrap();
        wait_terminal(&manager, &t1.id).await;
        wait_terminal(&manager, &t2.id).await;

        let calls = calls.lock().await.clone();
        assert_eq!(calls.len(), 2, "两个任务各触发一次通知");
        assert!(calls.iter().all(|(s, _, _)| s == "sess-1"));
        // remaining 单调递减：先完成的任务 remaining=1，后完成 remaining=0
        let mut remainings: Vec<usize> = calls.iter().map(|(_, _, r)| *r).collect();
        remainings.sort();
        assert_eq!(remainings, vec![0, 1], "remaining 应为 1 → 0");
    }

    #[tokio::test]
    async fn test_background_completion_wakes_on_all_done() {
        // 唤醒：会话内全部命令完成后触发 waker（ADR-013：remaining == 0）
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        #[derive(Clone)]
        struct CountingWaker {
            wakes: Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait]
        impl CommandWaker for CountingWaker {
            async fn wake(&self, session_id: &str) {
                let _ = session_id;
                self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = CommandManager::new(None);
        manager
            .set_waker(Arc::new(CountingWaker {
                wakes: wakes.clone(),
            }))
            .await;

        let t1 = manager
            .spawn_background("sess-w", "echo A", None, None, 0, None)
            .await
            .unwrap();
        let t2 = manager
            .spawn_background("sess-w", "echo B", None, None, 0, None)
            .await
            .unwrap();
        wait_terminal(&manager, &t1.id).await;
        wait_terminal(&manager, &t2.id).await;
        // watcher 是异步收尾，稍等让唤醒回调执行
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "全部完成后应唤醒恰好一次"
        );
    }

    /// B 方案判别力（长驻服务「就绪」= 完成）：就绪释放「待完成」计数后，
    /// 其余任务均已完成 → 立即唤醒；重复释放幂等。
    /// 修复前：就绪不释放计数 → 服务续跑占住 remaining → 永不唤醒
    /// （allComplete 停滞，需人工停服破局）——本断言固化新语义。
    #[tokio::test]
    async fn test_ready_release_unblocks_wake_and_is_idempotent() {
        let _g = CHILD_REGISTRY_LOCK.lock().await; // 与 kill_all 类测试互斥（见常量注释）
        #[derive(Clone)]
        struct CountingWaker {
            wakes: Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait]
        impl CommandWaker for CountingWaker {
            async fn wake(&self, _session_id: &str) {
                self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = CommandManager::new(None);
        manager
            .set_waker(Arc::new(CountingWaker {
                wakes: wakes.clone(),
            }))
            .await;

        // 构造一个"运行中的长驻服务"任务（counted=true；会话 s1 计数=1）
        let svc = CommandTask {
            id: "cmd_r1".to_string(),
            command: "dev server".to_string(),
            cwd: None,
            parent_session_id: "s1".to_string(),
            status: CommandTaskStatus::Running,
            exit_code: None,
            pid: Some(1),
            log_file: None,
            output_tail: String::new(),
            created_at: 0,
            completed_at: None,
            seq: 0,
            anchor_seq: 0,
            working_directory: None,
            ready: false,
            ready_note: None,
            counted: true,
        };
        manager.tasks.lock().await.insert(
            svc.id.clone(),
            TaskEntry {
                task: svc,
                cancelled: false,
            },
        );
        manager
            .session_counts
            .lock()
            .await
            .insert("s1".to_string(), 1);

        // 就绪：释放计数 → remaining 归零 → 唤醒（B：就绪即"完成"）
        CommandManager::release_wait_count(&manager, "cmd_r1").await;
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "就绪释放后 remaining=0 应唤醒（修复前：不释放、不唤醒）"
        );
        assert!(
            !manager.get("cmd_r1").await.unwrap().counted,
            "就绪后 counted 应置 false（不再占住待完成计数）"
        );

        // 幂等：重复释放不重复唤醒、不重复递减
        CommandManager::release_wait_count(&manager, "cmd_r1").await;
        assert_eq!(
            wakes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "重复释放应幂等（不重复唤醒）"
        );
    }

    /// 回归测试：输出增量事件按**时间窗合并**（ADR-028），且收尾必须 flush
    /// 残留增量——旧实现每读一块 emit 一次（高频命令即事件风暴）。
    #[tokio::test]
    async fn test_drain_output_merges_by_time_window_and_flushes_tail() {
        /// 分块立即就绪的测试 reader（模拟同一时间窗内的多次 read）。
        struct ChunkedReader {
            chunks: Vec<Vec<u8>>,
            idx: usize,
        }
        impl AsyncRead for ChunkedReader {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if self.idx >= self.chunks.len() {
                    return std::task::Poll::Ready(Ok(()));
                }
                buf.put_slice(&self.chunks[self.idx]);
                self.idx += 1;
                std::task::Poll::Ready(Ok(()))
            }
        }

        struct RecordingSink {
            deltas: Mutex<Vec<String>>,
        }
        #[async_trait]
        impl CommandEventSink for RecordingSink {
            async fn emit(&self, _task_id: &str, event: Value) {
                if let Some(d) = event.get("delta").and_then(|v| v.as_str()) {
                    self.deltas.lock().await.push(d.to_string());
                }
            }
        }

        let reader = ChunkedReader {
            chunks: vec![
                b"line-1\n".to_vec(),
                b"line-2\n".to_vec(),
                b"line-3\n".to_vec(),
            ],
            idx: 0,
        };
        let sink = Arc::new(RecordingSink {
            deltas: Mutex::new(Vec::new()),
        });
        let tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let file: Arc<Mutex<Option<tokio::fs::File>>> = Arc::new(Mutex::new(None));

        drain_output(
            reader,
            tail.clone(),
            file,
            "cmd_test".to_string(),
            Some(sink.clone()),
        )
        .await;

        let deltas = sink.deltas.lock().await.clone();
        assert_eq!(
            deltas.len(),
            1,
            "同一时间窗内的多块输出应合并为 1 个事件（实际 {} 个）",
            deltas.len()
        );
        assert_eq!(
            deltas[0], "line-1\nline-2\nline-3\n",
            "合并内容须完整无丢块"
        );
        assert_eq!(tail.lock().await.as_str(), "line-1\nline-2\nline-3\n");
    }

    /// 回归：输出 tail 截断必须 UTF-8 边界安全——旧实现 `String::drain(..excess)`
    /// 在丢弃起点落在多字节字符中间时 panic（当时 release `panic = "abort"` →
    /// 整个进程无痕退出——2026-09-14 后台任务闪退的根因路径；现已改 `unwind`）。
    /// 判别力：输入 33000 字节纯中文（32KB 上限后首切 excess=232，恰落在
    /// 字符内部）——旧实现必 panic 红。
    #[tokio::test]
    async fn test_drain_output_tail_truncation_utf8_safe() {
        let content = "中".repeat(11000); // 33000 字节
        let reader: &[u8] = content.as_bytes();
        let tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let file: Arc<Mutex<Option<tokio::fs::File>>> = Arc::new(Mutex::new(None));

        drain_output(reader, tail.clone(), file, "cmd_utf8".to_string(), None).await;

        let t = tail.lock().await;
        assert!(
            t.len() <= OUTPUT_TAIL_MAX_BYTES,
            "tail 不得超过 32KB 上限（实际 {} 字节）",
            t.len()
        );
        assert!(
            t.chars().all(|c| c == '中'),
            "截断后必须仍是完整字符（不得切出半字符）"
        );
        assert_eq!(t.len(), 32766, "对齐到字符边界后的精确保留量");
    }

    /// 回归（同源单点）：finalize_finished_task 追加退出标记后的 tail 截断
    /// 同样必须 UTF-8 安全（同一 `truncate_keep_tail_bytes` 单点）。
    #[tokio::test]
    async fn test_finalize_task_tail_truncation_utf8_safe() {
        let manager = CommandManager::new(None);
        let tail: Arc<Mutex<String>> = Arc::new(Mutex::new("中".repeat(11000))); // 33000 字节
        let file: Arc<Mutex<Option<tokio::fs::File>>> = Arc::new(Mutex::new(None));

        // 标记 21 字节：33021 → excess 253（恰落在字符内部）——旧实现必 panic 红
        let result = manager
            .finalize_finished_task("cmd_none", Some(0), &tail, &file)
            .await;
        assert!(result.is_none(), "未注册的任务应返回 None");
        let t = tail.lock().await;
        assert!(t.len() <= OUTPUT_TAIL_MAX_BYTES);
        assert!(
            t.ends_with("--- exit code: 0 ---\n"),
            "保留尾部应含退出标记"
        );
    }

    #[test]
    fn test_decode_utf8_keep_tail_holds_incomplete_sequence() {
        // "中" = E4 B8 AD：前 2 字节被切断 → 残余保留，不出 �
        let bytes = [b'a', 0xE4, 0xB8];
        let (text, rest) = decode_utf8_keep_tail(&bytes);
        assert_eq!(text, "a");
        assert_eq!(rest, vec![0xE4, 0xB8]);

        // 拼上第 3 字节 → 完整解码
        let mut joined = rest;
        joined.push(0xAD);
        let (text2, rest2) = decode_utf8_keep_tail(&joined);
        assert_eq!(text2, "中");
        assert!(rest2.is_empty());
    }

    #[test]
    fn test_decode_utf8_keep_tail_replaces_invalid_bytes() {
        // 非法字节（0xFF）→ �，不积累残余、不卡住
        let (text, rest) = decode_utf8_keep_tail(&[0xFF, b'x']);
        assert!(text.starts_with('\u{FFFD}'));
        assert!(text.ends_with('x'));
        assert!(rest.is_empty());
    }
}
