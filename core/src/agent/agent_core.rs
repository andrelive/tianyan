//! 智能体核心实现。
//!
//! 包含 Agent 结构体定义、构造函数和所有辅助方法。
//! 将 Agent 与 AgentCoordinator trait 分离，消除循环依赖。

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as TokioMutex;
use tokio::sync::{OwnedMutexGuard, RwLock};

use crate::agent::background::{BackgroundTaskManager, TaskWaker};
use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::stream_forward::{spawn_stream_forwarder, StreamEventDeliver, StreamEventMapper};
use crate::agent::tool_registry::DynamicToolExecutor;
use crate::agent::types::{AgentResponse, AgentState, StreamChunkType, StreamEventSender};
use crate::agent::working_set::{SessionWorkingSet, WorkingSetRegistry};
use crate::common::error::Result;
use crate::common::types::{
    InjectableContext, Message, MessageRole, StructuredMessage, TokenUsage,
};
use crate::context::{ContextAssembler, ContextPipeline};
use crate::executor::approval::ApprovalWorkflow;
use crate::observability::AgentMetrics;
use crate::observability::TokenRecord;
use crate::session::{Session, SessionManager};
use crate::snapshot::SnapshotManager;

/// compression_marker 之后至少积累多少条消息才触发压缩。
const MIN_MESSAGES_BEFORE_COMPRESSION: usize = 6;

/// 唤醒轮重试次数（ADR-013：重试 2 次后降级为静默）。
const WAKE_RETRY_LIMIT: usize = 3;
/// 唤醒轮重试间隔。
const WAKE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// 唤醒轮"失败必须汇报"的判定窗口（T1-2）。
///
/// 只把**本会话**中最近该窗口内到达终态的失败任务视为"本次失败"：
/// 任务注册表保留 3 天 TTL，旧实现扫全量跨会话任务且不看时间——任一会话、
/// 任一 3 天内的历史失败都会让每个唤醒轮进入"必须汇报失败"分支（指令与
/// 事实不符，模型被推着重复/编造失败汇报）。窗口取 1 小时：远大于"任务
/// 失败 → 唤醒触发"的正常延迟（秒级），又足以把陈年失败排除在外。
const WAKE_FAILURE_FRESH_WINDOW_MS: i64 = 60 * 60 * 1000;

/// 唤醒轮"本次需汇报失败"判定（T1-2 单点）。
///
/// 条件：任务属于**当前会话** && 失败状态 && 最近
/// [`WAKE_FAILURE_FRESH_WINDOW_MS`] 内到达终态（`completed_at` 缺失的
/// 异常数据保守不命中）。
fn is_fresh_failure(
    session_id: &str,
    task_session: &str,
    failed: bool,
    completed_at: Option<i64>,
    now_ms: i64,
) -> bool {
    failed
        && task_session == session_id
        && completed_at
            .map(|ts| now_ms - ts <= WAKE_FAILURE_FRESH_WINDOW_MS)
            .unwrap_or(false)
}

/// 轮执行模式：Plain 构造 [`AgentResponse`]；Stream 向 sender 推送流式事件。
#[derive(Clone)]
pub(crate) enum TurnMode {
    /// 非流式：结果组装为 `AgentResponse`。
    Plain,
    /// 流式：loop 用 `run_stream` 逐 chunk 推送，结果处理发送流式完成事件。
    Stream { sender: StreamEventSender },
}

/// 轮级差异段开关（调用方按路径语义选择）。
pub(crate) struct TurnOptions {
    pub mode: TurnMode,
    /// 轮前是否捕获工作区快照（用户轮 true；澄清回答轮保持原语义 false）。
    pub do_snapshot: bool,
    /// 轮后是否执行压缩检查（用户轮与流式轮 true；澄清回答轮 false）。
    pub do_compress: bool,
    /// 轮前是否检测"会话工具表变化"（真用户轮 true；唤醒轮不经本函数，
    /// 天然不检查——只在用户输入轮付一次指纹成本）。
    pub do_toolset_check: bool,
    /// 本会话思考强度档位（会话时选择；None 时使用模型默认）。
    pub thinking_effort: Option<String>,
}

/// 智能体协调器的默认实现。
#[derive(Clone)]
pub struct Agent {
    /// 默认对话模型名（配置中指定，未被请求级 model 覆盖时使用）。
    pub(crate) default_model: String,
    pub(crate) context_pipeline: ContextPipeline,
    pub(crate) metrics: Arc<AgentMetrics>,
    pub(crate) state: Arc<RwLock<AgentState>>,
    pub(crate) agent_loop: AgentLoop,
    pub(crate) session_manager: Arc<dyn SessionManager>,
    /// 工作区快照管理器（配置了 working_directory 时启用，用于会话回退恢复文件）。
    pub(crate) snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory 配置；会话级绑定缺省时
    /// 快照捕获以此为根）。
    pub(crate) default_working_directory: Option<PathBuf>,
    /// 会话工作集注册表（ADR-035）：物化上下文缓存 + per-session 锁的唯一持有者
    /// （ADR-013 串行化的锁表已迁入，`turn_guard` 委托之）。
    pub(crate) working_sets: Arc<WorkingSetRegistry>,
    /// 后台任务管理器（delegate_to_agent(background) 状态机）。
    ///
    /// 协调器/唤醒器直接持有（构造时从 ToolRegistry 提取**同一 Arc**，
    /// 与工具执行路径共享状态）——避免 `Agent → AgentLoop → ToolRegistry`
    /// 三层穿透。
    pub(crate) background_tasks: Arc<BackgroundTaskManager>,
    /// 后台命令管理器（execute_command(background) 状态机；与 ToolRegistry 共享同一 Arc）。
    pub(crate) command_tasks: Arc<crate::executor::CommandManager>,
    /// 审批工作流（危险操作门控）。协调器响应/状态查询直接持有
    /// （与 ToolRegistry 共享同一 Arc）。
    pub(crate) approval_workflow: Option<Arc<ApprovalWorkflow>>,
    /// 待用户确认的操作指纹队列（与 ToolRegistry 共享同一 Arc；
    /// 状态展示直读，不经穿透）。
    pub(crate) pending_approval_fingerprints: Arc<TokioMutex<Vec<String>>>,
}

