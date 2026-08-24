//! 智能体核心实现。
//!
//! 包含 Agent 结构体定义、构造函数和所有辅助方法。
//! 将 Agent 与 AgentCoordinator trait 分离，消除循环依赖。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as TokioMutex;
use tokio::sync::{OwnedMutexGuard, RwLock};

use crate::agent::background::{BackgroundTaskManager, TaskWaker};
use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::tool_registry::DynamicToolExecutor;
use crate::agent::types::{
    AgentResponse, AgentState, ClarificationQuestion, QuestionType, StreamChunkType,
    StreamEventSender,
};
use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    InjectableContext, Message, MessageRole, StructuredMessage, TokenUsage,
};
use crate::context::{ContextAssembler, ContextPipeline};
use crate::executor::approval::ApprovalWorkflow;
use crate::observability::AgentMetrics;
use crate::observability::TokenRecord;
use crate::session::{Session, SessionManager};
use crate::skills::SkillRefresher;
use crate::snapshot::SnapshotManager;

/// compression_marker 之后至少积累多少条消息才触发压缩。
const MIN_MESSAGES_BEFORE_COMPRESSION: usize = 6;

/// 唤醒轮重试次数（ADR-013：重试 2 次后降级为静默）。
const WAKE_RETRY_LIMIT: usize = 3;
/// 唤醒轮重试间隔。
const WAKE_RETRY_DELAY: Duration = Duration::from_secs(1);

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
    /// 技能注册表刷新钩子：压缩（会话转换点）时增量注册 VFS 学习技能。
    pub(crate) skill_refresher: Option<Arc<dyn SkillRefresher>>,
    /// 会话级轮次锁（ADR-013 串行化）：单会话同一时刻只有一个活动轮；
    /// 唤醒轮与用户轮互斥——wake 排队等待当前轮结束，绝不抢占。
    turn_locks: Arc<TokioMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
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
        skill_refresher: Option<Arc<dyn SkillRefresher>>,
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
            skill_refresher,
            turn_locks: Arc::new(TokioMutex::new(HashMap::new())),
            background_tasks,
            command_tasks,
            approval_workflow,
            pending_approval_fingerprints,
        }
    }

    /// 获取会话级轮次锁（ADR-013 串行化）。
    ///
    /// 持锁期间该会话的任何其他轮次（用户消息 / 唤醒轮）排队等待；
    /// 返回 OwnedMutexGuard（持有 Arc，不依赖 self 借用）。
    pub(crate) async fn turn_guard(&self, session_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.turn_locks.lock().await;
            map.entry(session_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
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
    pub(crate) async fn process_wake(&self, session_id: &str) {
        let _turn = self.turn_guard(session_id).await;
        let start = Instant::now();

        let mut last_err: Option<TianyanError> = None;
        for attempt in 0..WAKE_RETRY_LIMIT {
            if attempt > 0 {
                tokio::time::sleep(WAKE_RETRY_DELAY).await;
            }
            let state = match self.load_and_build_state(session_id).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, "唤醒轮加载会话失败");
                    last_err = Some(e);
                    continue;
                }
            };
            let messages = self.prepare_wake_context(&state).await;
            let parent_id = {
                let s = state.read().await;
                s.structured_messages.last().map(|m| m.id.clone())
            };

            let loop_result = self
                .agent_loop
                .clone()
                .with_allow_empty_answer()
                .run(
                    &mut messages.clone(),
                    None,
                    session_id,
                    parent_id.as_deref(),
                    &self.default_model,
                    None,
                    // 唤醒轮不携带会话思考选择，使用模型默认
                    None,
                )
                .await;

            match loop_result {
                Ok(AgentLoopResult::Answer {
                    content,
                    total_tokens,
                    persisted_message,
                    ..
                }) => {
                    state
                        .write()
                        .await
                        .add_structured_message(*persisted_message);
                    self.update_agent_metrics(session_id, Some(&total_tokens), true)
                        .await;
                    if content.is_empty() {
                        tracing::debug!(session = %session_id, "唤醒轮空输出（模型无需回复）");
                    } else {
                        tracing::info!(session = %session_id, "唤醒轮完成：后台任务结果已汇总");
                    }
                    return;
                }
                Ok(AgentLoopResult::NeedsClarification { .. }) => {
                    // 唤醒轮不应追问用户（无人值守）：放弃，通知已在 transcript
                    tracing::warn!(session = %session_id, "唤醒轮返回追问，忽略（等待用户下次交互）");
                    return;
                }
                Ok(AgentLoopResult::Cancelled { .. }) => {
                    tracing::debug!(session = %session_id, "唤醒轮被取消");
                    return;
                }
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, attempt, "唤醒轮执行失败");
                    last_err = Some(e);
                }
            }
        }
        tracing::warn!(
            error = ?last_err,
            session = %session_id,
            "唤醒轮重试 {} 次后放弃（消息已在会话中，用户下一条消息自然触发）",
            WAKE_RETRY_LIMIT - 1
        );
        let _ = start; // 保留 start 供未来指标扩展
    }

    /// 组装唤醒轮上下文（ADR-013）。
    ///
    /// 与 [`Self::prepare_context`] 的区别：**不添加用户消息**（无用户输入），
    /// 直接组装会话历史（System 完成通知已在其中）+ 会话定位 + 唤醒指令。
    async fn prepare_wake_context(&self, state: &Arc<RwLock<SessionState>>) -> Vec<Message> {
        let injectable = self.ensure_injectable(state, "").await;
        let s = state.read().await;
        let mut messages = ContextAssembler::assemble(&s.structured_messages, &injectable, "");

        // 会话定位信息（与 prepare_context 一致）
        insert_session_hint(&mut messages, &s.session_id);

        // 唤醒指令：告知模型这是后台任务完成触发的自动处理轮
        messages.push(Message::system(
            "## 自动唤醒轮\n这是一轮由后台任务完成（或失败）自动触发的处理轮，不是用户新消息。\n\
             若全部后台任务已完成：汇总各任务结果，并继续原有工作。\n\
             若无需向用户输出任何内容：直接输出空文本结束本轮。",
        ));

        messages
    }

    /// 加载（或复用）可注入上下文（soul/rules/memories 前缀）。
    ///
    /// 会话级缓存 + 首次加载固化快照（提取自 prepare_context，唤醒轮共用）；
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
            match self.context_pipeline.load_injectable(query).await {
                Ok(ctx) => {
                    let injected_rules = ctx.rules_and_experiences.len();
                    if injected_rules > 0 {
                        self.metrics.record_rule_hit(injected_rules).await;
                    }
                    state.write().await.injectable_context = ctx.clone();
                    // 固化注入上下文快照到会话（重启后沿用同一份前缀内容）
                    let sid = state.read().await.session_id.clone();
                    self.persist_injectable_snapshot(&sid, &ctx).await;
                    state.read().await.injectable_context.clone()
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
    pub(crate) async fn load_and_build_state(
        &self,
        session_id: &str,
    ) -> Result<Arc<RwLock<SessionState>>> {
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
            // 恢复待处理追问（session_meta.header_json 持久化）：澄清回答是
            // 独立请求，内存状态不跨请求存活，不恢复则追问回答恒失败
            s.pending_clarification = session.header.pending_clarification.clone();
            for sm in &session.messages {
                s.add_structured_message(sm.clone());
            }
        }
        Ok(state)
    }

    /// 准备当前轮次的上下文消息。
    ///
    /// 执行：写入用户消息 → 检查会话缓存 → 按需加载 injectable → 压缩 → 组装。
    /// soul/rules/memories 仅会话首次加载，后续轮次复用缓存，
    /// 确保 system prompt 前缀稳定以命中 DeepSeek 前缀缓存。
    ///
    /// 图片（`message.content_parts`）随用户消息写入会话状态，经
    /// `ContextAssembler` 组装为多模态传输消息，对 LLM 可见。
    pub(crate) async fn prepare_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        message: &Message,
    ) -> Vec<Message> {
        let query = message.content.clone();
        let images = message.image_urls();
        if images.is_empty() {
            state.write().await.add_user_message(&query);
        } else {
            state
                .write()
                .await
                .add_user_message_with_images(&query, images);
        }

        // 加载（或复用）可注入上下文：soul/rules/memories 前缀
        let injectable = self.ensure_injectable(state, &query).await;

        let s = state.read().await;
        let mut messages = ContextAssembler::assemble(&s.structured_messages, &injectable, "");

        // 注入会话定位信息：早期对话被压缩后，摘要字段可能不足以恢复细节，
        // 告知 LLM 当前会话 URI，使其可用 vfs_read 检索被压缩的原始记录。
        // 位置固定在 system 前缀（soul/rules）之后、历史消息之前，
        // 同会话内内容恒定，不影响 DeepSeek 前缀缓存。
        insert_session_hint(&mut messages, &s.session_id);

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
        let Ok(Some(mut session)) = self.session_manager.get_session(session_id).await else {
            return;
        };
        session.header.injectable_snapshot = Some(ctx.clone());
        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::warn!(error = %e, "持久化注入上下文快照失败");
        }
    }

    /// 持久化待处理追问到会话头部（session_meta.header_json）。
    ///
    /// 澄清回答是独立 HTTP 请求（load_and_build_state 重建内存状态），
    /// 追问不落库则回答轮永远看不到待处理追问——气泡出现后回答恒返回
    /// "当前没有待处理的追问"。设置（ask_user 触发）与清除（回答后）
    /// 都须调用。失败仅告警（同进程内内存状态仍生效）。
    pub(crate) async fn persist_pending_clarification(
        &self,
        session_id: &str,
        pending: &Option<Vec<ClarificationQuestion>>,
    ) {
        let Ok(Some(mut session)) = self.session_manager.get_session(session_id).await else {
            return;
        };
        session.header.pending_clarification = pending.clone();
        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::warn!(error = %e, session = %session_id, "持久化待处理追问失败");
        }
    }

    /// 清空注入上下文快照（压缩点调用）：内存缓存 + 持久化快照一并失效，
    /// 下一轮 prepare_context 重新加载并固化新快照。
    async fn invalidate_injectable_snapshot(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) {
        state.write().await.injectable_context = InjectableContext::default();
        if let Ok(Some(mut session)) = self.session_manager.get_session(session_id).await {
            session.header.injectable_snapshot = None;
            if let Err(e) = self.session_manager.update_session(&session).await {
                tracing::warn!(error = %e, "清空注入上下文快照失败");
            }
        }
    }

    /// 判断是否需要压缩，如果需要则生成摘要 StructuredMessage 并持久化。
    ///
    /// 压缩成功后执行会话转换点刷新（自动/手动压缩共用）：
    /// 1. 清空 injectable_context 缓存 —— 下一轮 `prepare_context` 重新加载，
    ///    learned rules / memories 最新化（GEPA 经验对会话后续阶段可见）；
    /// 2. 触发 [`SkillRefresher`] —— VFS 新进化技能增量注册，call_skill 可用。
    ///
    /// 压缩已重建 system 前缀（摘要消息插入），此刻刷新零额外缓存成本，
    /// 是会话内唯一的免费刷新点。
    ///
    /// 返回是否发生了压缩（供手动压缩入口判断结果）。
    pub(crate) async fn maybe_compress_and_persist(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) -> bool {
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
            return false;
        }

        let conversation: Vec<Message> = messages_since_marker
            .iter()
            .flat_map(ContextAssembler::structured_to_messages)
            .collect();

        // 真实上下文占用：最近一次请求的 prompt 侧 token（input + cache.read）。
        // 从消息持久化的 usage 聚合——最后一条带 usage 的 assistant 消息即
        // 最近一次请求的完整输入统计。
        let recent_input_tokens = messages_since_marker
            .iter()
            .rev()
            .find(|m| m.tokens.input > 0 || m.tokens.cache.read > 0)
            .map(|m| m.tokens.input + m.tokens.cache.read)
            .unwrap_or(0);

        let Some(summary_sm) = self
            .context_pipeline
            .compress_for_session(&conversation, session_id, recent_input_tokens)
            .await
        else {
            return false;
        };

        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, summary_sm.clone())
            .await
        {
            tracing::warn!(error = %e, "持久化压缩摘要失败");
        } else {
            state.write().await.add_structured_message(summary_sm);
        }

        // 会话转换点刷新（learned rules 缓存 + 技能注册表）
        self.invalidate_injectable_snapshot(state, session_id).await;
        if let Some(refresher) = &self.skill_refresher {
            if let Err(e) = refresher.refresh_skills().await {
                tracing::warn!(error = %e, "压缩后技能刷新失败（不影响会话）");
            }
        }

        true
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
    /// - `parent_id` — 在 `prepare_context` 之后统一计算（挂接在当前用户消息之下），
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
        options: TurnOptions,
    ) -> Result<AgentResponse> {
        // 0. Capture workspace snapshot (before message processing, for session rollback)
        if options.do_snapshot {
            let s = state.read().await;
            self.capture_workspace_snapshot(session_id, &s).await;
        }

        // 1. Persist current user message
        // 流式路径：入库后立即发送消息边界事件（携带服务端结构化消息，
        // 前端同步本地 id/内容）。由入库侧发送保证边界与入库时序一致——
        // 服务端 pump 不再读取"最后一条消息"猜测当前轮归属（修复第二轮
        // 竞态：用户消息未入库时误发上一轮 assistant 边界，导致前端把
        // 上一轮输出合并进本轮占位而重复显示）。
        let persisted_user = self.persist_user_message(session_id, state, message).await;
        if let TurnMode::Stream { sender } = &options.mode {
            if let Some(sm) = persisted_user {
                sender.send_message_boundary(sm).await;
            }
        }

        // 2. Prepare context
        let messages = self.prepare_context(state, message).await;

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
                        None,
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
            self.maybe_compress_and_persist(state, session_id).await;
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
                state
                    .write()
                    .await
                    .add_structured_message(*persisted_message);

                self.metrics.record_execution(true).await;
                loop_tokens = Some(total_tokens.clone());
                match mode {
                    TurnMode::Plain => {
                        let mut resp = AgentResponse::simple(content);
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
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
                        let mut resp = AgentResponse::simple(content);
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                }
            }
            Ok(AgentLoopResult::NeedsClarification {
                question,
                tool_call_id,
                options,
                total_tokens,
                ..
            }) => {
                let question_obj = ClarificationQuestion {
                    question,
                    question_type: QuestionType::OpenEnded,
                    options: options.clone(),
                    required: true,
                    tool_call_id,
                };
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                // 持久化到会话头部：澄清回答是独立请求（load_and_build_state
                // 重建状态），不落库则回答轮找不到待处理追问
                let sid = state.read().await.session_id.clone();
                self.persist_pending_clarification(&sid, &Some(vec![question_obj.clone()]))
                    .await;
                let formatted = format_clarification_questions(std::slice::from_ref(&question_obj));
                loop_tokens = Some(total_tokens.clone());
                match mode {
                    TurnMode::Plain => {
                        let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                    TurnMode::Stream { sender } => {
                        // 结构化选项随追问事件下发（前端渲染选项 + 自定义输入）
                        sender.send_clarification(&formatted, options).await;
                        let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
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
                    TurnMode::Plain => {
                        let mut resp = AgentResponse::simple("任务已取消".to_string());
                        resp.cancelled = true;
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                    TurnMode::Stream { sender } => {
                        sender
                            .send_complete("任务已取消", StreamChunkType::Answer, None, None, None)
                            .await;
                        let mut resp = AgentResponse::simple("任务已取消".to_string());
                        resp.cancelled = true;
                        resp.token_usage = total_tokens.clone();
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.metrics.record_execution(false).await;
                match mode {
                    TurnMode::Plain => {
                        let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                    TurnMode::Stream { sender } => {
                        sender.send_error(&format!("处理失败：{}", e)).await;
                        let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                        resp.processing_time_ms = start.elapsed().as_millis() as u64;
                        resp
                    }
                }
            }
        };
        (response, loop_tokens)
    }

    /// 处理用户对追问的回答（流式版）。
    ///
    /// 两条路径：
    /// - **ask_user 工具链**（待处理追问携带 `tool_call_id`）：用户回答
    ///   作为该工具调用的**工具结果**注入上下文（与普通工具轮同构），
    ///   AgentLoop 从工具结果继续本回合——追问是本轮对话的一部分，
    ///   模型看到完整的 调用→结果 对，上下文不丢问题。
    /// - **审批降级**（无 `tool_call_id`）：确认语义（指纹放行被拒操作），
    ///   回答作为用户消息跑一轮（原语义）。
    ///
    /// 增量经 sender 推送（思考/工具/输出逐块到达），前端可实时展示
    /// 而非等待整轮完成。
    pub(crate) async fn handle_clarification_response_stream(
        &self,
        state: &Arc<RwLock<SessionState>>,
        clarification_answers: &str,
        sender: StreamEventSender,
    ) {
        let pending = {
            let s = state.read().await;
            s.pending_clarification.clone()
        };
        let Some(questions) = pending else {
            sender
                .send_complete(
                    "当前没有待处理的追问",
                    StreamChunkType::Answer,
                    None,
                    Some("stop".to_string()),
                    None,
                )
                .await;
            return;
        };
        state.write().await.pending_clarification = None;
        // 清除后同步持久化（回答已受理，重启/请求边界后不应残留追问）
        let sid = state.read().await.session_id.clone();
        self.persist_pending_clarification(&sid, &None).await;

        let session_id = state.read().await.session_id.clone();
        let tool_call_id = questions.iter().find_map(|q| q.tool_call_id.clone());

        // 审批降级链路：与非流式路径一致的确认语义（指纹放行被拒操作）
        let approved = crate::executor::approval::is_user_confirmation(clarification_answers);
        if self
            .agent_loop
            .tool_registry()
            .confirm_pending_approval(approved)
            .await
        {
            tracing::info!(
                approved,
                answer = %clarification_answers,
                "已记录用户对审批追问的回应（流式）"
            );
        }

        // ask_user 工具链：回答作为工具结果，AgentLoop 恢复本回合
        if let Some(tool_call_id) = tool_call_id {
            self.continue_after_clarification(
                state,
                &session_id,
                &tool_call_id,
                clarification_answers,
                sender,
            )
            .await;
            return;
        }

        // 审批降级路径：回答作为用户消息跑一轮（保持原语义）
        let clarification_msg = Message::user(clarification_answers);
        let start = Instant::now();
        let _ = self
            .run_agent_turn(
                state,
                &session_id,
                &clarification_msg,
                &self.default_model,
                start,
                None,
                TurnOptions {
                    mode: TurnMode::Stream { sender },
                    do_snapshot: false,
                    do_compress: false,
                    thinking_effort: None,
                },
            )
            .await;
    }

    /// 澄清续轮（ask_user 工具链）：用户回答作为工具结果注入上下文，
    /// AgentLoop 从工具结果继续本回合——不注入用户消息、不持久化
    /// 用户消息（回答是工具的输入，不是新一轮用户输入）。
    async fn continue_after_clarification(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
        tool_call_id: &str,
        answer: &str,
        sender: StreamEventSender,
    ) {
        // 1. 工具结果消息入库（内存 + session_manager；parent 挂接在
        //    ask_user 调用消息之下——与普通工具轮的 observation 同构）
        let tool_msg = Message::tool(tool_call_id, answer);
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let sm = ContextAssembler::message_to_structured(
            &tool_msg,
            session_id,
            parent_id.as_deref(),
            None,
        );
        state.write().await.add_structured_message(sm.clone());
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, sm)
            .await
        {
            tracing::warn!(error = %e, "持久化追问回答（工具结果）失败");
        }

        // 2. 组装上下文（复用前缀缓存；回答已作为工具结果在消息链上，
        //    不再作为用户消息注入）
        let injectable = self.ensure_injectable(state, answer).await;
        let mut messages = {
            let s = state.read().await;
            ContextAssembler::assemble(&s.structured_messages, &injectable, "")
        };
        insert_session_hint(&mut messages, session_id);

        // 3. AgentLoop 从工具结果继续本回合
        let start = Instant::now();
        let parent = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let loop_result = self
            .agent_loop
            .clone()
            .run_stream(
                &mut messages,
                sender.clone(),
                session_id,
                parent.as_deref(),
                &self.default_model,
                None,
                None,
            )
            .await;

        // 4. 结果处理（完成事件/状态/指标；澄清轮不触发压缩——原语义）
        let (_response, loop_tokens) = self
            .apply_loop_result(state, loop_result, start, &TurnMode::Stream { sender })
            .await;
        self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
            .await;
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

    /// 持久化当前用户消息到 SessionManager。
    ///
    /// 返回入库成功后的结构化消息（供流式路径发送消息边界事件；
    /// 入库失败返回 `None`——边界事件缺失时前端保留本地 uuid，不误同步）。
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
        match self
            .session_manager
            .add_structured_message(session_id, user_sm.clone())
            .await
        {
            Ok(_) => Some(user_sm),
            Err(e) => {
                tracing::warn!(error = %e, "持久化用户消息失败");
                None
            }
        }
    }
}

/// 格式化追问问题为用户友好的文本。
pub(crate) fn format_clarification_questions(questions: &[ClarificationQuestion]) -> String {
    let mut content = String::from("为了更好帮助您，需要以下信息：\n\n");

    for (i, q) in questions.iter().enumerate() {
        content.push_str(&format!("{}. {}\n", i + 1, q.question));

        if let Some(options) = &q.options {
            content.push_str("   可选答案：\n");
            for (j, opt) in options.iter().enumerate() {
                content.push_str(&format!("   {}. {}\n", j + 1, opt));
            }
        }

        if q.required {
            content.push_str("   (必填)\n");
        }

        content.push('\n');
    }

    content.push_str("请提供以上信息，我会根据您的回答继续处理。");
    content
}

/// 向消息列表注入会话定位信息（prepare_context / prepare_wake_context 共用）。
///
/// 告知 LLM 当前会话 URI，使其可用 `vfs_read` 检索被压缩的原始记录。
/// 位置固定在 system 前缀（soul/rules）之后、历史消息之前；
/// 同会话内内容恒定，不影响 DeepSeek 前缀缓存。
fn insert_session_hint(messages: &mut Vec<Message>, session_id: &str) {
    let hint = format!(
        "## 会话定位\n当前会话 ID：{}\n若早期对话已被压缩且摘要信息不足，可用 vfs_read 工具读取 tianyan://session/{} 查看原始对话记录（JSONL 格式，含压缩前的完整消息）。",
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
pub struct AgentWakeForwarder {
    agent: std::sync::Weak<Agent>,
}

impl AgentWakeForwarder {
    /// 创建转发器（引用持有方必须与 `agent` 同一 Arc）。
    pub fn new(agent: &Arc<Agent>) -> Self {
        Self {
            agent: Arc::downgrade(agent),
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
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            agent.process_wake(&session_id).await;
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
        make_agent_full(
            mock,
            MockChatService::new(),
            CompressionConfig::default(),
            None,
        )
    }

    /// 构造带可配置压缩窗口与技能刷新钩子的 Agent（压缩测试专用）。
    #[allow(clippy::too_many_arguments)]
    fn make_agent_full(
        mock: MockChatService,
        compression_mock: MockChatService,
        compression_config: CompressionConfig,
        refresher: Option<Arc<dyn SkillRefresher>>,
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
            AgentLoopConfig {
                max_turns: 5,
                ..Default::default()
            },
        );
        Agent::new(
            "test-model".to_string(),
            context_pipeline,
            AgentMetrics::new(),
            agent_loop,
            Arc::new(MockSessionManager),
            None,
            None,
            refresher,
        )
    }

    /// 记录调用次数的技能刷新钩子 mock。
    struct MockRefresher {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl SkillRefresher for MockRefresher {
        async fn refresh_skills(&self) -> Result<usize> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(0)
        }
    }

    fn refresher_calls(refresher: &MockRefresher) -> usize {
        refresher.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn test_process_message_returns_clarification() {
        // 回归保护：process_message 编排路径（共享骨架 + 快照/压缩/技能学习 extras）。
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(response_with(ask_user_msg("测试问题"))));
        let agent = make_agent(mock);

        let resp = agent
            .process_message("session-1", &Message::user("帮我处理"), None, None)
            .await
            .unwrap();

        assert!(resp.needs_clarification, "ask_user 应触发追问响应");
        assert_eq!(resp.clarification_questions.len(), 1);
        assert_eq!(resp.clarification_questions[0].question, "测试问题");
    }

    #[tokio::test]
    async fn test_prepare_context_injects_session_hint() {
        // 回归保护：组装上下文必须包含会话定位信息（session_id + vfs 检索路径），
        // 使 LLM 在早期对话被压缩后仍能向上检索原始记录（resume 不丢意图的最后一环）。
        let mock = MockChatService::new();
        let agent = make_agent(mock);

        let state = agent.load_and_build_state("session-abc").await.unwrap();
        let query_msg = Message::user("你好");
        let messages = agent.prepare_context(&state, &query_msg).await;

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

    #[test]
    fn test_clarification_answer_injected_once() {
        // 回归保护：澄清回答链路中，用户对追问的回答
        // 通过 prepare_context 仅注入内存状态一次；持久化（persist_user_message）
        // 走 VFS，与上下文组装是两条独立存储，组装结果不得出现重复回答。
        let mut state = SessionState::new("session-1");
        state.add_user_message("原始问题");
        state.add_structured_message(ContextAssembler::message_to_structured(
            &Message::assistant("为了继续，需要确认：是否允许执行操作？"),
            "session-1",
            None,
            None,
        ));
        // prepare_context 对追问回答的注入（每次调用仅一次）
        state.add_user_message("允许");

        let messages = ContextAssembler::assemble(
            &state.structured_messages,
            &InjectableContext::default(),
            "",
        );

        let answer_count = messages
            .iter()
            .filter(|m| m.role == MessageRole::User && m.content.contains("允许"))
            .count();
        assert_eq!(answer_count, 1, "用户回答在组装上下文中只能出现一次");
    }

    #[tokio::test]
    async fn test_compression_refreshes_injectable_and_skills() {
        // 回归保护：压缩（会话转换点）成功后必须清空 injectable_context
        // （下一轮 prepare_context 重新加载，learned rules 最新化）并触发
        // SkillRefresher（VFS 新进化技能增量注册）。
        let mut compress_mock = MockChatService::new();
        compress_mock
            .expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant("这是压缩摘要"))));
        let refresher = Arc::new(MockRefresher {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let agent = make_agent_full(
            MockChatService::new(),
            compress_mock,
            CompressionConfig {
                context_window: 2000,
                compression_threshold: 1.0,
                ..CompressionConfig::default()
            },
            Some(refresher.clone()),
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

        let compressed = agent.maybe_compress_and_persist(&state, "session-1").await;
        assert!(compressed, "token 超阈值且消息数足够时应发生压缩");
        assert!(
            state.read().await.injectable_context.soul.is_empty(),
            "压缩后注入上下文缓存应清空（下一轮重新加载 learned rules）"
        );
        assert_eq!(refresher_calls(&refresher), 1, "压缩后应触发技能注册表刷新");
    }

    #[tokio::test]
    async fn test_compress_skipped_without_enough_messages() {
        // 回归保护：消息不足（< MIN_MESSAGES_BEFORE_COMPRESSION）时不压缩，
        // 不得清空注入上下文缓存、不得触发技能刷新。
        let refresher = Arc::new(MockRefresher {
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let agent = make_agent_full(
            MockChatService::new(),
            MockChatService::new(),
            CompressionConfig::default(),
            Some(refresher.clone()),
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

        let compressed = agent.maybe_compress_and_persist(&state, "session-1").await;
        assert!(!compressed, "消息不足不应压缩");
        assert!(
            !state.read().await.injectable_context.soul.is_empty(),
            "未压缩不应清空注入上下文缓存"
        );
        assert_eq!(refresher_calls(&refresher), 0, "未压缩不应触发技能刷新");
    }

    // ── ADR-013：唤醒轮（process_wake） ─────────────────────────

    #[tokio::test]
    async fn test_process_wake_produces_summary() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().times(1).returning(|_| {
            Ok(response_with(Message::assistant(
                "三个调研任务汇总：已完成",
            )))
        });
        let agent = make_agent(mock);

        agent.process_wake("session-1").await;

        let state = agent.state.read().await;
        assert_eq!(
            state.conversations_processed, 1,
            "唤醒轮应计入处理指标（成功）"
        );
    }

    #[tokio::test]
    async fn test_process_wake_empty_answer_legal() {
        // ADR-013：唤醒轮空输出是合法结束（模型无需回复），非错误
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(response_with(Message::assistant(""))));
        let agent = make_agent(mock);

        agent.process_wake("session-1").await;

        let state = agent.state.read().await;
        assert_eq!(
            state.conversations_processed, 1,
            "空输出应正常结束（计入成功，不重试）"
        );
    }

    #[tokio::test]
    async fn test_process_wake_retries_then_gives_up() {
        // 失败重试 3 次后静默放弃（ADR-013：重试 2 次后降级），不 panic、不记成功
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(3)
            .returning(|_| Err(TianyanError::Custom("mock: LLM 不可用".to_string())));
        let agent = make_agent(mock);

        agent.process_wake("session-1").await;

        let state = agent.state.read().await;
        assert_eq!(state.conversations_processed, 0, "失败重试后不记成功");
    }
}
