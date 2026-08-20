use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Semaphore};

use crate::common::error::{Result, TianyanError};

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

/// 后台命令终态通知器（Agent 装配层注入；默认实现把通知持久化到父会话）。
///
/// `remaining` 为父会话中仍运行中的命令任务数（join 信号）。
#[async_trait]
pub trait CommandNotifier: Send + Sync {
    /// 命令任务进入终态时调用（Completed / Failed / Cancelled）。
    async fn on_command_terminal(&self, session_id: &str, task: &CommandTask, remaining: usize);
    /// 就绪探测通过/超时通知（默认静默；实现方写入父会话）。
    async fn on_command_ready(
        &self,
        _session_id: &str,
        _task: &CommandTask,
        _note: &str,
    ) {
    }
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
    /// 就绪探测是否已通过（服务可用的信号）。
    pub ready: bool,
    /// 就绪/超时说明（如 "端口 3000 已监听" / "就绪探测超时"）。
    pub ready_note: Option<String>,
}

/// 后台命令就绪探测规格（execute_command(background).ready 解析）。
#[derive(Debug, Clone, Default)]
pub struct ReadySpec {
    /// 就绪判定端口（TCP 连接成功即就绪）。
    pub port: Option<u16>,
    /// 就绪判定日志关键词（日志尾部出现即就绪）。
    pub pattern: Option<String>,
    /// 首次探测等待（指数退避起点，毫秒；缺省 500）。
    pub initial_delay_ms: u64,
    /// 探测总超时（毫秒；缺省 300000）。
    pub timeout_ms: u64,
}

/// 注册表内条目（保留 + 内部标志）。
struct TaskEntry {
    task: CommandTask,
    cancelled: bool,
}

/// 后台命令管理器（fire-and-forget 进程原语，对标 DSH 后台任务语义）。
///
/// - **立即返回**：`spawn_background` 只负责拉起进程并返回任务快照，调用方不被阻塞；
/// - **输出观测**：stdout/stderr 追加写入日志文件（logs_dir 存在时）+ 内存尾部缓冲，
///   通过 `get`/`list` 非阻塞读取尾部，完整输出用日志文件路径 + read_file 读取；
/// - **显式终止**：`kill` 按 PID 杀进程树（Windows taskkill /T；Unix 进程组 kill），
///   终态任务为幂等空操作；
/// - **防失控**：并发上限信号量 + 注册表保留上限（逐出最旧终态任务）。
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
}