impl Agent {
    /// 创建新的 Agent 实例。
    ///
    /// 此构造函数由 AgentBuilder::build() 调用，外部应通过 Builder 创建 Agent。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        default_model: String,
        context_pipeline: ContextPipeline,
        metrics: Arc<AgentMetrics>,
        agent_loop: AgentLoop,
        session_manager: Arc<dyn SessionManager>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
        default_working_directory: Option<PathBuf>,
        working_sets: Arc<WorkingSetRegistry>,
    ) -> Self {
        // 从 AgentLoop 的 ToolRegistry 提取共享句柄（同一 Arc，非复制状态）：
        // 协调器对后台任务/审批的直接入口，避免逐调用三层穿透。
        let tool_registry = agent_loop.tool_registry();
        let background_tasks = tool_registry.background_tasks.clone();
        let command_tasks = tool_registry.command_tasks.clone();
        let approval_workflow = tool_registry.approval_workflow.clone();
        let pending_approval_fingerprints = tool_registry.pending_approval_fingerprints.clone();
        Self {
            default_model,
            context_pipeline,
            metrics,
            state: Arc::new(RwLock::new(AgentState::default())),
            agent_loop,
            session_manager,
            snapshot_manager,
            default_working_directory,
            working_sets,
            background_tasks,
            command_tasks,
            approval_workflow,
            pending_approval_fingerprints,
        }
    }

    /// 获取会话级轮次锁（ADR-013 串行化；锁表由工作集注册表承载，ADR-035 §6）。
    ///
    /// 持锁期间该会话的任何其他轮次（用户消息 / 唤醒轮）**排队等待**；
    /// 通知唤醒走 [`Self::turn_try_lock`]（幂等，不排队）。
    pub(crate) async fn turn_guard(&self, session_id: &str) -> OwnedMutexGuard<()> {
        self.working_sets.lock(session_id).await
    }

    /// 尝试获取会话级轮次锁（幂等，不排队）——通知唤醒入口（ADR-035 §4）。
    ///
    /// 拿不到 = 该会话已有 loop 在跑 → 无需再启一个（那个 loop 的轮边界
    /// 续跑判定会消化新通知）。
    pub(crate) async fn turn_try_lock(&self, session_id: &str) -> Option<OwnedMutexGuard<()>> {
        self.working_sets.try_lock(session_id).await
    }

    /// 注册任务唤醒器（ADR-013：后台任务全部完成/失败 → 触发唤醒轮）。
    ///
    /// 由 server 装配层在 Agent 构建完成后调用（传入自引用转发器
    /// [`AgentWakeForwarder`]——Weak 打破循环引用）。
    pub async fn register_task_waker(&self, waker: Arc<dyn TaskWaker>) {
        self.background_tasks.set_waker(waker.clone()).await;
        // 后台命令唤醒复用同一转发器（CommandWaker 适配）
        self.command_tasks
            .set_waker(Arc::new(CommandWakeAdapter(waker)))
            .await;
    }

    /// 唤醒轮（ADR-013）：后台任务全部完成或失败时触发主 agent 新一轮。
    ///
    /// 与用户轮共用 turn 锁（串行化）；上下文 = 会话历史（含已注入的
    /// System 完成通知）+ 唤醒指令；模型可输出空文本合法结束；失败重试
    /// [`WAKE_RETRY_LIMIT`] 次后降级为静默（消息已在 transcript，用户下
    /// 一条消息自然触发）。
    ///
    /// ADR-031：**流式化**——输出经调用方传入的 [`StreamEventSender`] 逐
    /// chunk 推送（调用方转发到统一事件通道，前端打字机看到汇总）。调用方
    /// 必须在调用**前**创建事件通道并**立即开始消费**（转发器先启动消费
    /// 任务再调用本方法）——唤醒轮期间事件实时流出；若先 await 本方法再
    /// 消费，输出会积压到轮结束（超过通道缓冲还会死锁：发送端等消费、
    pub(crate) async fn process_wake(&self, session_id: &str, sender: StreamEventSender) {
        let _turn = self.turn_guard(session_id).await;
        self.process_wake_locked(session_id, sender).await;
    }

    /// 唤醒轮主体（ADR-035 §4：**调用方须已持有会话锁**）。
    ///
    /// 幂等入口 [`AgentWakeForwarder::wake`] 用 `try_lock` 取得锁后直接调用本方法
    /// （避免重复取锁死锁）；持锁跨越整个唤醒轮 → "已有 loop 在跑"的判定与
    /// loop 收尾处于同一临界区，无丢通知窗口。
    pub(crate) async fn process_wake_locked(&self, session_id: &str, sender: StreamEventSender) {
        let mut last_err: Option<String> = None;
        for attempt in 0..WAKE_RETRY_LIMIT {
            if attempt > 0 {
                tokio::time::sleep(WAKE_RETRY_DELAY).await;
            }
            let state = match self.load_and_build_state(session_id).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, "唤醒轮加载会话失败");
                    last_err = Some(e.to_string());
                    continue;
                }
            };
            let messages = self.prepare_wake_context(&state, session_id).await;
            let parent_id = {
                let s = state.read().await;
                s.structured_messages.last().map(|m| m.id.clone())
            };
            let start = Instant::now();

            let loop_result = self
                .agent_loop
                .clone()
                .run_stream(
                    &mut messages.clone(),
                    sender.clone(),
                    session_id,
                    parent_id.as_deref(),
                    &self.default_model,
                    None,
                    // 唤醒轮不携带会话思考选择，使用模型默认
                    None,
                )
                .await;

            match &loop_result {
                Ok(AgentLoopResult::Answer { content, .. }) => {
                    if content.is_empty() {
                        // 空输出（含 reasoning-only——思考模型"想完没说话"）
                        // 直接按正常结束落库（前端可见思考，不静默）。
                        // 此前对 reasoning-only 重试整个唤醒轮，但实测确认
                        // 这是模型对 system 完成通知的系统性行为（相同上下文
                        // 重试结果相同）——整轮重试只浪费 LLM 调用，已移除；
                        // loop 内部对空响应的单次重试（针对偶发完全空响应）
                        // 仍保留。
                        tracing::info!(session = %session_id, "唤醒轮空输出（模型无需回复/仅思考）");
                    } else {
                        tracing::info!(session = %session_id, "唤醒轮完成：后台任务结果已汇总");
                    }
                }
                Ok(AgentLoopResult::Cancelled { .. }) => {
                    tracing::debug!(session = %session_id, "唤醒轮被取消");
                }
                Ok(AgentLoopResult::MaxTurnsReached { content, .. }) => {
                    // ADR-030：唤醒轮用默认策略（max_turns 报错），此变体
                    // 仅防御性覆盖——尽力而为策略下达到轮数上限视为完成
                    tracing::info!(session = %session_id, content_len = content.len(), "唤醒轮达到轮数上限");
                }
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, attempt, "唤醒轮执行失败");
                    last_err = Some(e.to_string());
                    continue;
                }
            }
            // 统一结果处理（Stream 模式：send_complete + 落库 + 指标）
            let (_, loop_tokens) = self
                .apply_loop_result(
                    &state,
                    loop_result,
                    start,
                    &TurnMode::Stream {
                        sender: sender.clone(),
                    },
                )
                .await;
            self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
                .await;
            // C1：唤醒轮同样执行轮末压缩检查——高负载区间若主要由唤醒轮
            // 推进（等子代理报告/后台通知），无检查会使上下文持续增长而
            // 压缩被无限推迟（实测：60.8%→81.7% 区间零压缩）。判定成本
            // 廉价（消息数/实测 token 读取），仅超阈值才真正压缩。
            self.maybe_compress_and_persist(&state, session_id, false)
                .await;
            return;
        }
        tracing::warn!(
            error = ?last_err,
            session = %session_id,
            "唤醒轮重试 {} 次后放弃（消息已在会话中，用户下一条消息自然触发）",
            WAKE_RETRY_LIMIT - 1
        );
        // 重试耗尽：错误事件（前端可见——不再静默）
        sender
            .send_error(&format!("唤醒轮失败：{}", last_err.unwrap_or_default()))
            .await;
    }

    /// 组装唤醒轮上下文（ADR-013）。
    ///
    /// 与 [`Self::assemble_context`] 的区别：**不添加用户消息**（无用户输入），
    /// 直接组装会话历史（System 完成通知已在其中）+ 会话定位 + 唤醒指令。
    ///
    /// 唤醒指令区分失败/完成场景：**失败必须汇报**（ADR-013 shouldReply =
    /// allComplete || isTaskFailure——失败唤醒的目的就是让主 agent 知情并
    /// 继续处理），只有"全部成功且无需输出"才允许空输出。此前指令把两者
    /// 混在一起，模型在失败场景下选择空输出结束，主 agent 静默无反馈。
    ///
    /// 失败判定限定**本会话 + 时间窗**（T1-2，见 [`is_fresh_failure`]）。
    async fn prepare_wake_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) -> Vec<Message> {
        // 会话历史 + 前缀 + 会话定位（与用户轮共用组装；无用户消息注入）
        let mut messages = self.assemble_context(state, "").await;

        // 感知任务状态：任一**本会话、近期**后台任务/命令失败 → 失败场景
        let now_ms = chrono::Utc::now().timestamp_millis();
        let has_failure = {
            let bg = self.background_tasks.snapshot().await;
            let bg_failed = bg.iter().any(|t| {
                is_fresh_failure(
                    session_id,
                    &t.parent_session_id,
                    t.status == crate::agent::background::TaskStatus::Failed,
                    t.completed_at,
                    now_ms,
                )
            });
            let cmd = self.command_tasks.list().await;
            let cmd_failed = cmd.iter().any(|t| {
                is_fresh_failure(
                    session_id,
                    &t.parent_session_id,
                    t.status == crate::executor::CommandTaskStatus::Failed,
                    t.completed_at,
                    now_ms,
                )
            });
            bg_failed || cmd_failed
        };

        if has_failure {
            messages.push(Message::system(
                "## 自动唤醒轮\n这是一轮由后台任务完成（或失败）自动触发的处理轮，不是用户新消息。\n\
                 会话中有后台任务/命令执行失败：**必须向用户汇报失败情况**（哪些任务失败、\n\
                 失败原因、后续处理建议），并继续原有工作。\n\
                 禁止输出空文本结束本轮。",
            ));
        } else {
            messages.push(Message::system(
                "## 自动唤醒轮\n这是一轮由后台任务完成（或失败）自动触发的处理轮，不是用户新消息。\n\
                 若全部后台任务已完成：**必须向用户输出汇总**（哪些任务完成、\n\
                 结果摘要、后续计划），并继续原有工作。\n\
                 禁止输出空文本结束本轮。",
            ));
        }

        messages
    }

    /// 加载（或复用）可注入上下文（soul/rules/memories 前缀）。
    ///
    /// 会话级缓存 + 首次加载固化快照（提取自 assemble_context，唤醒轮共用）；
    /// `query` 用于首次加载时的语义检索（唤醒轮无用户查询，传空串）。
    async fn ensure_injectable(
        &self,
        state: &Arc<RwLock<SessionState>>,
        query: &str,
    ) -> InjectableContext {
        // 会话级缓存：soul 非空表示已加载过，跳过 I/O
        let soul_loaded = {
            let s = state.read().await;
            !s.injectable_context.soul.is_empty()
        };

        if !soul_loaded {
            // 会话生效工作目录（AGENTS.md 项目指令前缀的探测根）
            let workdir = self
                .resolve_working_directory(&state.read().await.session_id)
                .await;
            match self
                .context_pipeline
                .load_injectable(query, workdir.as_deref())
                .await
            {
                Ok(ctx) => {
                    let injected_rules = ctx.rules_and_experiences.len();
                    if injected_rules > 0 {
                        self.metrics.record_rule_hit(injected_rules).await;
                    }
                    state.write().await.injectable_context = ctx.clone();
                    // 固化注入上下文快照到会话（重启后沿用同一份前缀内容）
                    let sid = state.read().await.session_id.clone();
                    self.persist_injectable_snapshot(&sid, &ctx).await;
                    ctx
                }
                Err(e) => {
                    tracing::warn!(error = %e, "加载可注入上下文失败");
                    self.metrics.record_pipeline_failure().await;
                    state.read().await.injectable_context.clone()
                }
            }
        } else {
            let s = state.read().await;
            s.injectable_context.clone()
        }
    }

    /// 注册动态工具（如 MCP 工具桥接）。
    ///
    /// 由 server 层在 Agent 构建后注入：配置中的 MCP 服务器工具经
    /// 桥接注册后，对 LLM 可见并可执行（与内置工具同等地位）。
    pub async fn register_dynamic_tools(&self, tools: Vec<Arc<dyn DynamicToolExecutor>>) {
        for tool in tools {
            self.agent_loop
                .tool_registry()
                .register_dynamic_tool(tool)
                .await;
        }
    }

    /// 解析会话生效的工作目录：会话级绑定（header.working_directory，目录必须
    /// 存在）优先，缺省回退构建时注入的全局默认（[agent] working_directory）。
    pub(crate) async fn resolve_working_directory(&self, session_id: &str) -> Option<PathBuf> {
        if let Ok(Some(session)) = self.session_manager.get_session(session_id).await {
            if let Some(wd) = session.working_directory(None) {
                return Some(wd);
            }
        }
        self.default_working_directory.clone()
    }

    /// 捕获工作区快照（消息处理前调用，索引 = 当前消息数）。
    ///
    /// 快照根取会话生效的工作目录（会话级绑定优先，缺省全局配置），
    /// 经 [`SnapshotManager::with_workdir`] 绑定后捕获。
    pub(crate) async fn capture_workspace_snapshot(&self, session_id: &str, state: &SessionState) {
        let Some(sm) = &self.snapshot_manager else {
            return;
        };
        let Some(workdir) = self.resolve_working_directory(session_id).await else {
            return;
        };
        let index = state.structured_messages.len();
        if let Err(e) = sm.with_workdir(workdir).capture(session_id, index).await {
            tracing::warn!(error = %e, session = %session_id, "工作区快照捕获失败");
        }
    }

    /// 从持久化存储加载会话并构建 SessionState。
    ///
    /// ADR-035：装配了会话存储（`working_sets` 可用）时经工作集加载——
    /// 跨轮复用物化上下文 + seq 语义 + 廉价新鲜度自愈；未装配（测试桩）时
    /// 回退全量加载路径（行为与 ADR-035 之前一致）。
    pub(crate) async fn load_and_build_state(
        &self,
        session_id: &str,
    ) -> Result<Arc<RwLock<SessionState>>> {
        if self.working_sets.has_store() {
            match self.working_sets.ensure(session_id).await {
                Ok(ws) => return Ok(ws.state()),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = %session_id,
                        "工作集加载失败，回退全量加载"
                    );
                }
            }
        }
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .unwrap_or_else(|| Session::new(session_id));

        let state = Arc::new(RwLock::new(SessionState::new(session_id)));
        {
            let mut s = state.write().await;
            // 恢复注入上下文快照（重启后沿用会话固化的前缀内容，不重新检索——
            // 保证旧会话前缀稳定，prompt 缓存不失效；无快照时为空，首次加载填充）
            s.injectable_context = session.header.injectable_snapshot.unwrap_or_default();
            for sm in &session.messages {
                s.add_structured_message(sm.clone());
            }
        }
        Ok(state)
    }

    /// 取会话工作集（未装配会话存储时为 `None`）。
    pub(crate) async fn working_set(&self, session_id: &str) -> Option<Arc<SessionWorkingSet>> {
        self.working_sets.get(session_id).await
    }

    /// 写入收口 ①（ADR-035 §3）：经工作集追加（落库 + 更新物化段 + 推进 seq）。
    ///
    /// 未装配工作集时回退旧路径（内存态 + `session_manager` 落库），保证测试桩
    /// 与未装配场景行为不变。调用方传入的 `state` 在有工作集时**就是**工作集的
    /// 内存态（`load_and_build_state` 的返回源），故不重复写入。
    pub(crate) async fn persist_structured(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
        msg: StructuredMessage,
        index_fts: bool,
    ) -> Result<()> {
        if let Some(ws) = self.working_sets.get(session_id).await {
            ws.append(&msg, index_fts).await?;
            return Ok(());
        }
        state.write().await.add_structured_message(msg.clone());
        if index_fts {
            self.session_manager
                .add_structured_message(session_id, msg)
                .await
        } else {
            self.session_manager
                .add_structured_message_no_fts(session_id, msg)
                .await
        }
    }

    /// 组装当前轮次的上下文消息（soul/rules/memories 前缀 + 会话历史 + 会话定位）。
    ///
    /// 用户消息已由调用方写入 state（`persist_user_message` / 澄清续轮 / 唤醒轮），
    /// 本函数只做组装、不修改状态——用户消息的**单一创建点**在
    /// [`Self::persist_user_message`]（入库 + 内存状态同一份 StructuredMessage，
    /// 避免状态与库中消息链 id 分叉）。
    ///
    /// soul/rules/memories 仅会话首次加载，后续轮次复用缓存，
    /// 确保 system prompt 前缀稳定以命中 DeepSeek 前缀缓存。
    ///
    /// 图片（`message.content_parts`）随用户消息写入会话状态，经
    /// `ContextAssembler` 组装为多模态传输消息，对 LLM 可见。
    pub(crate) async fn assemble_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        query: &str,
    ) -> Vec<Message> {
        // 加载（或复用）可注入上下文：soul/rules/memories 前缀
        let injectable = self.ensure_injectable(state, query).await;

        let s = state.read().await;
        let sid = s.session_id.clone();
        let mut messages = ContextAssembler::assemble(&s.structured_messages, &injectable);

        // 注入会话定位信息：早期对话被压缩后，摘要字段可能不足以恢复细节，
        // 告知 LLM 当前会话 URI，使其可用 vfs_read 检索被压缩的原始记录。
        // 位置固定在 system 前缀（soul/rules）之后、历史消息之前，
        // 同会话内内容恒定，不影响 DeepSeek 前缀缓存。
        insert_session_hint(&mut messages, &sid);
        drop(s);

        // ADR-035 §4：组装即「消费到库尾」——这是轮边界续跑判定与唤醒幂等
        // 的唯一依据（读取进度为推导值，非独立水位）。
        if let Some(ws) = self.working_sets.get(&sid).await {
            ws.mark_consumed(ws.last_seq());
        }

        messages
    }

    /// 持久化注入上下文快照到会话头部（JSONL 首行）。    ///
    /// 会话首次加载时调用一次：前缀内容（soul/rules/memories）随会话固化，
    /// 重启后 `load_and_build_state` 直接恢复，不重新检索——旧会话前缀稳定，    /// prompt 缓存不失效。失败仅告警（本次运行内存缓存仍生效）。
    pub(crate) async fn persist_injectable_snapshot(
        &self,
        session_id: &str,
        ctx: &InjectableContext,
    ) {
        self.update_session_header(session_id, "持久化注入上下文快照", |h| {
            h.injectable_snapshot = Some(ctx.clone());
        })
        .await;
    }

    /// 清空注入上下文快照（压缩点调用）：内存缓存 + 持久化快照一并失效，
    /// 下一轮 assemble_context 重新加载并固化新快照。
    async fn invalidate_injectable_snapshot(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) {
        state.write().await.injectable_context = InjectableContext::default();
        self.update_session_header(session_id, "清空注入上下文快照", |h| {
            h.injectable_snapshot = None;
        })
        .await;
    }

    /// 更新会话头部字段的公共骨架（get_session → 修改 header → update_session）。
    ///
    /// 会话不存在或更新失败仅告警（内存状态仍生效）；`what` 用于告警日志
    /// 区分调用方。三个会话头部持久化点（注入快照 / 待处理追问 / 快照失效）
    /// 共用，消除逐份复制的 get/update 样板。
    async fn update_session_header(
        &self,
        session_id: &str,
        what: &str,
        f: impl FnOnce(&mut crate::session::SessionHeader),
    ) {
        let Ok(Some(mut session)) = self.session_manager.get_session(session_id).await else {
            return;
        };
        f(&mut session.header);
        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::warn!(error = %e, session = %session_id, "更新会话头部失败（{what}）");
        }
    }

    /// 判断是否需要压缩，如果需要则生成摘要 StructuredMessage 并持久化。
    ///
    /// 压缩成功后执行会话转换点刷新（自动/手动压缩共用）：
    /// 清空 injectable_context 缓存 —— 下一轮 `assemble_context` 重新加载，
    /// learned rules / memories 最新化（GEPA 经验对会话后续阶段可见）。
    ///
    /// 压缩已重建 system 前缀（摘要消息插入），此刻刷新零额外缓存成本，
    /// 是会话内唯一的免费刷新点。
    ///
    /// 返回压缩生成的摘要消息（`Some` = 发生了压缩；`None` = 无需压缩）。
    /// 自动压缩调用方忽略返回值（压缩检查只是副作用）；手动压缩透传给前端。
    pub(crate) async fn maybe_compress_and_persist(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
        force: bool,
    ) -> Option<StructuredMessage> {
        let messages_since_marker: Vec<StructuredMessage> = {
            let s = state.read().await;
            let marker_pos = s
                .structured_messages
                .iter()
                .rposition(|m| m.compression_marker);
            let start_idx = marker_pos.unwrap_or(0);
            s.structured_messages[start_idx..].to_vec()
        };

        if messages_since_marker.len() < MIN_MESSAGES_BEFORE_COMPRESSION {
            return None;
        }

        let conversation: Vec<Message> = messages_since_marker
            .iter()
            .flat_map(ContextAssembler::structured_to_messages)
            .collect();

        // 真实上下文占用：最近一次请求的 prompt 侧 token（取数口径单点见
        // `StructuredMessage::prompt_side_tokens`——`tokens.input` 是**完整
        // 输入**（含缓存命中部分，`cache.read` 为其子集），两者相加会重复
        // 计算（判定值≈真实值×2，真实占用远低于阈值也会提前压缩）。
        // 压缩点自身不参与——其 tokens 是摘要请求（压缩前上下文重发）的
        // 用量，不代表会话当前占用。从消息持久化的 usage 聚合——最后一条
        // 带 usage 的消息即最近一次请求的输入统计。
        let recent_input_tokens = messages_since_marker
            .iter()
            .rev()
            .find(|m| !m.compression_marker && (m.tokens.input > 0 || m.tokens.cache.read > 0))
            .map(|m| m.prompt_side_tokens())
            .unwrap_or(0);

        let summary_sm = self
            .context_pipeline
            .compress_for_session(&conversation, session_id, recent_input_tokens, force)
            .await?;
        let summary_sm = summary_sm.clone();

        if let Err(e) = self
            .persist_structured(state, session_id, summary_sm.clone(), true)
            .await
        {
            tracing::warn!(error = %e, "持久化压缩摘要失败");
        }

        // 会话转换点刷新（learned rules 缓存）
        self.invalidate_injectable_snapshot(state, session_id).await;

        Some(summary_sm)
    }

    /// 共享编排骨架：快照（可选）→ 持久化用户消息 → 准备上下文 → AgentLoop →
    /// 结果组装 → 指标更新 → 压缩检查（可选）。
    ///
    /// `process_message` 与 `process_message_stream` 两条路径共用此骨架；
    /// 差异段由 [`TurnOptions`] 参数化：
    /// - `mode`：Plain（构造 `AgentResponse`）或 Stream（`run_stream` + 流式事件）
    /// - `do_snapshot` / `do_compress`：快照与压缩检查开关（流式轮此前缺失
    ///   压缩检查的漂移在此修复）。
    /// - `model` — 本次使用的模型（调用方已解析）。
    /// - `start` — 由调用方传入，保证 `processing_time_ms` 测量窗口与调用前一致。
    /// - `parent_id` — 在 `assemble_context` 之后统一计算（挂接在当前用户消息之下），
    ///   与 process_message 原行为一致（澄清路径的回复链随之修正为 回答→回复）。
    /// - `cancel` — 取消标志（客户端断开/服务关停时置位）；`None` 表示不可取消。
    // 共享骨架：7 个参数均为单轮演进所需状态/依赖，收敛为结构体反而降低可读性
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_agent_turn(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
        message: &Message,
        model: &str,
        start: Instant,
        cancel: Option<&AtomicBool>,
        user_message_id: Option<&str>,
        options: TurnOptions,
    ) -> Result<AgentResponse> {
        // 0. Capture workspace snapshot (before message processing, for session rollback)
        if options.do_snapshot {
            let s = state.read().await;
            self.capture_workspace_snapshot(session_id, &s).await;
        }

        // 1. Persist current user message
        // 流式路径：入库后立即发送确认事件（ADR-031：乐观渲染的 id 回显——
        // 携带前端 user_message_id 与落库后的真实 message_id，前端比对后
        // 把本地乐观消息替换为真实 id）。由入库侧发送保证确认与入库时序
        // 一致——服务端 pump 不再读取"最后一条消息"猜测当前轮归属（修复
        // 第二轮竞态：用户消息未入库时误发上一轮 assistant 边界，导致前端
        // 把上一轮输出合并进本轮占位而重复显示）。
        let persisted_user = self.persist_user_message(session_id, state, message).await;
        if let TurnMode::Stream { sender } = &options.mode {
            if let Some(sm) = persisted_user {
                // ADR-031：乐观渲染的 id 回显（前端比对 user_message_id 后
                // 把本地乐观消息替换为真实 id）——与消息边界并存（波次过渡：
                // 前端乐观渲染落地后边界事件按 id 去重自动失效，波次 4 移除）。
                if let Some(umid) = user_message_id {
                    sender.send_user_message_id(umid, &sm.id).await;
                }
                sender.send_message_boundary(sm).await;
            }
        }

        // 1.5 工具表变化检测（仅真用户轮）：MCP 增删 / 配置热重载 → 主动压缩。
        //
        // 设计取舍（2026-09 讨论）：请求侧工具定义**每轮现取**全局注册表——
        // 工具变化天然在下一轮可见；本检测的价值是把"因工具变化导致的提示词
        // 前缀缓存失效"与一次压缩**合并到同一轮**（变化立即可见 + 掉缓存值得），
        // 且只对主会话的用户轮付一次指纹成本。
        if options.do_toolset_check {
            let current = self.agent_loop.tool_registry().toolset_fingerprint().await;
            let changed = {
                let mut s = state.write().await;
                match s.toolset_fingerprint {
                    Some(prev) if prev == current => false,
                    Some(_) => {
                        s.toolset_fingerprint = Some(current);
                        true
                    }
                    None => {
                        // 首次（新会话/重启后首轮）：只记录基线，不触发压缩
                        s.toolset_fingerprint = Some(current);
                        false
                    }
                }
            };
            if changed {
                tracing::info!(session_id, "工具表已变化：主动压缩以重建前缀");
                // force=true：主动重建（消息不足时由压缩门槛自然拒绝）
                self.maybe_compress_and_persist(state, session_id, true)
                    .await;
            }
        }

        // 2. Prepare context（用户消息已由 persist_user_message 写入 state）
        let messages = self.assemble_context(state, &message.content).await;

        // 3. Run agent loop (internal persistence in AgentLoop)
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let loop_result = match &options.mode {
            TurnMode::Plain => {
                self.agent_loop
                    .clone()
                    .run(
                        &mut messages.clone(),
                        session_id,
                        parent_id.as_deref(),
                        model,
                        cancel,
                        options.thinking_effort,
                    )
                    .await
            }
            TurnMode::Stream { sender } => {
                self.agent_loop
                    .clone()
                    .run_stream(
                        &mut messages.clone(),
                        sender.clone(),
                        session_id,
                        parent_id.as_deref(),
                        model,
                        cancel,
                        options.thinking_effort,
                    )
                    .await
            }
        };

        // 4. Handle loop result (mode-aware output assembly)
        let (response, loop_tokens) = self
            .apply_loop_result(state, loop_result, start, &options.mode)
            .await;

        // 5. Update agent metrics
        self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
            .await;

        // 6. Compression check and persist (optional per path semantics)
        if options.do_compress {
            self.maybe_compress_and_persist(state, session_id, false)
                .await;
        }

        Ok(response)
    }

    /// 组装 AgentLoop 结果：公共部分（状态更新 / 指标记录 / token 收集）集中，
    /// 输出按模式分叉——Plain 构造 [`AgentResponse`]；Stream 发送流式完成事件
    /// 并返回占位响应（内容已在 `run_stream` 阶段逐 chunk 推送完毕）。
    async fn apply_loop_result(
        &self,
        state: &Arc<RwLock<SessionState>>,
        loop_result: Result<AgentLoopResult>,
        start: Instant,
        mode: &TurnMode,
    ) -> (AgentResponse, Option<TokenUsage>) {
        let mut loop_tokens: Option<TokenUsage> = None;
        let response = match loop_result {
            Ok(AgentLoopResult::Answer {
                content,
                total_tokens,
                last_turn_usage,
                persisted_message,
                ..
            }) => {
                // T4：模型 finish_reason 已由 loop 持久化到 StructuredMessage.finish，
                // 流式完成事件复用该值（`persisted_message` 随后被 move 进 state）
                let finish_reason = persisted_message.finish.clone();
                // 消息已由 AgentLoop 落库（经工作集时其内存态同步已推进，
                // 见 `persist_structured`）；此处幂等补写内存态，覆盖回退路径
                push_message_if_absent(state, &persisted_message).await;

                self.metrics.record_execution(true).await;
                loop_tokens = Some(total_tokens.clone());
                match mode {
                    TurnMode::Plain => {
                        finalize_response(AgentResponse::simple(content), &total_tokens, start)
                    }
                    TurnMode::Stream { sender } => {
                        // 上下文占用语义用最后一轮单轮用量（跨轮累计值只用于
                        // 计费统计，不能当当前输入窗口展示——否则多轮工具
                        // 循环后前端显示超过 100% 的假占用）
                        let usage = last_turn_usage
                            .clone()
                            .unwrap_or_else(|| total_tokens.clone());
                        sender
                            .send_complete(
                                "",
                                StreamChunkType::Answer,
                                None,
                                finish_reason,
                                Some(usage),
                            )
                            .await;
                        finalize_response(AgentResponse::simple(content), &total_tokens, start)
                    }
                }
            }

            Ok(AgentLoopResult::MaxTurnsReached {
                content,
                total_tokens,
                last_turn_usage,
                turns,
            }) => {
                // ADR-030：尽力而为策略（子代理）达到轮数上限——返回最后
                // 输出（消息已逐轮持久化，不重复落库）
                tracing::info!(turns, "AgentLoop 达到轮数上限（尽力而为）");
                self.metrics.record_execution(true).await;
                loop_tokens = Some(total_tokens.clone());
                match mode {
                    TurnMode::Plain => {
                        finalize_response(AgentResponse::simple(content), &total_tokens, start)
                    }
                    TurnMode::Stream { sender } => {
                        let usage = last_turn_usage.unwrap_or_else(|| total_tokens.clone());
                        sender
                            .send_complete("", StreamChunkType::Answer, None, None, Some(usage))
                            .await;
                        finalize_response(AgentResponse::simple(content), &total_tokens, start)
                    }
                }
            }

            Ok(AgentLoopResult::Cancelled {
                total_tokens,
                last_turn_usage: _,
                turns,
            }) => {
                tracing::info!(turns, "AgentLoop 被取消");
                self.metrics.record_execution(false).await;
                loop_tokens = Some(total_tokens.clone());
                match mode {
                    TurnMode::Plain => finalize_response(
                        AgentResponse::simple("任务已取消".to_string()),
                        &total_tokens,
                        start,
                    ),
                    TurnMode::Stream { sender } => {
                        sender
                            .send_complete("任务已取消", StreamChunkType::Answer, None, None, None)
                            .await;
                        finalize_response(
                            AgentResponse::simple("任务已取消".to_string()),
                            &total_tokens,
                            start,
                        )
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.metrics.record_execution(false).await;
                match mode {
                    TurnMode::Plain => finalize_response(
                        AgentResponse::error(format!("处理失败：{}", e)),
                        &TokenUsage::default(),
                        start,
                    ),
                    TurnMode::Stream { sender } => {
                        sender.send_error(&format!("处理失败：{}", e)).await;
                        finalize_response(
                            AgentResponse::error(format!("处理失败：{}", e)),
                            &TokenUsage::default(),
                            start,
                        )
                    }
                }
            }
        };
        (response, loop_tokens)
    }

    /// 更新全局 Agent 指标（对话计数、token 统计）。
    pub(crate) async fn update_agent_metrics(
        &self,
        session_id: &str,
        loop_tokens: Option<&TokenUsage>,
        success: bool,
    ) {
        let mut st = self.state.write().await;
        st.conversations_processed += 1;
        if let Some(tokens) = loop_tokens {
            st.total_tokens.input += tokens.prompt_tokens;
            st.total_tokens.output += tokens.completion_tokens;
            st.total_tokens.total += tokens.total_tokens;
            self.metrics
                .record_token_usage(TokenRecord {
                    session_id: session_id.to_string(),
                    timestamp: chrono::Utc::now(),
                    total_tokens: tokens.total_tokens,
                    system_prompt_tokens: 0,
                    retrieved_tokens: 0,
                    success,
                })
                .await;
        }
    }

    /// 持久化当前用户消息（**单一创建点**：入库 + 内存状态同一份
    /// StructuredMessage，消息链 id 一致）。
    ///
    /// 返回入库成功后的结构化消息（供流式路径发送消息边界事件；
    /// 入库失败返回 `None`——边界事件缺失时前端保留本地 uuid，不误同步；
    /// 内存状态仍写入，会话可继续）。
    pub(crate) async fn persist_user_message(
        &self,
        session_id: &str,
        state: &Arc<RwLock<SessionState>>,
        message: &Message,
    ) -> Option<StructuredMessage> {
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let images = message.image_urls();
        let user_msg = if images.is_empty() {
            Message::user(message.content.clone())
        } else {
            Message::user_with_images(message.content.clone(), images)
        };
        let user_sm = ContextAssembler::message_to_structured(
            &user_msg,
            session_id,
            parent_id.as_deref(),
            None,
        );
        // 内存状态与入库同步写入（此前 prepare_context 会再建一份不同 id 的
        // 消息，导致状态与库中消息链 id 分叉、assistant 的 parent 悬空）。
        // ADR-035 §4 实施约束：经工作集时**先落库取号、再进上下文**——否则
        // `last_seq` 落后于工作集，轮边界续跑判定会出现假阳性。
        match self
            .persist_structured(state, session_id, user_sm.clone(), true)
            .await
        {
            Ok(_) => Some(user_sm),
            Err(e) => {
                tracing::warn!(error = %e, "持久化用户消息失败");
                // 入库失败仍写内存（会话可继续；工作集路径下 append 未写内存）
                push_message_if_absent(state, &user_sm).await;
                None
            }
        }
    }
}

/// 幂等补写内存态（按消息 id 去重）。
///
/// 工作集路径下 `append` 已把消息写入**同一份**内存态（`load_and_build_state`
/// 返回源），此处不得重复追加；回退路径（未装配工作集）依赖本函数补齐内存态。
/// 只扫尾部若干条（链尾追加语义），避免长会话全表扫描。
async fn push_message_if_absent(state: &Arc<RwLock<SessionState>>, msg: &StructuredMessage) {
    let mut s = state.write().await;
    let n = s.structured_messages.len();
    let start = n.saturating_sub(8);
    if s.structured_messages[start..]
        .iter()
        .any(|m| m.id == msg.id)
    {
        return;
    }
    s.add_structured_message(msg.clone());
}

/// 组装响应公共字段（token 用量 + 处理耗时）。
///
/// Plain/Stream 两路分支共用：消除逐分支重复的
/// `resp.token_usage = ...; resp.processing_time_ms = ...` 样板。
fn finalize_response(
    mut resp: AgentResponse,
    tokens: &TokenUsage,
    start: Instant,
) -> AgentResponse {
    resp.token_usage = tokens.clone();
    resp.processing_time_ms = start.elapsed().as_millis() as u64;
    resp
}

/// 向消息列表注入会话定位信息（assemble_context / prepare_wake_context 共用）。
///
/// 告知 LLM 当前会话 URI，使其可用 `vfs_read` 检索被压缩的原始记录
///（超长输出会被截断；精确查找建议改用 `session_recall`）。
/// 位置固定在 system 前缀（soul/rules）之后、历史消息之前；
/// 同会话内内容恒定，不影响 DeepSeek 前缀缓存。
fn insert_session_hint(messages: &mut Vec<Message>, session_id: &str) {
    let hint = format!(
        "## 会话定位\n当前会话 ID：{}\n若早期对话已被压缩且摘要信息不足，可用 vfs_read 工具读取 tianyan://session/{} 查看原始对话记录（JSONL 格式，含压缩前的完整消息；超长输出会被截断，精确查找建议用 session_recall 按关键词检索）。",
        session_id, session_id
    );
    let insert_at = messages
        .iter()
        .position(|m| m.role != MessageRole::System)
        .unwrap_or(messages.len());
    messages.insert(insert_at, Message::system(hint));
}

/// 任务唤醒转发器（ADR-013：BackgroundTaskManager → [`Agent::process_wake`]）。
///
/// 用 `Weak<Agent>` 打破循环引用（Agent → AgentLoop → ToolRegistry →
/// BackgroundTaskManager → TaskWaker → Agent）；Agent 已销毁时唤醒静默丢弃。
/// `wake` 内部 spawn 独立任务执行唤醒轮，不阻塞任务完成收尾。
///
/// ADR-031/032：唤醒轮流式化——映射器与送达目标由宿主（server）注入
/// （`mapper` 复用主会话的 `map_chunk_to_event`；`deliver` 为统一事件通道），
/// 转发骨架与用户轮 / 子代理共用 [`spawn_stream_forwarder`]（建通道即消费，
/// 死锁形态从结构上不可能再现）。
pub struct AgentWakeForwarder {
    agent: std::sync::Weak<Agent>,
    /// 流式事件映射（chunk → 事件 JSON；与主会话共用实现）。
    mapper: StreamEventMapper,
    /// 事件送达目标（统一事件通道）。
    deliver: Arc<dyn StreamEventDeliver>,
}

impl AgentWakeForwarder {
    /// 创建转发器（引用持有方必须与 `agent` 同一 Arc）。
    pub fn new(
        agent: &Arc<Agent>,
        mapper: StreamEventMapper,
        deliver: Arc<dyn StreamEventDeliver>,
    ) -> Self {
        Self {
            agent: Arc::downgrade(agent),
            mapper,
            deliver,
        }
    }
}

#[async_trait::async_trait]
impl TaskWaker for AgentWakeForwarder {
    async fn wake(&self, session_id: &str) {
        let Some(agent) = self.agent.upgrade() else {
            tracing::debug!("唤醒被丢弃：Agent 已销毁");
            return;
        };
        // ADR-035 §4：唤醒**幂等**（不是入队）——`try_lock` 拿不到即表示该会话
        // 已有 loop 在跑，由其轮边界续跑判定消化新通知，不再另起一轮。
        // 实测（0.4.7 之前）：N 条通知各自 spawn 一个任务排队，依次重读整段
        // 历史、反复汇报（9 轮风暴）；幂等入口把「N 条通知 → 1 个 loop」。
        let Some(guard) = agent.turn_try_lock(session_id).await else {
            tracing::debug!(
                session = %session_id,
                "唤醒跳过：该会话已有 loop 在跑（轮边界续跑判定会消化新通知）"
            );
            return;
        };
        // 无未消费新消息 → 不启动（避免空转一轮）。重建已把消费水位推到库尾
        // （ADR-035 §4），故重启/首次唤醒不会误判为「有新消息」。
        if let Some(ws) = agent.working_set(session_id).await {
            if !ws.has_unconsumed() {
                tracing::debug!(
                    session = %session_id,
                    "唤醒跳过：无未消费新消息"
                );
                return;
            }
        }
        let mapper = self.mapper.clone();
        let deliver = self.deliver.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            // 持锁跨越整个唤醒轮：与 loop 收尾判定同一临界区（无丢通知窗口）
            let _guard = guard;
            // 统一转发器：通道在唤醒轮**开始前**创建并立即开始消费——
            // 唤醒轮期间事件实时流出（此前先 await 整个唤醒轮再取 rx：
            // 输出积压到轮结束才转发，超过通道缓冲还会死锁——1151680）。
            let (sender, forward_handle) =
                spawn_stream_forwarder(session_id.clone(), mapper, deliver);
            // 轮结束（sender 全部释放）→ 消费任务收尾：等待句柄确保
            // 尾部事件已送达（唤醒轮输出不丢最后一块）。
            agent.process_wake_locked(&session_id, sender).await;
            let _ = forward_handle.await;
        });
    }
}

/// TaskWaker → CommandWaker 适配器（后台命令唤醒复用委托层唤醒器）。
struct CommandWakeAdapter(Arc<dyn TaskWaker>);

#[async_trait::async_trait]
impl crate::executor::CommandWaker for CommandWakeAdapter {
    async fn wake(&self, session_id: &str) {
        self.0.wake(session_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session_state::SessionState;
    use crate::common::error::TianyanError;
    use crate::common::types::{InjectableContext, MessageRole};
    use crate::context::assembler::ContextAssembler;

    // ── 编排路径集成测试（MockChatService + 真实 ContextPipeline/ToolRegistry）──

    use crate::agent::r#loop::AgentLoopConfig;
    use crate::agent::tool_registry::ToolRegistry;
    use crate::agent::AgentCoordinator;
    use crate::common::types::{FunctionCall, ToolCall, ToolCallType};
    use crate::context::compression::{CompressionConfig, ContextCompressor};
    use crate::context::retrieval::DualLayerRetriever;
    use crate::executor::SecurityPolicy;
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::MockChatService;
    use crate::session::{Session, SessionManager};
    use crate::test_utils::MockVfs;
    use crate::vfs::VirtualFileSystem;
    use async_trait::async_trait;
    use tokio::sync::Mutex as TokioMutex;

    struct MockSessionManager;
    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> Result<()> {
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> Result<()> {
            Ok(())
        }
        async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(_id))
        }
        async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn update_session(&self, _session: &Session) -> Result<()> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(vec![])
        }
        async fn delete_session(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    fn response_with(assistant: Message) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "resp-1".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: assistant,
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    /// 流式完成 chunk（ADR-031：唤醒轮流式化后 mock 走 chat_completion_stream）。
    fn stream_chunk_finish(content: &str) -> crate::model::types::ChatCompletionChunk {
        use crate::model::types::{ChunkChoice, DeltaContent};
        crate::model::types::ChatCompletionChunk {
            id: "chunk-f".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaContent {
                    role: None,
                    content: Some(content.to_string()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(TokenUsage::default()),
        }
    }

    /// 仅思考的流式完成 chunk（reasoning-only：有思考无正文——思考模型
    /// "想完没说话"的真实形态，如唤醒轮输出 " 10 秒后见。"）。
    fn stream_chunk_reasoning_only(reasoning: &str) -> crate::model::types::ChatCompletionChunk {
        use crate::model::types::{ChunkChoice, DeltaContent};
        crate::model::types::ChatCompletionChunk {
            id: "chunk-r".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaContent {
                    role: None,
                    content: None,
                    reasoning_content: Some(reasoning.to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(TokenUsage::default()),
        }
    }

    /// 流式 mock：固定 chunk 序列（每次调用重放）。
    fn stream_mock(chunks: Vec<crate::model::types::ChatCompletionChunk>) -> MockChatService {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(move |_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunks = chunks.clone();
            tokio::spawn(async move {
                for chunk in chunks {
                    tx.send(Ok(chunk)).await.ok();
                }
            });
            Ok(rx)
        });
        mock
    }

    fn ask_user_msg(question: &str) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_ask".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "ask_user".to_string(),
                    arguments: format!(r#"{{"question": "{}"}}"#, question),
                },
            }],
        )
    }