impl CommandManager {
    /// 创建管理器。`logs_dir` 为后台命令日志目录（None 时仅内存尾部缓冲，不落盘）。
    pub fn new(logs_dir: Option<PathBuf>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
            logs_dir,
            semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_COMMANDS)),
            notifier: None,
            waker: Arc::new(Mutex::new(None)),
            session_counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 后台启动命令：立即返回任务快照，进程在独立任务中运行。
    ///
    /// 输出写日志文件 + 内存尾部；进程退出后由 watcher 任务更新终态，
    /// 并触发完成通知（注入父会话）+ 唤醒（ADR-013 语义）。
    ///
    /// `session_id` 为发起命令的父会话（完成通知归属）。
    pub async fn spawn_background(
        &self,
        session_id: &str,
        command: &str,
        cwd: Option<&str>,
        ready: Option<ReadySpec>,
    ) -> Result<CommandTask> {
        // 并发许可：排队等待（防失控扇出；许可随 watcher 任务结束自动释放）
        let _permit = self
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
        let log_file = self
            .logs_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.log")));

        // 预写命令回显头
        if let Some(path) = &log_file {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let header = format!(
                "> $ {}{}\n",
                cwd.map(|d| format!("({d}) ")).unwrap_or_default(),
                command
            );
            let _ = tokio::fs::write(path, header).await;
        }

        let mut cmd = build_shell_command(command);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| TianyanError::Custom(format!("executor: 后台命令启动失败：{e}")))?;
        let pid = child.id();

        let tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let file: Arc<Mutex<Option<tokio::fs::File>>> = Arc::new(Mutex::new(None));
        if let Some(path) = &log_file {
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
        }

        let out_reader = child
            .stdout
            .take()
            .map(|r| tokio::spawn(drain_output(r, tail.clone(), file.clone())));
        let err_reader = child
            .stderr
            .take()
            .map(|r| tokio::spawn(drain_output(r, tail.clone(), file.clone())));

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
            ready: false,
            ready_note: None,
        };
        self.tasks.lock().await.insert(
            id.clone(),
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

        // watcher：等进程退出 → 收 reader → 写退出标记 → 更新终态
        let manager = self.clone();
        let tail_w = tail.clone();
        let file_w = file.clone();
        let id_w = id.clone();
        tokio::spawn(async move {
            let wait_result = child.wait().await;
            if let Some(h) = out_reader {
                let _ = h.await;
            }
            if let Some(h) = err_reader {
                let _ = h.await;
            }
            let exit_code = wait_result.ok().and_then(|s| s.code());

            let mut tasks = manager.tasks.lock().await;
            let is_cancelled = tasks.get(&id_w).map(|e| e.cancelled).unwrap_or(false);
            let marker = if is_cancelled {
                "--- terminated (killed) ---\n".to_string()
            } else {
                format!("--- exit code: {} ---\n", exit_code.unwrap_or(-1))
            };
            {
                let mut t = tail_w.lock().await;
                t.push_str(&marker);
                if t.len() > OUTPUT_TAIL_MAX_BYTES {
                    let excess = t.len() - OUTPUT_TAIL_MAX_BYTES;
                    t.drain(..excess);
                }
                let mut f = file_w.lock().await;
                if let Some(f) = f.as_mut() {
                    let _ = f.write_all(marker.as_bytes()).await;
                }
            }
            let task_snapshot = {
                if let Some(entry) = tasks.get_mut(&id_w) {
                    if !is_cancelled {
                        entry.task.status = if exit_code == Some(0) {
                            CommandTaskStatus::Completed
                        } else {
                            CommandTaskStatus::Failed
                        };
                        entry.task.exit_code = exit_code;
                    }
                    entry.task.completed_at = Some(now_ms());
                    entry.task.output_tail = tail_w.lock().await.clone();
                }
                tasks.get(&id_w).map(|e| e.task.clone())
            };
            // 锁外收尾：remaining 计数 → 完成通知 → 唤醒（ADR-013）
            if let Some(task) = task_snapshot {
                let session_id = task.parent_session_id.clone();
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
                if let Some(notifier) = &manager.notifier {
                    notifier
                        .on_command_terminal(&session_id, &task, remaining)
                        .await;
                }
                // should_wake = allComplete || failure（部分完成保持静默）
                if remaining == 0 || task.status == CommandTaskStatus::Failed {
                    if let Some(waker) = manager.waker.lock().await.clone() {
                        waker.wake(&session_id).await;
                    }
                }
            }
        });

        // 就绪探测：端口监听 / 日志关键词匹配后通知主 agent 服务已就绪。
        // 指数退避（起点 initial_delay_ms，×2，抖动 ±20%）；总超时后通知未就绪
        // （不杀进程——服务可能仍在启动，退出由 watcher 另行通知）。
        if let Some(spec) = ready {
            let manager = self.clone();
            let tail_r = tail.clone();
            let task_id = id.clone();
            tokio::spawn(async move {
                let mut delay_ms = spec.initial_delay_ms.max(50);
                let start = now_ms();
                let timeout_ms = spec.timeout_ms.max(delay_ms);
                loop {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    // 进程已退出：停止探测（watcher 已发终态通知）
                    let terminal = manager
                        .tasks
                        .lock()
                        .await
                        .get(&task_id)
                        .map(|e| e.task.status != CommandTaskStatus::Running)
                        .unwrap_or(true);
                    if terminal {
                        break;
                    }
                    // 判定 1：端口 TCP 连接成功
                    let port_ok = match spec.port {
                        Some(port) => tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok(),
                        None => false,
                    };
                    // 判定 2：日志尾部关键词
                    let pattern_ok = if let Some(p) = spec.pattern.as_deref() {
                        if p.is_empty() {
                            false
                        } else {
                            tail_r.lock().await.contains(p)
                        }
                    } else {
                        false
                    };
                    if port_ok || pattern_ok {
                        let note = if port_ok {
                            format!("端口 {} 已监听", spec.port.unwrap_or(0))
                        } else {
                            format!("日志出现关键词 \"{}\"", spec.pattern.as_deref().unwrap_or(""))
                        };
                        Self::mark_ready(&manager, &task_id, Some(note.clone())).await;
                        Self::notify_ready(&manager, &task_id, &note).await;
                        break;
                    }
                    // 超时：通知未就绪（不杀进程）
                    if now_ms() - start >= timeout_ms as i64 {
                        let note = format!("就绪探测超时（{} ms）", timeout_ms);
                        Self::mark_ready(&manager, &task_id, Some(note.clone())).await;
                        Self::notify_ready(&manager, &task_id, &note).await;
                        break;
                    }
                    // 指数退避：×2 + ±20% 抖动
                    delay_ms = (delay_ms * 2).min(30_000);
                    // 纯指数退避（无抖动依赖）
                }
            });
        }

        Ok(task)
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
/// Windows 用 PowerShell（Win10/11 默认自带）：与工具描述、系统提示词的
/// PowerShell 语义一致（管道/Out-File/Select-String 等 cmdlet 可用），
/// 避免模型按 PowerShell 语法写命令却在 cmd.exe 下失败。
/// `-NoProfile -NonInteractive` 跳过用户配置加载并禁止交互提示（后台/
/// 自动化场景下防挂起）；`-Command` 直接执行整条命令串。
fn build_shell_command(command: &str) -> tokio::process::Command {
    if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("powershell");
        c.arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// 按 PID 杀进程树（Windows: taskkill /T /F；Unix: 进程组 kill -9 -pgid）。
///
/// Unix 侧依赖 spawn 时 `process_group(0)`：子进程成为新进程组组长，
/// 负 PID 信号作用于整个组——shell 派生的服务进程一并终止（对齐
/// opencode #30868 教训：只杀 shell 会留孤儿服务进程）。
async fn kill_process_tree(pid: u32) {
    let result = if cfg!(target_os = "windows") {
        tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
    } else {
        tokio::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
    };
    if let Err(e) = result {
        tracing::warn!(pid, error = %e, "终止进程树失败");
    }
}

/// 读取子进程输出流：追加到内存尾部缓冲（截断）+ 日志文件。
///
/// 锁顺序固定为 tail → file（与 watcher 的标记写入一致，避免死锁）。
async fn drain_output<R>(
    mut reader: R,
    tail: Arc<Mutex<String>>,
    file: Arc<Mutex<Option<tokio::fs::File>>>,
) where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                {
                    let mut t = tail.lock().await;
                    t.push_str(&String::from_utf8_lossy(chunk));
                    if t.len() > OUTPUT_TAIL_MAX_BYTES {
                        let excess = t.len() - OUTPUT_TAIL_MAX_BYTES;
                        t.drain(..excess);
                    }
                }
                let mut f = file.lock().await;
                if let Some(f) = f.as_mut() {
                    if f.write_all(chunk).await.is_err() {
                        tracing::warn!("写入后台命令日志失败");
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "读取后台命令输出失败");
                break;
            }
        }
    }
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

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
///
/// 跨平台：Unix (Linux/macOS) 用 `sh -c`，Windows 用 `powershell -NoProfile -NonInteractive -Command`。
/// 超时后**杀进程树**（不只 shell）：Windows taskkill /T /F，Unix 进程组 kill，
/// 避免派生服务进程残留为孤儿（opencode #30868 同款问题）。
pub async fn execute_command_action(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
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

    let result = tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await;

    match result {
        Ok(Ok(status)) => {
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

            Ok(json!({
                "stdout": String::from_utf8_lossy(&stdout_bytes).to_string(),
                "stderr": String::from_utf8_lossy(&stderr_bytes).to_string(),
                "exit_code": status.code().unwrap_or(-1),
            }))
        }
        Ok(Err(e)) => Err(TianyanError::Custom(format!(
            "executor: 命令执行失败：{}",
            e
        ))),
        Err(_elapsed) => {
            // 杀进程树（taskkill /T 或 kill -9 -pgid），再回收子进程
            if let Some(pid) = child.id() {
                kill_process_tree(pid).await;
            }
            if let Err(e) = child.wait().await {
                tracing::warn!(error = %e, "等待超时子进程退出失败");
            }
            Err(TianyanError::timeout("executor: 执行超时"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
    async fn test_execute_command() {
        let result = execute_command_action("echo Hello", None, Some(5)).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output["stdout"].as_str().unwrap_or("").contains("Hello"));
    }

    #[tokio::test]
    async fn test_command_timeout_is_timeout_error() {
        // 子进程运行时长超过超时阈值：Windows 用 ping 计数（约 4s），Unix 用 sleep
        let cmd = if cfg!(target_os = "windows") {
            "ping -n 5 127.0.0.1"
        } else {
            "sleep 5"
        };
        let err = execute_command_action(cmd, None, Some(1))
            .await
            .unwrap_err();
        assert!(
            err.is_timeout(),
            "执行超时应分类为 timeout（ADR-014）：{err}"
        );
    }

    #[tokio::test]
    async fn test_ready_probe_pattern_marks_ready() {
        // 日志关键词就绪探测：进程输出匹配 pattern 后任务标记 ready 并通知。
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
            .spawn_background("sess-r", cmd, None, Some(spec))
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
        assert_eq!(t.status, CommandTaskStatus::Running, "长驻服务就绪后仍应运行中");

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
        let dir = tempfile::tempdir().unwrap();
        let manager = CommandManager::new(Some(dir.path().to_path_buf()));
        let task = manager
            .spawn_background("sess-1", "echo Hello", None, None)
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
        let manager = CommandManager::new(None);
        let cmd = if cfg!(target_os = "windows") {
            "ping -n 20 127.0.0.1"
        } else {
            "sleep 30"
        };
        let task = manager.spawn_background("sess-1", cmd, None, None).await.unwrap();
        manager.kill(&task.id).await.unwrap();

        let t = manager.get(&task.id).await.unwrap();
        assert_eq!(t.status, CommandTaskStatus::Cancelled);
        // 终态幂等：再次 kill 为 no-op 不报错
        manager.kill(&task.id).await.unwrap();
    }

    #[tokio::test]
    async fn test_background_list_and_unknown() {
        let manager = CommandManager::new(None);
        let task = manager
            .spawn_background("sess-1", "echo A", None, None)
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
        #[derive(Clone)]
        struct CountingNotifier {
            calls: Arc<tokio::sync::Mutex<Vec<(String, String, usize)>>>,
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
        let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let manager = CommandManager::new(None).with_notifier(Arc::new(CountingNotifier {
            calls: calls.clone(),
        }));

        let t1 = manager
            .spawn_background("sess-1", "echo A", None, None)
            .await
            .unwrap();
        let t2 = manager
            .spawn_background("sess-1", "echo B", None, None)
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
            .spawn_background("sess-w", "echo A", None, None)
            .await
            .unwrap();
        let t2 = manager
            .spawn_background("sess-w", "echo B", None, None)
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
}