    fn make_agent(mock: MockChatService) -> Agent {
        make_agent_full(mock, MockChatService::new(), CompressionConfig::default())
    }

    /// 构造带可配置压缩窗口的 Agent（压缩测试专用）。
    #[allow(clippy::too_many_arguments)]
    fn make_agent_full(
        mock: MockChatService,
        compression_mock: MockChatService,
        compression_config: CompressionConfig,
    ) -> Agent {
        let vfs = Arc::new(MockVfs::new());
        let retriever = Arc::new(DualLayerRetriever::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>
        ));
        let compressor = Arc::new(TokioMutex::new(ContextCompressor::new(
            Arc::new(compression_mock),
            compression_config,
        )));
        let context_pipeline = ContextPipeline::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>,
            retriever,
            compressor,
            10,
            5,
        );
        let agent_loop = AgentLoop::new(
            Arc::new(mock),
            ToolRegistry::new(SecurityPolicy::default()),
            Arc::new(MockSessionManager),
            AgentLoopConfig { max_turns: 5 },
        );
        Agent::new(
            "test-model".to_string(),
            context_pipeline,
            AgentMetrics::new(),
            agent_loop,
            Arc::new(MockSessionManager),
            None,
            None,
            WorkingSetRegistry::new(None),
        )
    }

    #[tokio::test]
    async fn test_process_message_ask_user_unconfigured_returns_error() {
        // 回归保护：process_message 编排路径（共享骨架 + 快照/压缩/技能学习 extras）。
        // ask_user 是普通工具：未装配用户问题服务时返回工具失败结果（错误响应）。
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(2).returning(|req| {
            // 第一轮：ask_user 调用（服务未装配 → 工具失败结果）；
            // 第二轮：模型看到失败结果后给出最终回答
            let has_tool_result = req.messages.iter().any(|m| m.role == MessageRole::Tool);
            if has_tool_result {
                Ok(response_with(Message::assistant(
                    "无法追问，基于现有信息继续",
                )))
            } else {
                Ok(response_with(ask_user_msg("测试问题")))
            }
        });
        let agent = make_agent(mock);

        let resp = agent
            .process_message("session-1", &Message::user("帮我处理"), None, None)
            .await
            .unwrap();

        assert_eq!(resp.content, "无法追问，基于现有信息继续");
    }

    #[tokio::test]
    async fn test_assemble_context_injects_session_hint() {
        // 回归保护：组装上下文必须包含会话定位信息（session_id + vfs 检索路径），
        // 使 LLM 在早期对话被压缩后仍能向上检索原始记录（resume 不丢意图的最后一环）。
        let mock = MockChatService::new();
        let agent = make_agent(mock);

        let state = agent.load_and_build_state("session-abc").await.unwrap();
        let query_msg = Message::user("你好");
        // 用户消息经 persist_user_message 单一创建点写入 state（组装不再注入）
        agent
            .persist_user_message("session-abc", &state, &query_msg)
            .await;
        let messages = agent.assemble_context(&state, "你好").await;

        let hint = messages
            .iter()
            .find(|m| m.role == MessageRole::System && m.content.contains("会话定位"));
        assert!(hint.is_some(), "上下文应包含会话定位提示");
        let hint = hint.unwrap();
        assert!(
            hint.content.contains("session-abc"),
            "提示应包含当前会话 ID"
        );
        assert!(
            hint.content.contains("tianyan://session/session-abc"),
            "提示应包含可检索的会话 URI"
        );
        // 提示应位于 system 前缀之后、历史/当前输入之前
        let hint_idx = messages
            .iter()
            .position(|m| m.content == hint.content)
            .unwrap();
        assert!(
            messages[hint_idx + 1..]
                .iter()
                .any(|m| m.role == MessageRole::User),
            "会话定位提示之后应有历史或当前输入"
        );
        assert!(
            messages[..hint_idx]
                .iter()
                .all(|m| m.role == MessageRole::System),
            "会话定位提示之前只应有 system 前缀（soul/rules）"
        );
    }

    #[tokio::test]
    async fn test_persist_user_message_single_creation_point() {
        // 回归保护：用户消息单一创建点——persist_user_message 返回的消息
        // 必须与写入 state 的是同一份（id 一致），assemble_context 不再
        // 重建第二份（此前状态与库中消息链 id 分叉，assistant 的 parent
        // 指向库中不存在的消息）。
        let mock = MockChatService::new();
        let agent = make_agent(mock);

        let state = agent.load_and_build_state("session-1").await.unwrap();
        let query_msg = Message::user("你好");
        let persisted = agent
            .persist_user_message("session-1", &state, &query_msg)
            .await
            .expect("入库应成功");

        let state_last = state.read().await.structured_messages.last().cloned();
        assert_eq!(
            state_last.as_ref().map(|m| m.id.as_str()),
            Some(persisted.id.as_str()),
            "state 中的用户消息应与入库消息同一份（id 一致）"
        );
        // 组装上下文只含一份用户消息（不重复注入）
        let messages = agent.assemble_context(&state, "你好").await;
        let user_count = messages
            .iter()
            .filter(|m| m.role == MessageRole::User && m.content == "你好")
            .count();
        assert_eq!(user_count, 1, "组装上下文应恰好包含一份用户消息");
    }
    #[test]
    fn test_clarification_answer_injected_once() {
        // 回归保护：澄清回答链路中，用户对追问的回答
        // 只注入内存状态一次（单一创建点语义），组装结果不得出现重复回答。
        let mut state = SessionState::new("session-1");
        state.add_user_message("原始问题");
        state.add_structured_message(ContextAssembler::message_to_structured(
            &Message::assistant("为了继续，需要确认：是否允许执行操作？"),
            "session-1",
            None,
            None,
        ));
        // 追问回答的注入（每次调用仅一次）
        state.add_user_message("允许");

        let messages =
            ContextAssembler::assemble(&state.structured_messages, &InjectableContext::default());

        let answer_count = messages
            .iter()
            .filter(|m| m.role == MessageRole::User && m.content.contains("允许"))
            .count();
        assert_eq!(answer_count, 1, "用户回答在组装上下文中只能出现一次");
    }

    #[tokio::test]
    async fn test_compression_refreshes_injectable() {
        // 回归保护：压缩（会话转换点）成功后必须清空 injectable_context
        // （下一轮 prepare_context 重新加载，learned rules 最新化）。
        let mut compress_mock = MockChatService::new();
        compress_mock
            .expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant("这是压缩摘要"))));
        let agent = make_agent_full(
            MockChatService::new(),
            compress_mock,
            CompressionConfig {
                context_window: 2000,
                compression_threshold: 1.0,
                ..CompressionConfig::default()
            },
        );

        let state = agent.load_and_build_state("session-1").await.unwrap();
        // 预置已加载的注入上下文（模拟会话早期已加载 soul/rules/memories）
        state.write().await.injectable_context = InjectableContext {
            soul: "已加载的 soul".to_string(),
            rules_and_experiences: vec!["旧规则".to_string()],
            ..Default::default()
        };
        // 添加 ≥6 条大消息，并携带真实 usage（窗口 2000 × 阈值 1.0 → 触发线 2000）
        let big = "字".repeat(12_000);
        for _ in 0..7 {
            let mut sm = ContextAssembler::message_to_structured(
                &Message::user(big.clone()),
                "session-1",
                None,
                None,
            );
            sm.tokens.input = 3_000;
            state.write().await.add_structured_message(sm);
        }

        let summary = agent
            .maybe_compress_and_persist(&state, "session-1", false)
            .await;
        assert!(summary.is_some(), "token 超阈值且消息数足够时应发生压缩");
        assert!(
            state.read().await.injectable_context.soul.is_empty(),
            "压缩后注入上下文缓存应清空（下一轮重新加载 learned rules）"
        );
    }

    /// 内存态会话管理器（C1 唤醒轮压缩测试专用）：get_session 返回真实
    /// 消息链、add_structured_message 追加、update_session 写回——支撑
    /// "唤醒轮结束 → 压缩点落库"的端到端断言。
    struct InMemorySessionManager {
        session: TokioMutex<Session>,
    }
    #[async_trait]
    impl SessionManager for InMemorySessionManager {
        async fn add_structured_message(
            &self,
            session_id: &str,
            msg: StructuredMessage,
        ) -> Result<()> {
            let mut s = self.session.lock().await;
            if s.session_id == session_id {
                s.messages.push(msg);
            }
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> Result<()> {
            Ok(())
        }
        async fn create_session(&self, id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(id))
        }
        async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
            let s = self.session.lock().await;
            if s.session_id == session_id {
                Ok(Some(s.clone()))
            } else {
                Ok(None)
            }
        }
        async fn update_session(&self, session: &Session) -> Result<()> {
            *self.session.lock().await = session.clone();
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(vec![])
        }
        async fn delete_session(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    /// 构造带自定义会话管理器的 Agent（C1 唤醒轮压缩测试专用）。
    fn make_agent_with_sessions(
        session_manager: Arc<dyn SessionManager>,
        mock: MockChatService,
        compression_mock: MockChatService,
        compression_config: CompressionConfig,
    ) -> Agent {
        let vfs = Arc::new(MockVfs::new());
        let retriever = Arc::new(DualLayerRetriever::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>
        ));
        let compressor = Arc::new(TokioMutex::new(ContextCompressor::new(
            Arc::new(compression_mock),
            compression_config,
        )));
        let context_pipeline = ContextPipeline::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>,
            retriever,
            compressor,
            10,
            5,
        );
        let agent_loop = AgentLoop::new(
            Arc::new(mock),
            ToolRegistry::new(SecurityPolicy::default()),
            session_manager.clone(),
            AgentLoopConfig { max_turns: 5 },
        );
        Agent::new(
            "test-model".to_string(),
            context_pipeline,
            AgentMetrics::new(),
            agent_loop,
            session_manager,
            None,
            None,
            WorkingSetRegistry::new(None),
        )
    }

    #[tokio::test]
    async fn test_process_wake_triggers_compression_check() {
        // C1 回归：唤醒轮同样执行轮末压缩检查——高负载区间若主要由唤醒轮
        // 推进（等子代理报告/后台通知），此前无检查使上下文持续增长而压缩
        // 被无限推迟（实测：60.8%→81.7% 区间零压缩）。修复后唤醒轮结束即
        // 检查：超阈值场景应落库压缩点。
        let mut session = Session::new("session-1");
        // 7 条超阈值消息（窗口 2000 × 阈值 1.0 → 触发线 2000；实测值 3000）
        for _ in 0..7 {
            let mut sm = ContextAssembler::message_to_structured(
                &Message::user("历史消息"),
                "session-1",
                None,
                None,
            );
            sm.tokens.input = 3_000;
            session.add_structured_message(sm);
        }
        let session_manager = Arc::new(InMemorySessionManager {
            session: TokioMutex::new(session),
        });

        let mut compress_mock = MockChatService::new();
        compress_mock
            .expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant("唤醒轮压缩摘要"))));
        let agent = make_agent_with_sessions(
            session_manager.clone(),
            stream_mock(vec![stream_chunk_finish("已汇总")]),
            compress_mock,
            CompressionConfig {
                context_window: 2000,
                compression_threshold: 1.0,
                ..CompressionConfig::default()
            },
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        agent
            .process_wake("session-1", StreamEventSender::new(tx))
            .await;
        let _ = drain.await;

        let saved = session_manager.session.lock().await;
        let markers = saved
            .messages
            .iter()
            .filter(|m| m.compression_marker)
            .count();
        assert_eq!(markers, 1, "唤醒轮结束应执行压缩检查并落库压缩点");
        let marker = saved
            .messages
            .iter()
            .find(|m| m.compression_marker)
            .unwrap();
        let text = marker
            .parts
            .iter()
            .find_map(|p| match p {
                crate::common::types::Part::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            text.contains("唤醒轮压缩摘要"),
            "压缩点内容应来自压缩调用: {text}"
        );
    }

    #[tokio::test]
    async fn test_compress_skipped_without_enough_messages() {
        // 回归保护：消息不足（< MIN_MESSAGES_BEFORE_COMPRESSION）时不压缩，
        // 不得清空注入上下文缓存。
        let agent = make_agent_full(
            MockChatService::new(),
            MockChatService::new(),
            CompressionConfig::default(),
        );

        let state = agent.load_and_build_state("session-1").await.unwrap();
        state.write().await.injectable_context = InjectableContext {
            soul: "已加载的 soul".to_string(),
            ..Default::default()
        };
        for _ in 0..5 {
            let sm = ContextAssembler::message_to_structured(
                &Message::user("简短消息"),
                "session-1",
                None,
                None,
            );
            state.write().await.add_structured_message(sm);
        }

        let summary = agent
            .maybe_compress_and_persist(&state, "session-1", false)
            .await;
        assert!(summary.is_none(), "消息不足不应压缩");
        assert!(
            !state.read().await.injectable_context.soul.is_empty(),
            "未压缩不应清空注入上下文缓存"
        );
    }

    #[tokio::test]
    async fn test_compress_not_triggered_by_double_counted_cache() {
        // 回归保护（double count）：压缩判定基于"最近请求的真实占用"。
        // `tokens.input` 已含缓存命中部分（cache.read 是其子集）——此前
        // `input + cache.read` 重复计算，判定值≈真实×2，真实占用远低于
        // 阈值也会提前压缩。
        //
        // 窗口 10000 × 阈值 0.5 → 触发线 5000；最近请求 input=4800（其中
        // cache.read=4700 命中）→ 真实 4800 < 5000：不得压缩
        // （旧逻辑 4800+4700=9500 > 5000 会误触发）。
        let agent = make_agent_full(
            MockChatService::new(),
            MockChatService::new(),
            CompressionConfig {
                context_window: 10_000,
                compression_threshold: 0.5,
                ..CompressionConfig::default()
            },
        );

        let state = agent.load_and_build_state("session-1").await.unwrap();
        for i in 0..7 {
            let mut sm = ContextAssembler::message_to_structured(
                &Message::user(format!("消息 {i}")),
                "session-1",
                None,
                None,
            );
            if i == 6 {
                sm.tokens.input = 4_800;
                sm.tokens.cache.read = 4_700;
            }
            state.write().await.add_structured_message(sm);
        }

        let summary = agent
            .maybe_compress_and_persist(&state, "session-1", false)
            .await;
        assert!(
            summary.is_none(),
            "真实占用 4800 < 触发线 5000：不应压缩（旧逻辑 4800+4700 会误压）"
        );
    }

    #[tokio::test]
    async fn test_compress_point_own_usage_excluded_from_decision() {
        // 回归保护（6b0112b 第三处改动）：压缩点**自身**的 usage（摘要请求的
        // 上下文重发用量）不计入会话占用——若计入，压缩完成后会立即再次触发压缩。
        // 窗口 10000 × 阈值 0.5 → 触发线 5000；压缩点自身 input=8000，其后
        // 无任何带 usage 的消息 → 修复后应视为"无实测占用"（不压缩）；
        // 旧实现会把压缩点的 8000 当最近占用 → 误触发（本测试在旧实现下必红）。
        let mut compress_mock = MockChatService::new();
        compress_mock
            .expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant("这是压缩摘要"))));
        let agent = make_agent_full(
            MockChatService::new(),
            compress_mock,
            CompressionConfig {
                context_window: 10_000,
                compression_threshold: 0.5,
                ..CompressionConfig::default()
            },
        );

        let state = agent.load_and_build_state("session-1").await.unwrap();
        // 压缩点（摘要消息）：compression_marker=true + 大 usage（摘要请求用量）
        let mut marker = ContextAssembler::message_to_structured(
            &Message::user("对话摘要……"),
            "session-1",
            None,
            None,
        );
        marker.compression_marker = true;
        marker.tokens.input = 8_000;
        state.write().await.add_structured_message(marker);
        // 压缩点之后累积 ≥ MIN_MESSAGES_BEFORE_COMPRESSION 条普通消息（均无 usage）
        for i in 0..6 {
            let sm = ContextAssembler::message_to_structured(
                &Message::user(format!("压缩后消息 {i}")),
                "session-1",
                None,
                None,
            );
            state.write().await.add_structured_message(sm);
        }

        let summary = agent
            .maybe_compress_and_persist(&state, "session-1", false)
            .await;
        assert!(
            summary.is_none(),
            "压缩点自身 usage（8000）不应计入占用；其后无带 usage 消息 → 不压缩"
        );
    }

    #[tokio::test]
    async fn test_compress_triggered_when_real_input_above_threshold() {
        // 正向保护：修复 double count 后，真实占用超过触发线仍正常压缩。
        // 窗口 10000 × 阈值 0.5 → 触发线 5000；最近请求 input=5100（含
        // cache.read=5000）→ 真实 5100 > 5000：应压缩。
        let mut compress_mock = MockChatService::new();
        compress_mock
            .expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant("这是压缩摘要"))));
        let agent = make_agent_full(
            MockChatService::new(),
            compress_mock,
            CompressionConfig {
                context_window: 10_000,
                compression_threshold: 0.5,
                ..CompressionConfig::default()
            },
        );

        let state = agent.load_and_build_state("session-1").await.unwrap();
        for i in 0..7 {
            let mut sm = ContextAssembler::message_to_structured(
                &Message::user(format!("消息 {i}")),
                "session-1",
                None,
                None,
            );
            if i == 6 {
                sm.tokens.input = 5_100;
                sm.tokens.cache.read = 5_000;
            }
            state.write().await.add_structured_message(sm);
        }

        let summary = agent
            .maybe_compress_and_persist(&state, "session-1", false)
            .await;
        let summary = summary.expect("真实占用 5100 > 触发线 5000：应发生压缩");
        assert!(summary.compression_marker, "压缩摘要消息应带压缩标记");
    }

    // ── ADR-013：唤醒轮（process_wake） ─────────────────────────

    #[tokio::test]
    async fn test_process_wake_produces_summary() {
        // ADR-031：唤醒轮流式化——mock 走 chat_completion_stream
        let mock = stream_mock(vec![stream_chunk_finish("三个调研任务汇总：已完成")]);
        let agent = make_agent(mock);
        // 事件通道：接收端丢弃（发送静默失败，不影响唤醒轮本身）
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        drop(rx);
        agent
            .process_wake("session-1", StreamEventSender::new(tx))
            .await;

        let state = agent.state.read().await;
        assert_eq!(
            state.conversations_processed, 1,
            "唤醒轮应计入处理指标（成功）"
        );
    }

    #[tokio::test]
    async fn test_process_wake_empty_answer_retries_once() {
        // 唤醒轮与用户轮同构：空输出重试一次（统一循环框架），
        // 重试仍空才按正常结束处理（非错误）
        let mock = stream_mock(vec![stream_chunk_finish("")]);
        let agent = make_agent(mock);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        drop(rx);
        agent
            .process_wake("session-1", StreamEventSender::new(tx))
            .await;

        let state = agent.state.read().await;
        assert_eq!(
            state.conversations_processed, 1,
            "空输出重试后仍空应正常结束（计入成功）"
        );
    }

    #[tokio::test]
    async fn test_process_wake_reasoning_only_ends_normally() {
        // 回归保护：reasoning-only 输出（有思考无正文——思考模型"想完没
        // 说话"，如 " 10 秒后见。"）在 loop 内部触发一次空响应重试
        // （content 空 + 无工具调用），重试仍空则直接按正常结束落库
        // （前端可见思考，不静默）。此前 process_wake 层还会重试整个
        // 唤醒轮——实测确认这是模型对 system 完成通知的系统性行为
        // （相同上下文重试结果相同），整轮重试只浪费 LLM 调用，已移除。
        // mock 应只被调用 2 次（loop 内部空响应重试一次），不再整轮重试。
        let mut mock = MockChatService::new();
        let call = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_clone = call.clone();
        mock.expect_chat_completion_stream().returning(move |_| {
            call_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            let chunk = stream_chunk_reasoning_only(" 10 秒后见。");
            tokio::spawn(async move {
                tx.send(Ok(chunk)).await.ok();
            });
            Ok(rx)
        });
        let agent = make_agent(mock);

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        drop(rx);
        agent
            .process_wake("session-1", StreamEventSender::new(tx))
            .await;

        let state = agent.state.read().await;
        assert_eq!(
            state.conversations_processed, 1,
            "reasoning-only 应直接正常结束（计入成功）"
        );
        assert_eq!(
            call.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "loop 内部空响应重试一次，process_wake 不再整轮重试"
        );
    }

    #[tokio::test]
    async fn test_prepare_wake_context_requires_report_on_failure() {
        // 回归保护：后台任务失败时唤醒指令必须要求模型汇报（ADR-013
        // shouldReply = allComplete || isTaskFailure——失败唤醒的目的就是
        // 让主 agent 知情并继续处理）。此前指令允许空输出，模型在失败
        // 场景下选择沉默，主 agent 无反馈（"任务失败但没通知"类问题）。
        let agent = make_agent(MockChatService::new());
        // 注入一个失败的后台任务
        let id = agent
            .background_tasks
            .register(
                crate::agent::background::TaskKind::Delegate,
                "测试任务".to_string(),
                "session-1".to_string(),
                0,
                None,
            )
            .await;
        agent
            .background_tasks
            .fail(&id, "mock 失败".to_string())
            .await;

        let state = agent.load_and_build_state("session-1").await.unwrap();
        let messages = agent.prepare_wake_context(&state, "session-1").await;
        let last = messages.last().expect("唤醒指令应存在");
        assert_eq!(last.role, MessageRole::System);
        assert!(
            last.content.contains("必须向用户汇报失败情况"),
            "失败场景唤醒指令必须要求汇报，实际：{}",
            last.content
        );
        assert!(
            !last.content.contains("直接输出空文本结束本轮"),
            "失败场景不得允许空输出，实际：{}",
            last.content
        );
    }

    #[tokio::test]
    async fn test_prepare_wake_context_requires_output_on_success() {
        // 全部成功时唤醒指令也必须要求模型输出汇总（用户期望：通知后
        // LLM 输出结果摘要——此前允许空输出，模型在完成场景下选择沉默，
        // 前端只看到通知看不到汇总，交互断裂）
        let agent = make_agent(MockChatService::new());

        let state = agent.load_and_build_state("session-1").await.unwrap();
        let messages = agent.prepare_wake_context(&state, "session-1").await;
        let last = messages.last().expect("唤醒指令应存在");
        assert!(
            last.content.contains("必须向用户输出汇总"),
            "无失败时也必须输出汇总，实际：{}",
            last.content
        );
        assert!(
            !last.content.contains("直接输出空文本结束本轮"),
            "完成场景不得允许空输出，实际：{}",
            last.content
        );
    }

    /// T1-2（主回归）：失败判定必须**限定本会话 + 时间窗**。
    ///
    /// 判别力：旧实现扫全量跨会话注册表且不看时间——任一会话、任一 3 天内
    /// （TTL 未清）的历史失败都会命中。
    #[test]
    fn test_fresh_failure_predicate_scopes_session_and_time() {
        let now = 1_700_000_000_000_i64;
        let hour = 60 * 60 * 1000;

        assert!(
            is_fresh_failure("s1", "s1", true, Some(now), now),
            "本会话新失败应命中"
        );
        assert!(
            !is_fresh_failure("s1", "s2", true, Some(now), now),
            "其他会话的失败不得影响本会话（T1-2：旧实现跨会话）"
        );
        assert!(
            !is_fresh_failure("s1", "s1", true, Some(now - 3 * 24 * hour), now),
            "3 天历史失败不得命中（T1-2：旧实现吃 TTL 内全部终态）"
        );
        assert!(
            !is_fresh_failure("s1", "s1", false, Some(now), now),
            "非失败任务不命中"
        );
        assert!(
            !is_fresh_failure("s1", "s1", true, None, now),
            "无完成时间（异常数据）保守不命中"
        );
    }

    /// T1-2：其他会话的失败不得把本会话唤醒指令推进失败分支。
    #[tokio::test]
    async fn test_prepare_wake_context_ignores_other_session_failures() {
        let agent = make_agent(MockChatService::new());
        let id = agent
            .background_tasks
            .register(
                crate::agent::background::TaskKind::Delegate,
                "别的会话任务".to_string(),
                "session-other".to_string(),
                0,
                None,
            )
            .await;
        agent
            .background_tasks
            .fail(&id, "mock 失败".to_string())
            .await;

        let state = agent.load_and_build_state("session-1").await.unwrap();
        let messages = agent.prepare_wake_context(&state, "session-1").await;
        let last = messages.last().expect("唤醒指令应存在");
        assert!(
            last.content.contains("必须向用户输出汇总"),
            "其他会话的失败不得进入失败分支（T1-2），实际：{}",
            last.content
        );
    }

    #[tokio::test]
    async fn test_process_wake_retries_then_gives_up() {
        // 失败重试 3 次后静默放弃（ADR-013：重试 2 次后降级），不 panic、不记成功
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream()
            .times(3)
            .returning(|_| Err(TianyanError::Custom("mock: LLM 不可用".to_string())));
        let agent = make_agent(mock);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        drop(rx);
        agent
            .process_wake("session-1", StreamEventSender::new(tx))
            .await;

        let state = agent.state.read().await;
        assert_eq!(state.conversations_processed, 0, "失败重试后不记成功");
    }

    #[tokio::test]
    async fn test_process_wake_streams_events_while_running() {
        // 回归保护：唤醒轮事件必须在**轮进行中**流出（而非积压到轮结束）。
        // 此前转发器先 await 整个 process_wake 再取 rx：输出积压到轮结束
        // 才转发，超过通道缓冲（100）还会死锁（发送端等消费、转发器等轮
        // 结束）——用户报告"后台命令完成后好久没有输出，退出重进才看到"。
        // 本测试：mock 发送第一个 chunk 后阻塞（等信号），消费端必须在轮
        // 结束前收到该 chunk（证明流式实时性）。
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                // 第一个 chunk：立即可见；第二个 chunk 前挂起（模拟长轮）
                tx.send(Ok(stream_chunk_finish(""))).await.ok();
                // 挂起等待（不发送更多）——轮不结束，消费端仍应收到首 chunk
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
            Ok(rx)
        });
        let agent = make_agent(mock);

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let handle = tokio::spawn(async move {
            agent
                .process_wake("session-1", StreamEventSender::new(tx))
                .await;
        });

        // 轮仍在进行（handle 未完成）：消费端必须能在 1s 内收到事件
        // （证明事件在轮进行中实时流出，而非积压到轮结束）。
        let chunk = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("唤醒轮事件应在轮进行中到达（1s 内）");
        assert!(chunk.is_some(), "应收到流式事件");
        assert!(!handle.is_finished(), "此时唤醒轮应仍在进行中（未结束）");

        handle.await.ok();
    }

    #[tokio::test]
    async fn test_wake_forwarder_delivers_events_while_turn_running() {
        // 端到端回归：`AgentWakeForwarder::wake`（任务完成信号的真实入口）
        // 经统一转发器（spawn_stream_forwarder）把事件送达，且**在轮进行中**
        // 即到达。此前 `wake` 先 await 整个 process_wake 再取 rx——输出积压
        // 到轮结束才转发，超过通道缓冲还会死锁（1151680，用户报告"后台命令
        // 完成后好久没有输出，退出重进才看到"）。本测试锁死该路径：
        // 转发器注入 → 轮进行中消费端必须收到事件。
        use crate::agent::stream_forward::{spawn_stream_forwarder, StreamEventDeliver};
        use std::sync::Mutex as StdMutex;

        struct Collect(StdMutex<usize>);
        #[async_trait::async_trait]
        impl StreamEventDeliver for Collect {
            async fn deliver(&self, _sid: &str, _event: serde_json::Value) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                // 首 chunk 立即可见；随后挂起（模拟长轮——轮不结束）
                tx.send(Ok(stream_chunk_finish("汇总中"))).await.ok();
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
            Ok(rx)
        });
        let agent = make_agent(mock);
        let collect = Arc::new(Collect(StdMutex::new(0)));

        // 模拟 wake 入口：转发器先建（通道即消费），sender 交给轮。
        let mapper: StreamEventMapper =
            Arc::new(|_sid, chunk| Some(serde_json::json!({ "delta": chunk.delta })));
        let (sender, forward_handle) = spawn_stream_forwarder("session-1", mapper, collect.clone());
        let turn = tokio::spawn(async move {
            agent.process_wake("session-1", sender).await;
        });

        // 轮仍在进行：送达计数应在 1s 内增长（证明事件实时流出，未积压）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while *collect.0.lock().unwrap() == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            *collect.0.lock().unwrap() > 0,
            "唤醒轮事件应在轮进行中送达（1s 内）——转发器必须建通道即消费"
        );
        assert!(!turn.is_finished(), "此时唤醒轮应仍在进行中（未结束）");

        turn.await.ok();
        let _ = forward_handle.await;
    }
}
