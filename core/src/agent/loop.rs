use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::{StreamEventSender, ToolCallEvent};
use crate::common::error::TianyanError;
use crate::common::types::{FunctionCall, Message, MessageRole, StructuredMessage, TokenUsage};
use crate::common::types::{ToolCall, ToolCallType};
use crate::context::ContextAssembler;
use crate::model::spec::ModelSpec;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use crate::observability::trace::TraceCollector;
use crate::observability::usage_log::UsageLog;
use crate::session::SessionManager;

/// Agent 循环配置。
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// 最大轮数。
    pub max_turns: usize,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self { max_turns: 200 }
    }
}

/// 工具执行器（ADR-030：主 agent 与子代理同构的配置点）。
#[derive(Debug, Clone)]
pub enum ToolExecutorKind {
    /// 全局注册表（主 agent：审批可交互）。
    Global,
    /// 角色过滤（子代理：审批不交互，ADR-011——未授权操作拒绝并上报主 agent）。
    /// `role_tools` 为 Some 时白名单外工具不执行（合成错误回喂模型重试）。
    RoleFiltered {
        role_tools: Option<Vec<String>>,
        role_name: Option<String>,
    },
}

/// 单轮演进策略（ADR-030：主 agent 与子代理同构的配置点）。
///
/// 主 agent 用默认策略（全局注册表工具执行 + 索引 FTS + max_turns 报错）；
/// 子代理用委托策略（角色过滤工具执行 + 不索引 FTS（ADR-026）+ max_turns
/// 尽力返回最后输出）。
#[derive(Debug, Clone)]
pub struct TurnPolicy {
    /// 工具执行器。
    pub tool_executor: ToolExecutorKind,
    /// 持久化是否索引 FTS（子代理不索引，ADR-026：子会话不参与回忆检索）。
    pub persist_no_fts: bool,
    /// 达到 max_turns 时的行为：false = 报错（主 agent）；true = 返回最后
    /// 输出（子代理尽力而为，不因轮数上限丢失已完成的工作）。
    pub max_turns_graceful: bool,
    /// 请求构造的工具集（None = 全局注册表；Some = 过滤后的工具集——
    /// 子代理：角色白名单 + submit_result）。
    pub tools: Option<Vec<crate::model::types::ToolDefinition>>,
}

impl Default for TurnPolicy {
    fn default() -> Self {
        Self {
            tool_executor: ToolExecutorKind::Global,
            persist_no_fts: false,
            max_turns_graceful: false,
            tools: None,
        }
    }
}

/// Agent 循环结果。
#[derive(Debug, Clone)]
pub enum AgentLoopResult {
    /// 直接回答。
    Answer {
        /// 回答内容。
        content: String,
        /// Token 用量。
        total_tokens: TokenUsage,
        /// 最后一轮 LLM 调用的单轮用量（上下文占用语义：prompt_tokens 即
        /// 当前请求实际输入窗口；total_tokens 为跨轮累计，仅用于计费统计）。
        last_turn_usage: Option<TokenUsage>,
        /// 循环轮数。
        turns: usize,
        /// AgentLoop 已持久化的 StructuredMessage，coordinator 直接复用此消息加入状态，避免重复创建。
        persisted_message: Box<StructuredMessage>,
    },

    /// 循环被取消（客户端断开或服务关停）。
    Cancelled {
        /// Token 用量。
        total_tokens: TokenUsage,
        /// 最后一轮 LLM 调用的单轮用量（语义同 Answer）。
        last_turn_usage: Option<TokenUsage>,
        /// 已执行的循环轮数。
        turns: usize,
    },

    /// 达到 max_turns 且策略为尽力而为（ADR-030：子代理——不因轮数上限
    /// 丢失已完成的工作，返回最后输出；主 agent 策略为报错，不产生此变体）。
    MaxTurnsReached {
        /// 最后一条非空 assistant 输出（已持久化，调用方从会话读取完整消息）。
        content: String,
        /// Token 用量。
        total_tokens: TokenUsage,
        /// 最后一轮 LLM 调用的单轮用量。
        last_turn_usage: Option<TokenUsage>,
        /// 已执行的循环轮数。
        turns: usize,
    },
}

// AgentLoopError replaced with TianyanError::Custom("agent_loop: ...")

/// 单轮迭代的回合级上下文。
///
/// 收拢 `run` / `run_stream` 两条路径中逐轮演进的可变状态，
/// 使 [`AgentLoop::handle_llm_response`] 只需一个数据入口，
/// 后续新增回合级状态（如流式缓冲）无需再改函数签名。
struct TurnContext<'a> {
    /// 当前会话 ID。
    session_id: &'a str,
    /// 上一条已持久化消息的 ID（消息链父节点）。
    current_parent_id: &'a mut Option<String>,
    /// 全轮累计 token 用量。
    total_tokens: &'a mut TokenUsage,
    /// 内存中的消息历史。
    messages: &'a mut Vec<Message>,
    /// 流式事件发送器（非流式路径为 None）。
    stream_sender: Option<&'a StreamEventSender>,
    /// 当前轮数（0 起）。
    turn: usize,
    /// 当前模型（持久化到消息 model_id，供实测输入按模型恢复）。
    model: &'a str,
}

/// 单轮响应获取步骤返回的 future（`run` / `run_stream` 传入 [`AgentLoop::run_turns`]）。
/// 输出为 [`TurnResult`]（含 finish_reason、实测输入与取消收尾标记）。
type TurnStep<'a> = Pin<Box<dyn Future<Output = Result<TurnResult, TianyanError>> + Send + 'a>>;

/// 单轮模型响应结果 `(assistant_msg, usage, finish_reason, 实测输入, 取消收尾)`。
///
/// `run` / `run_stream` 的 step 产出与 [`AgentLoop::run_turns`] 归约共用同一形式；
/// 提炼为别名以消除复杂元组的内联重复。
/// 第三元素为模型响应的 finish_reason；第四元素为更新后的实测输入
/// （`(model, prompt_tokens)`，供下一轮动态 max_tokens 校准）；
/// 第五元素为取消收尾标记（true = 流式接收中检测到取消：tool_calls 已丢弃、
/// 消息需落库后终止轮次——见 [`AgentLoop::finalize_streamed_turn`]）。
type TurnResult = (
    Message,
    Option<TokenUsage>,
    Option<String>,
    Option<(String, usize)>,
    bool,
);

/// 流式单轮 chunk 累积结果（`run_stream` 路径内部用）。
///
/// 收敛「接收 chunk → 累积正文/推理/工具调用」过程的全部可变状态，
/// 使累积循环与结果组装（[`AgentLoop::finalize_streamed_turn`]）解耦。
struct StreamAccum {
    /// 已累积的正文增量。
    content: String,
    /// 已累积的推理（思考）增量。
    reasoning: String,
    /// 按 index 累积的工具调用增量 `(id, name, arguments)`。
    tool_calls: std::collections::BTreeMap<usize, (String, String, String)>,
    /// 流式 usage（网关未返回时为 None，交由组装阶段估算）。
    usage: Option<TokenUsage>,
    /// 流式接收错误（中断）。
    stream_error: Option<String>,
    /// finish_reason（最终 chunk 携带；None 表示未到达）。
    finish_reason: Option<String>,
    /// 本轮实测输入（usage 到达时更新）。
    updated_input: Option<(String, usize)>,
    /// 取消收尾标记（chunk 循环内检测到取消置位；累积保留、tool_calls 丢弃）。
    cancelled: bool,
}

/// 通知投递水位初值（T1-23）。
///
/// 会话消息 seq 从 **0** 起（`COALESCE(MAX(seq), -1) + 1`），故"尚未投递
/// 任何消息"必须用 `-1`——用 0 会跳过首条（seq=0）通知。
const NOTICE_WATERMARK_INIT: i64 = -1;

/// Agent 迭代循环。
#[derive(Clone)]
pub struct AgentLoop {
    model_service: Arc<dyn ChatService>,
    tool_registry: ToolRegistry,
    session_manager: Arc<dyn SessionManager>,
    config: AgentLoopConfig,
    /// 结构化 Trace 收集器（G6；None 时不记录轮 span）。
    trace: Option<Arc<TraceCollector>>,
    /// 当前聊天模型的上下文规格（T5；由 AgentBuilder 注入，供请求构造/压缩联动使用）。
    pub(crate) chat_spec: Option<ModelSpec>,
    /// LLM 用量日志（token 统计：每轮调用落库；None 时不记录）。
    usage_log: Option<Arc<UsageLog>>,
    /// model → provider 映射（配置注入；用量日志的 provider 维度）。
    provider_by_model: HashMap<String, String>,
    /// 单轮演进策略（ADR-030：主 agent 默认；子代理委托策略）。
    turn_policy: TurnPolicy,
    /// 事件通知投递水位（T1-23）：session_id → 已投递给模型上下文的会话 seq。
    ///
    /// 轮边界增量投递据此只投递"新落库且尚未被任何轮读到"的后台事件通知；
    /// 投递与水位推进在同一临界区内完成 → 每个通知**恰好投递一次**（会话轮
    /// 由 turn_guard 互斥，轮内检查点与唤醒组装不会并发投递）。
    notice_delivered: Arc<tokio::sync::Mutex<HashMap<String, i64>>>,
}

impl AgentLoop {
    /// 创建新的 AgentLoop。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        tool_registry: ToolRegistry,
        session_manager: Arc<dyn SessionManager>,
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            model_service,
            tool_registry,
            session_manager,
            config,
            trace: None,
            chat_spec: None,
            usage_log: None,
            provider_by_model: HashMap::new(),
            turn_policy: TurnPolicy::default(),
            notice_delivered: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 设置单轮演进策略（ADR-030：主 agent 默认；子代理委托策略）。
    pub fn with_turn_policy(mut self, policy: TurnPolicy) -> Self {
        self.turn_policy = policy;
        self
    }

    /// 设置聊天模型上下文规格（T5：由 AgentBuilder 从模型配置注入；None 时走默认窗口）。
    pub fn with_chat_spec(mut self, spec: Option<ModelSpec>) -> Self {
        self.chat_spec = spec;
        self
    }

    /// 设置结构化 Trace 收集器（G6：轮次 span 记录；None 时不记录）。
    pub fn with_trace_collector(mut self, collector: Arc<TraceCollector>) -> Self {
        self.trace = Some(collector);
        self
    }

    /// 设置 LLM 用量日志（token 统计；None 时不记录）。
    pub fn with_usage_log(mut self, log: Arc<UsageLog>) -> Self {
        self.usage_log = Some(log);
        self
    }

    /// 设置 model → provider 映射（用量日志的 provider 维度；
    /// 空表时回落 model 名前缀推导）。
    pub fn with_provider_by_model(mut self, map: HashMap<String, String>) -> Self {
        self.provider_by_model = map;
        self
    }

    /// 获取工具注册表引用。
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// 轮边界增量投递（T1-23）：把"已落库但尚未投递给模型"的后台事件通知
    /// 追加到本轮上下文尾部。
    ///
    /// 为什么需要：上下文在本轮启动时组装一次，turn 之间不重读历史——活动轮
    /// 期间落库的就绪/完成/失败通知此前只能等"下一个 loop"才被读到：期间
    /// 用户已在面板看到任务终态而模型毫不知情（"右侧完成了、主会话没反应"），
    /// 且积压的唤醒请求会让同一批历史被反复重读、反复汇报（实测主轮收尾后
    /// 连跑 9 轮）。轮边界投递让活动轮自行消化通知，唤醒只留给"没有后续
    /// turn 能读到"的场景（轮收尾检查）。
    ///
    /// 恰好一次：只取 `delivered_seq` 之后的**事件通知**；投递与水位推进同一
    /// 临界区完成（会话轮互斥 → 无并发投递）。
    pub(crate) async fn deliver_pending_notices(
        &self,
        messages: &mut Vec<Message>,
        session_id: &str,
    ) {
        let Some(store) = &self.tool_registry.session_store else {
            return; // 未装配会话存储（测试桩）：无投递来源
        };
        let last = match store.last_seq(session_id).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let delivered = {
            self.notice_delivered
                .lock()
                .await
                .get(session_id)
                .copied()
                .unwrap_or(NOTICE_WATERMARK_INIT)
        };
        if last <= delivered {
            return;
        }
        let rows = match store.load_after(session_id, delivered).await {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut appended = 0usize;
        for (_seq, msg) in &rows {
            if !crate::agent::background::is_event_notice(msg) {
                continue;
            }
            messages.push(Message::system(crate::agent::background::message_text(msg)));
            appended += 1;
        }
        // 水位推进（含被跳过的非通知消息：它们无需投递，但已"处理到"）
        self.notice_delivered
            .lock()
            .await
            .insert(session_id.to_string(), last);
        if appended > 0 {
            tracing::info!(
                session = %session_id,
                appended,
                "轮边界投递后台事件通知（T1-23）"
            );
        }
    }

    /// 推进通知投递水位到指定 seq（T1-23）。
    ///
    /// 唤醒轮组装会读取**全量历史**（含已落库通知）——组装**前**记录水位
    /// （组装读到的上界）并在组装后调用本方法推进到该值，表示"这批通知本轮
    /// 已读到"；组装与推进之间新落库的通知（水位之后）仍保持 pending，
    /// 由下一个 turn 或轮收尾检查兜住（不丢、不重复）。
    pub(crate) async fn mark_notices_delivered(&self, session_id: &str, seq: i64) {
        self.notice_delivered
            .lock()
            .await
            .insert(session_id.to_string(), seq);
    }

    /// 读当前会话末尾 seq（T1-23：唤醒轮组装前记录水位上界）。
    pub(crate) async fn store_last_seq(&self, session_id: &str) -> Option<i64> {
        let store = self.tool_registry.session_store.as_ref()?;
        store.last_seq(session_id).await.ok()
    }

    /// 是否仍有"已落库但未被任何轮读到"的事件通知（T1-23）。
    ///
    /// 轮收尾检查（补唤醒）与唤醒入口（水位失效跳过）共用同一判定。
    pub(crate) async fn has_pending_notices(&self, session_id: &str) -> bool {
        let Some(store) = &self.tool_registry.session_store else {
            // 无存储（测试桩）：保持旧行为（视为有，不跳过唤醒）
            return true;
        };
        let last = match store.last_seq(session_id).await {
            Ok(v) => v,
            Err(_) => return false,
        };
        let delivered = {
            self.notice_delivered
                .lock()
                .await
                .get(session_id)
                .copied()
                .unwrap_or(NOTICE_WATERMARK_INIT)
        };
        if last <= delivered {
            return false;
        }
        match store.load_after(session_id, delivered).await {
            Ok(rows) => rows
                .iter()
                .any(|(_, m)| crate::agent::background::is_event_notice(m)),
            Err(_) => false,
        }
    }

    /// 轮收尾补唤醒（T1-23）：若仍有未投递事件通知 → 请求一轮唤醒。
    ///
    /// 这就是"唤醒的条件"：只有当**没有后续 turn 能读到**通知时才醒
    /// （主轮循环中的通知由下一个 turn 投递消化，不唤醒）。
    pub(crate) async fn wake_if_pending_notices(&self, session_id: &str) {
        if self.has_pending_notices(session_id).await {
            self.tool_registry
                .background_tasks
                .request_wake(session_id)
                .await;
        }
    }

    /// `run_turns` 的收尾包装（T1-23）：轮结束后检查是否有未投递事件通知
    /// （有 → 补一轮唤醒；由 `AgentWakeForwarder` 再判水位失效）。
    #[allow(clippy::too_many_arguments)]
    async fn run_turns_with_notice_check<F>(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<&StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
        cancel: Option<&AtomicBool>,
        step: F,
    ) -> Result<AgentLoopResult, TianyanError>
    where
        F: for<'a> FnMut(
            &'a AgentLoop,
            &'a str,
            Option<&'a StreamEventSender>,
            Vec<Message>,
            Option<&'a AtomicBool>,
            Option<(String, usize)>,
        ) -> TurnStep<'a>,
    {
        let result = self
            .run_turns(
                messages,
                stream_sender,
                session_id,
                initial_parent_id,
                model,
                cancel,
                step,
            )
            .await;
        self.wake_if_pending_notices(session_id).await;
        result
    }

    /// 取消标志检查（None 视为未取消）。
    fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
        cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
    }

    /// 构造本轮 LLM 请求（`run` / `run_stream` 共用，消除两条路径的重复）。
    ///
    /// 统一处理：T6 动态 max_tokens → 思考强度 → 流式开关。
    /// 动态 max_tokens：优先上次请求实测输入（usage.prompt_tokens，由
    /// `run_turns` 从会话消息恢复并逐轮更新——重启后不退回估算），
    /// 无实测时退化为 TokenEstimator 估算；预算不足 1024 时提前报错
    /// （不发送请求），交由压缩链路在后续轮次恢复。
    async fn prepare_request(
        &self,
        model: &str,
        msgs: &[Message],
        stream: bool,
        thinking_effort: Option<String>,
        last_input_usage: Option<(String, usize)>,
    ) -> Result<ChatCompletionRequest, TianyanError> {
        // ADR-030：工具集按策略——主 agent 全局注册表；子代理过滤集
        // （角色白名单 + submit_result）
        let tools = match &self.turn_policy.tools {
            Some(t) => t.clone(),
            None => self.tool_registry.definitions().await,
        };

        let max_tokens = self.chat_spec.and_then(|spec| {
            // 实测输入：run_turns 从会话消息恢复 + 逐轮更新（model 一致才复用）
            let measured = last_input_usage
                .as_ref()
                .filter(|(m, _)| m == model)
                .map(|(_, tokens)| *tokens);
            let input_est = measured.unwrap_or_else(|| {
                crate::common::token_estimator::TokenEstimator::new().estimate_messages(msgs)
            });
            let max = crate::model::spec::dynamic_max_tokens(&spec, measured, msgs);
            tracing::debug!(
                model = %model,
                max_tokens = ?max,
                input_est,
                "agent_loop: 动态 max_tokens"
            );
            max
        });

        let mut request = ChatCompletionRequest::new(model, msgs.to_vec()).with_tools(tools);
        if stream {
            request = request.with_stream(true);
        }
        if let Some(t) = thinking_effort {
            request = request.with_thinking_effort(t);
        }
        if let Some(n) = max_tokens {
            request = request.with_max_tokens(n);
        } else if self.chat_spec.is_some() {
            return Err(TianyanError::Custom(
                "agent_loop: 上下文预算不足（不足 1024 tokens），将由压缩链路在后续轮次恢复"
                    .to_string(),
            ));
        }
        Ok(request)
    }

    /// 运行迭代循环直到回答或追问（非流式）。
    ///
    /// `cancel` 为 `Some` 时，每轮开始前检查取消标志；被取消返回
    /// [`AgentLoopResult::Cancelled`]（而非错误），调用方可区分"用户取消"与"失败"。
    /// 流式事件（思考/工具卡片）由 [`Self::run_stream`] 承担，本路径不携带 sender。
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
        cancel: Option<&AtomicBool>,
        thinking_effort: Option<String>,
    ) -> Result<AgentLoopResult, TianyanError> {
        self.run_turns_with_notice_check(
            messages,
            None,
            session_id,
            initial_parent_id,
            model,
            cancel,
            |this, model, _sender, msgs, _cancel, last_input_usage| {
                // FnMut 闭包按值捕获 thinking_effort，每次调用 clone 供本轮使用
                let thinking_effort = thinking_effort.clone();
                Box::pin(async move {
                    let request = this
                        .prepare_request(model, &msgs, false, thinking_effort, last_input_usage)
                        .await?;

                    let response =
                        this.model_service
                            .chat_completion(request)
                            .await
                            .map_err(|e| {
                                TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e))
                            })?;

                    // T6：记录本次实测输入，供下一轮动态 max_tokens 校准（随返回值更新）。
                    let updated_input = Some((model.to_string(), response.usage.prompt_tokens));
                    let turn_usage = response.usage;
                    let finish_reason = response
                        .choices
                        .first()
                        .and_then(|c| c.finish_reason.clone());
                    let choice =
                        response
                            .choices
                            .into_iter()
                            .next()
                            .ok_or(TianyanError::Custom(
                                "agent_loop: LLM 返回空响应".to_string(),
                            ))?;

                    Ok((
                        choice.message,
                        Some(turn_usage),
                        finish_reason,
                        updated_input,
                        false,
                    ))
                })
            },
        )
        .await
    }

    /// 运行迭代循环（流式），将 LLM 文本增量实时推送到前端。
    ///
    /// 与 [`run`] 的区别：每个 LLM 调用使用 `chat_completion_stream`，
    /// 文本 delta 通过 `stream_sender` 逐 token 发送，实现打字机效果。
    /// tool call 检测和执行仍为非流式（在完整消息累积后处理）。
    /// `cancel` 为 `Some` 时，chunk 接收循环与轮次边界都会检查取消标志。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_stream(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: StreamEventSender,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
        cancel: Option<&AtomicBool>,
        thinking_effort: Option<String>,
    ) -> Result<AgentLoopResult, TianyanError> {
        self.run_turns_with_notice_check(
            messages,
            Some(&stream_sender),
            session_id,
            initial_parent_id,
            model,
            cancel,
            |this, model, sender, msgs, cancel, last_input_usage| {
                // FnMut 闭包按值捕获 thinking_effort，每次调用 clone 供本轮使用
                let thinking_effort = thinking_effort.clone();
                Box::pin(async move {
                    let sender = sender.ok_or_else(|| {
                        TianyanError::Custom("agent_loop: 流式路径缺少 stream_sender".to_string())
                    })?;
                    // 收流并累积 chunk，再组装为与 `run` 路径一致的响应元组
                    let accum = this
                        .collect_streamed_turn(
                            model,
                            sender,
                            &msgs,
                            cancel,
                            thinking_effort,
                            &last_input_usage,
                        )
                        .await?;
                    AgentLoop::finalize_streamed_turn(
                        accum,
                        &msgs,
                        model,
                        last_input_usage.as_ref(),
                    )
                })
            },
        )
        .await
    }

    /// 获取本轮流式响应并累积 chunk（`run_stream` 单轮）。
    ///
    /// 职责：构造流式请求、打开流、逐 chunk 累积正文/推理/工具调用增量，
    /// 并透传 usage / finish_reason / 实测输入。取消在 chunk 循环内即时响应
    /// （标记 `cancelled` 并保留已收累积返回，由 [`Self::finalize_streamed_turn`]
    /// 收尾、[`Self::run_turns`] 统一转换为 Cancelled）。
    ///
    /// 不变量：累积顺序与发送顺序一致（reasoning 先于 content）；
    /// 网关缺失 usage 时此处不兜底（由 [`Self::finalize_streamed_turn`] 估算）。
    async fn collect_streamed_turn(
        &self,
        model: &str,
        sender: &StreamEventSender,
        msgs: &[Message],
        cancel: Option<&AtomicBool>,
        thinking_effort: Option<String>,
        last_input_usage: &Option<(String, usize)>,
    ) -> Result<StreamAccum, TianyanError> {
        let request = self
            .prepare_request(model, msgs, true, thinking_effort, last_input_usage.clone())
            .await?;

        let mut rx = self
            .model_service
            .chat_completion_stream(request)
            .await
            .map_err(|e| TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e)))?;

        // Accumulators for streaming chunks
        let mut accum = StreamAccum {
            content: String::new(),
            reasoning: String::new(),
            tool_calls: std::collections::BTreeMap::new(),
            usage: None,
            stream_error: None,
            finish_reason: None,
            updated_input: None,
            cancelled: false,
        };

        while let Some(chunk_result) = rx.recv().await {
            // 客户端断开/服务关停：chunk 循环内及时中断（长响应时不必等流结束）。
            // 已收累积标记 cancelled 后保留返回——取消收尾（仅留已交付前缀、
            // 丢弃 tool_calls）由 finalize_streamed_turn 统一处理。
            if AgentLoop::is_cancelled(cancel) {
                accum.cancelled = true;
                break;
            }
            match chunk_result {
                Ok(chunk) => {
                    // OpenAI sends usage in the final chunk
                    if let Some(ref usage) = chunk.usage {
                        accum.usage = Some(usage.clone());
                        // T6：记录本次实测输入，供下一轮动态 max_tokens 校准。
                        accum.updated_input = Some((model.to_string(), usage.prompt_tokens));
                    }
                    for choice in &chunk.choices {
                        // finish_reason：最终 chunk 携带；None 保持之前值（if let Some 覆盖累计）
                        if let Some(ref fr) = choice.finish_reason {
                            accum.finish_reason = Some(fr.clone());
                        }
                        // 顺序：reasoning 先于 content——同一 chunk 同时携带
                        // reasoning_content 与 content（推理→正文切换边界）时，
                        // 思考增量必须先发，正文后发，否则前端出现
                        // "思考未结束正文已插入"的乱序。
                        if let Some(ref reasoning) = choice.delta.reasoning_content {
                            if !reasoning.is_empty() {
                                accum.reasoning.push_str(reasoning);
                                sender.send_thought(reasoning).await;
                            }
                        }
                        if let Some(ref content) = choice.delta.content {
                            accum.content.push_str(content);
                            sender.send_answer_delta(content).await;
                        }
                        if let Some(ref tc_deltas) = choice.delta.tool_calls {
                            for tc in tc_deltas {
                                let entry = accum.tool_calls.entry(tc.index).or_insert_with(|| {
                                    (String::new(), String::new(), String::new())
                                });
                                if let Some(ref id) = tc.id {
                                    entry.0.clone_from(id);
                                }
                                if let Some(ref func) = tc.function {
                                    if let Some(ref name) = func.name {
                                        entry.1.clone_from(name);
                                    }
                                    if let Some(ref args) = func.arguments {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let err_msg = format!("流式接收中断: {}", e);
                    tracing::warn!(error = %e, "{}", err_msg);
                    accum.stream_error = Some(err_msg);
                    break;
                }
            }
        }

        Ok(accum)
    }

    /// 将流式累积结果组装为模型响应元组（`run_stream` 单轮收尾）。
    ///
    /// 职责：取消收尾（仅保留“已交付前缀”、丢弃全部 tool_calls）、流式中断
    /// 容错（有部分输出则保留为截断、否则报错）、finish 标记（中断且未携带时
    /// 补 `interrupted`）、构建 assistant 消息、usage 兜底估算。
    /// 返回形式与 `run` 路径一致：`(assistant_msg, turn_usage, finish_reason, 实测输入, 取消收尾)`。
    ///
    /// 不变量：错误消息与 finish 标记逐字与拆分前一致；估算仅在网关未返回
    /// usage 时触发。
    fn finalize_streamed_turn(
        accum: StreamAccum,
        msgs: &[Message],
        model: &str,
        last_input_usage: Option<&(String, usize)>,
    ) -> Result<TurnResult, TianyanError> {
        let StreamAccum {
            content,
            reasoning,
            tool_calls: accumulated_tool_calls,
            usage,
            stream_error,
            mut finish_reason,
            updated_input,
            cancelled,
        } = accum;
        // 取消收尾（对齐 DSH“中断先于分发”）：只保留能安全收尾的前缀——
        // 非空白的正文/推理（即用户已看到的内容）；tool_calls 无论完整与否
        // 全部丢弃（保留一个未分发的调用就必须捏造一个结果，故整体丢弃——
        // “下一次请求包含用户看到的内容”）。完全无内容时按取消上抛（调用方
        // 统一转 Cancelled，不落库）。finish 标记独立值 interrupted。
        let (assistant_msg, finish_reason, cancelled_turn) = if cancelled {
            let keep_content = !content.trim().is_empty();
            let keep_reasoning = !reasoning.trim().is_empty();
            if !keep_content && !keep_reasoning {
                return Err(TianyanError::Custom("agent_loop: 任务已取消".to_string()));
            }
            let msg = Message {
                role: MessageRole::Assistant,
                content: if keep_content { content } else { String::new() },
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
                tool_duration_ms: None,
                tool_error: None,
                reasoning_content: if keep_reasoning {
                    Some(reasoning)
                } else {
                    None
                },
            };
            (msg, Some("interrupted".to_string()), true)
        } else {
            // 流式中断容错（对齐 DSH：中断不丢弃已收内容）：
            // - 已有内容累积（正文/推理/工具调用）→ 保留为截断输出
            //   （finish=length 语义，前端可提示截断），不再整体报错；
            // - 完全无内容（流一开始就断）→ 按失败上报。
            let stream_interrupted = stream_error.is_some();
            if let Some(err_msg) = &stream_error {
                let has_partial = !content.is_empty()
                    || !reasoning.is_empty()
                    || !accumulated_tool_calls.is_empty();
                if !has_partial {
                    return Err(TianyanError::Custom(format!(
                        "agent_loop: LLM 调用失败：{}",
                        err_msg
                    )));
                }
                tracing::warn!(
                    error = %err_msg,
                    content_len = content.len(),
                    "流式中断，保留已收部分输出（截断）"
                );
            }

            // 流式中断：已收内容保留为截断输出——finish 标记独立值
            // "interrupted"（区别于 token 上限的 length：用户配置的
            // max_tokens 远未触顶时显示"已达上限"是误导，前端据此
            // 显示"流式中断"提示）。
            if stream_interrupted && finish_reason.is_none() {
                finish_reason = Some("interrupted".to_string());
            }

            // Build assistant message from accumulated content + tool calls
            let tool_calls: Option<Vec<ToolCall>> = if accumulated_tool_calls.is_empty() {
                None
            } else {
                Some(
                    accumulated_tool_calls
                        .into_values()
                        .map(|(id, name, arguments)| ToolCall {
                            id,
                            call_type: ToolCallType::Function,
                            function: FunctionCall { name, arguments },
                        })
                        .collect(),
                )
            };

            let msg = Message {
                role: MessageRole::Assistant,
                content,
                content_parts: None,
                tool_calls,
                tool_call_id: None,
                tool_duration_ms: None,
                tool_error: None,
                reasoning_content: if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning)
                },
            };
            (msg, finish_reason, false)
        };

        // usage 兜底（对齐 DSH token-meter 启发式）：部分网关
        // （如 ollama 兼容层）流式响应不携带 usage 字段——真实值
        // 缺失时用 TokenEstimator 估算，保证上下文占用/缓存命中
        // 展示与压缩判定不因网关差异而失效。
        let turn_usage = usage.or_else(|| {
            let estimator = crate::common::token_estimator::TokenEstimator::new();
            let prompt = last_input_usage
                .filter(|(m, _)| m == model)
                .map(|(_, tokens)| *tokens)
                .unwrap_or_else(|| estimator.estimate_messages(msgs));
            let completion = estimator.estimate_text(&assistant_msg.content)
                + assistant_msg
                    .reasoning_content
                    .as_deref()
                    .map(|r| estimator.estimate_text(r))
                    .unwrap_or(0);
            Some(TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                cache_read: 0,
                cache_write: 0,
            })
        });

        Ok((
            assistant_msg,
            turn_usage,
            finish_reason,
            updated_input,
            cancelled_turn,
        ))
    }

    /// 运行多轮迭代循环的共享脚手架（`run` / `run_stream` 共用）。
    ///
    /// 统一处理：轮次状态初始化（`current_parent_id` / `total_tokens` /
    /// 实测输入恢复）、[`TurnContext`] 构建、[`AgentLoop::handle_llm_response`]
    /// 调用与早返回、最大轮数错误、取消检查（轮顶 + step 错误时）。
    /// `step` 负责每轮获取模型响应，并统一转换为
    /// `(assistant_msg, turn_usage, finish_reason, 更新后的实测输入)` 形式。
    ///
    /// 实测输入（T6 动态 max_tokens 校准）：从会话消息恢复**最后一个压缩点
    /// 之后**最后一条带 usage 的消息（重启后不退回估算——usage 已持久化在
    /// 消息里），循环内逐轮用最新实测更新局部变量（无跨轮共享字段，克隆安全）。
    // 私有脚手架：8 个参数均为单轮演进所需状态/依赖，收敛为结构体反而降低可读性
    #[allow(clippy::too_many_arguments)]
    async fn run_turns<F>(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<&StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
        cancel: Option<&AtomicBool>,
        mut step: F,
    ) -> Result<AgentLoopResult, TianyanError>
    where
        F: for<'a> FnMut(
            &'a AgentLoop,
            &'a str,
            Option<&'a StreamEventSender>,
            Vec<Message>,
            Option<&'a AtomicBool>,
            Option<(String, usize)>,
        ) -> TurnStep<'a>,
    {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();
        // 实测输入恢复（T6 动态 max_tokens 校准）：口径见 recover_last_input_usage。
        let mut last_input_usage = self.recover_last_input_usage(session_id, model).await;

        for turn in 0..self.config.max_turns {
            // 轮顶取消检查：工具执行结束后、进入下一轮 LLM 调用前响应取消
            if Self::is_cancelled(cancel) {
                return Ok(AgentLoopResult::Cancelled {
                    total_tokens,
                    last_turn_usage: None,
                    turns: turn,
                });
            }

            // G6 结构化 Trace：设置当前轮次（工具 span 归属本轮）
            // T1-23：轮边界增量投递——把活动轮期间落库的事件通知追加到本轮
            // 上下文尾部（前缀不变，仅尾部追加 → 缓存友好；恰好一次由水位保证）
            self.deliver_pending_notices(messages, session_id).await;
            let turn_start = std::time::Instant::now();
            if let Some(ref trace) = self.trace {
                trace.set_current_turn(session_id, turn as i64);
            }

            let step_result = match self
                .run_step_with_retry(
                    &mut step,
                    model,
                    stream_sender,
                    messages,
                    cancel,
                    last_input_usage.as_ref(),
                    session_id,
                    turn,
                )
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    // step 内部（如流式 chunk 循环）检测到取消会上抛"任务已取消"，
                    // 此处统一转换为 Cancelled 结果，避免被当作失败
                    if Self::is_cancelled(cancel) {
                        return Ok(AgentLoopResult::Cancelled {
                            total_tokens,
                            last_turn_usage: None,
                            turns: turn + 1,
                        });
                    }
                    return Err(e);
                }
            };

            let (assistant_msg, turn_usage, finish_reason, updated_input, cancelled_turn) =
                step_result;
            // 更新实测输入（供下一轮动态 max_tokens 校准；None = 本轮无实测，保持旧值）
            if let Some(ui) = updated_input {
                last_input_usage = Some(ui);
            }

            // LLM 用量日志：每轮一次（聊天/子代理/演化任务统一记录）。
            self.record_usage_log(session_id, model, &turn_usage).await;

            let mut ctx = TurnContext {
                session_id,
                current_parent_id: &mut current_parent_id,
                total_tokens: &mut total_tokens,
                messages: &mut *messages,
                stream_sender,
                turn,
                model,
            };
            // 取消收尾（对齐 DSH“中断先于分发”）：finalize 已保留“已交付前缀”
            // （正文/推理——用户已看到的内容）并丢弃全部 tool_calls——此处落库后
            // 终止轮次：不执行工具、不进入下一轮。完全无内容时 finalize 已上抛
            // 取消错误（由上方 Err 分支统一转 Cancelled，不落库）。
            if cancelled_turn {
                ctx.messages.push(assistant_msg.clone());
                self.persist_message(&mut ctx, &assistant_msg, turn_usage, finish_reason)
                    .await;
                return Ok(AgentLoopResult::Cancelled {
                    total_tokens: total_tokens.clone(),
                    last_turn_usage: None,
                    turns: turn + 1,
                });
            }

            if let Some(result) = self
                .handle_llm_response(&assistant_msg, turn_usage, finish_reason, &mut ctx)
                .await?
            {
                // G6：终轮 span（含 token 消耗）
                self.record_turn_span(session_id, model, turn_start, &total_tokens);
                return Ok(result);
            }

            // G6：非终轮 span（工具执行后继续下一轮）
            self.record_turn_span(session_id, model, turn_start, &total_tokens);
        }

        // ADR-030：max_turns 行为按策略——主 agent 报错；子代理尽力而为
        // （返回最后输出，不因轮数上限丢失已完成的工作）
        self.max_turns_result(messages, total_tokens)
    }

    /// 恢复上一轮实测输入（T6 动态 max_tokens 校准）。
    ///
    /// 从会话消息取**最后一个压缩点之后**最后一条带 usage 且 model 一致的消息
    /// （重启后不退回估算——usage 已持久化在消息里；model 不一致 = 切换模型，
    /// 旧实测值不适用，退回估算）。
    ///
    /// 取数口径与压缩判定共用 `prompt_side_tokens` 单点——此前此处为
    /// `input + cache.read`，把缓存命中重复计一遍（≈真实×2；1M 窗口下
    /// 高缓存命中会话直接越过窗口 → "上下文预算不足"误报，会话被卡死）。
    /// 作用域与组装/压缩判定一致：只看**最后一个压缩点之后**的消息；
    /// 压缩点之前的实测对应旧上下文，不适用于压缩后的请求（压缩后首轮退回估算）。
    async fn recover_last_input_usage(
        &self,
        session_id: &str,
        model: &str,
    ) -> Option<(String, usize)> {
        match self.session_manager.get_session(session_id).await {
            Ok(Some(session)) => {
                let start_idx = session
                    .messages
                    .iter()
                    .rposition(|m| m.compression_marker)
                    .unwrap_or(0);
                session.messages[start_idx..]
                    .iter()
                    .rev()
                    .find(|m| {
                        !m.compression_marker
                            && (m.tokens.input > 0 || m.tokens.cache.read > 0)
                            && m.model_id.as_deref() == Some(model)
                    })
                    .map(|m| (model.to_string(), m.prompt_side_tokens()))
            }
            _ => None,
        }
    }

    /// 执行单轮 step 获取模型响应（含空响应重试一次）。
    ///
    /// 空响应（既无正文也无工具调用）会让工具循环中途静默终止；消息历史此时
    /// 未变（空响应尚未入史），重试一次通常能恢复；重试仍空则按正常结束处理
    /// （空输出 = completed，对齐 DSH）。唤醒轮与用户轮同构（统一循环框架）。
    ///
    /// 不变量：每次调用 `step` 传入 `messages` 的克隆与最新实测输入；
    /// 取消（step 内上抛"任务已取消"）以 `Err` 上抛，由调用方统一转换为
    /// Cancelled（不在此处改变可观测行为）。
    // 私有脚手架：参数均为单轮 step 所需状态/依赖，收敛为结构体反而降低可读性
    #[allow(clippy::too_many_arguments)]
    async fn run_step_with_retry<F>(
        &self,
        step: &mut F,
        model: &str,
        stream_sender: Option<&StreamEventSender>,
        messages: &[Message],
        cancel: Option<&AtomicBool>,
        last_input_usage: Option<&(String, usize)>,
        session_id: &str,
        turn: usize,
    ) -> Result<TurnResult, TianyanError>
    where
        F: for<'a> FnMut(
            &'a AgentLoop,
            &'a str,
            Option<&'a StreamEventSender>,
            Vec<Message>,
            Option<&'a AtomicBool>,
            Option<(String, usize)>,
        ) -> TurnStep<'a>,
    {
        let mut step_result = step(
            self,
            model,
            stream_sender,
            messages.to_vec(),
            cancel,
            last_input_usage.cloned(),
        )
        .await?;

        // 空响应重试（用户轮）：LLM 偶发返回既无正文也无工具调用的空响应
        // （思考模型"想完没说话"），直接结束会让任务在工具循环中途静默终止。
        // 消息历史此时未变（空响应尚未入史），重试一次通常能恢复；
        // 重试仍空则按正常结束处理（空输出 = completed，对齐 DSH）。
        // 唤醒轮与用户轮同构（统一循环框架）：空响应同样重试一次——
        // 后台命令完成通知是明确的输入，模型"想完没说话"或思考碎片
        // （如只剩 "@if"）时给第二次机会（失败场景指令要求必须汇报，
        // 重试通常能恢复完整输出），重试仍空才按正常结束处理。
        // 取消收尾不重试：取消时“无正文但有推理”会被误判为空响应——重试会
        // 让已收推理丢失落库路径（且第二次调用在取消标志下会立即再取消）。
        let empty = matches!(
            &step_result,
            (m, _, _, _, false) if m.content.is_empty() && m.tool_calls.is_none()
        );
        if empty {
            tracing::warn!(
                session = %session_id,
                turn,
                "LLM 返回空响应，重试一次"
            );
            step_result = step(
                self,
                model,
                stream_sender,
                messages.to_vec(),
                cancel,
                last_input_usage.cloned(),
            )
            .await?;
        }

        Ok(step_result)
    }

    /// 记录单轮 LLM 用量日志（聊天/子代理/演化任务统一记录）。
    ///
    /// provider 优先查配置映射（模型名可能不含前缀），未命中时按 model 名前缀
    /// 推导（provider/model 命名）。无 usage_log 或无用量时静默跳过。
    async fn record_usage_log(
        &self,
        session_id: &str,
        model: &str,
        turn_usage: &Option<TokenUsage>,
    ) {
        if let Some(ref usage_log) = self.usage_log {
            if let Some(usage) = turn_usage {
                let provider = self
                    .provider_by_model
                    .get(model)
                    .cloned()
                    .unwrap_or_else(|| model.split("/").next().unwrap_or("unknown").to_string());
                usage_log.record(session_id, &provider, model, usage).await;
            }
        }
    }

    /// 记录本轮的 G6 结构化 Trace span（含 token 消耗）。
    ///
    /// 终轮与非终轮共用同一记录口径（success = true）；未配置 trace 时静默跳过。
    fn record_turn_span(
        &self,
        session_id: &str,
        model: &str,
        turn_start: std::time::Instant,
        total_tokens: &TokenUsage,
    ) {
        if let Some(ref trace) = self.trace {
            trace.record_turn(
                session_id,
                model,
                turn_start.elapsed().as_millis() as i64,
                (total_tokens.prompt_tokens + total_tokens.completion_tokens) as i64,
                true,
            );
        }
    }

    /// 达到 max_turns 上限时的结果（ADR-030）：策略为尽力而为则返回最后输出
    /// （不因轮数上限丢失已完成的工作），否则报错。
    fn max_turns_result(
        &self,
        messages: &[Message],
        total_tokens: TokenUsage,
    ) -> Result<AgentLoopResult, TianyanError> {
        if self.turn_policy.max_turns_graceful {
            let content = messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Assistant && !m.content.is_empty())
                .map(|m| m.content.clone())
                .unwrap_or_default();
            return Ok(AgentLoopResult::MaxTurnsReached {
                content,
                total_tokens,
                last_turn_usage: None,
                turns: self.config.max_turns,
            });
        }

        Err(TianyanError::Custom(format!(
            "agent_loop: 达到最大轮数限制：{}",
            self.config.max_turns
        )))
    }

    /// 构造工具调用展示事件（A2 展示契约；ask_user 与普通工具轮共用）。
    fn tool_call_event(&self, tc: &ToolCall) -> ToolCallEvent {
        ToolCallEvent {
            id: tc.id.clone(),
            name: tc.function.name.clone(),
            arguments: tc.function.arguments.clone(),
            presentation: self
                .tool_registry
                .presentation(&tc.function.name)
                .as_str()
                .to_string(),
        }
    }

    /// Process a single turn after the LLM response has been obtained.
    ///
    /// Handles: token accumulation, ask_user check, message persistence (in-memory + VFS),
    /// tool execution, and result assembly. Called by both [`run`] and [`run_stream`].
    ///
    /// Returns `Ok(Some(result))` if this turn produces a final answer or clarification,
    /// `Ok(None)` if tool calls were executed and the loop should continue.
    async fn handle_llm_response(
        &self,
        assistant_msg: &Message,
        turn_usage: Option<TokenUsage>,
        finish_reason: Option<String>,
        ctx: &mut TurnContext<'_>,
    ) -> Result<Option<AgentLoopResult>, TianyanError> {
        // Accumulate turn token usage
        if let Some(ref usage) = turn_usage {
            ctx.total_tokens.accumulate(usage);
        }

        // 归一化：空 tool_calls 数组视为无工具调用（部分 provider 在无工具时
        // 返回 [] 而非 null——否则会走进"执行 0 个工具 → 继续循环"的死路）
        let tool_calls = assistant_msg.tool_calls.as_ref().filter(|c| !c.is_empty());

        // Add assistant message to in-memory history
        ctx.messages.push(assistant_msg.clone());

        // Persist assistant message via SessionManager
        // Clone before moving into persist_message — needed for the
        // Answer result in the no-tool-calls branch below.
        let persisted = self
            .persist_message(ctx, assistant_msg, turn_usage.clone(), finish_reason)
            .await;

        if let Some(tool_calls) = tool_calls {
            // 工具轮用量事件：每轮 LLM 调用都是完整上下文重发（O(n²) 量级），
            // 必须逐轮下发——否则前端本地消息只有最终轮带 usage，会话消耗
            // 汇总（sumSessionUsage）漏计所有中间轮输入。
            if let (Some(sender), Some(usage)) = (ctx.stream_sender, &turn_usage) {
                sender.send_turn_usage(usage).await;
            }

            // Notify about tool calls if sender is available
            if let Some(sender) = ctx.stream_sender {
                for tc in tool_calls {
                    // A2 展示契约：携带结构化工具信息（ID/名称/参数/展示意图），
                    // 前端据此渲染 tool card 并按 ID 关联执行结果；delta 保持人类可读文本。
                    let event = self.tool_call_event(tc);
                    sender
                        .send_tool_call(
                            &format!("\u{8c03}\u{7528}: {}", tc.function.name),
                            Some(event),
                        )
                        .await;
                }
            }

            // 执行工具（ADR-030：按策略选择执行器——主 agent 全局注册表
            // （审批可交互）；子代理角色过滤（审批不交互，ADR-011））
            let results = match &self.turn_policy.tool_executor {
                ToolExecutorKind::Global => {
                    self.tool_registry
                        .execute_parallel(tool_calls, ctx.session_id, false)
                        .await
                }
                ToolExecutorKind::RoleFiltered {
                    role_tools,
                    role_name,
                } => {
                    self.tool_registry
                        .execute_tool_calls_with_role(
                            tool_calls,
                            ctx.session_id,
                            role_tools.as_deref(),
                            role_name.as_deref(),
                        )
                        .await
                }
            };

            for (call_id, outcome) in results {
                let content = match &outcome.result {
                    Ok(ref value) => serde_json::to_string(value).unwrap_or_else(|e| {
                        format!(r#"{{"error": "serialization failed: {}"}}"#, e)
                    }),
                    Err(ref e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                };

                // Persist tool result（携带耗时/成败元数据：Part::ToolResult.time
                // 与 error 由 from_message 从消息级字段接线，历史回放可见）
                let tool_msg = Message::tool_result(
                    &call_id,
                    &content,
                    Some(outcome.duration_ms),
                    outcome.error(),
                );
                self.persist_message(ctx, &tool_msg, None, None).await;

                ctx.messages.push(tool_msg);

                // 流式下发工具结果事件（耗时/成败结构化，前端卡片实时显示）
                if let Some(sender) = ctx.stream_sender {
                    sender
                        .send_tool_result(
                            &content,
                            &call_id,
                            outcome.duration_ms,
                            outcome.is_ok(),
                            outcome.error(),
                        )
                        .await;
                }
            }

            Ok(None)
        } else if assistant_msg.content.is_empty() {
            // 空输出 = 正常结束（对齐 DSH：无工具调用即 completed，内容空不空
            // 不改变结束判定）。空响应重试（run_turns）已给模型一次恢复机会；
            // 重试仍空说明模型确实无需回复——结束而非报错。
            // 空消息已在上方 persist_message 持久化（组装层跳过空文本，
            // 不进 LLM 上下文；前端渲染层跳过，不产生可见气泡）。
            Ok(Some(AgentLoopResult::Answer {
                content: String::new(),
                total_tokens: ctx.total_tokens.clone(),
                last_turn_usage: turn_usage.clone(),
                turns: ctx.turn + 1,
                persisted_message: Box::new(persisted),
            }))
        } else {
            // Already persisted above — just return.
            Ok(Some(AgentLoopResult::Answer {
                content: assistant_msg.content.clone(),
                total_tokens: ctx.total_tokens.clone(),
                last_turn_usage: turn_usage.clone(),
                turns: ctx.turn + 1,
                persisted_message: Box::new(persisted),
            }))
        }
    }

    /// 将消息转换为 StructuredMessage 并持久化到会话，返回持久化后的消息。
    ///
    /// 同时推进 `current_parent_id` 指向新消息，维持消息链。
    /// 持久化失败仅告警不中断（与历史行为一致）。
    async fn persist_message(
        &self,
        ctx: &mut TurnContext<'_>,
        message: &Message,
        usage: Option<TokenUsage>,
        finish_reason: Option<String>,
    ) -> StructuredMessage {
        let mut structured = ContextAssembler::message_to_structured(
            message,
            ctx.session_id,
            ctx.current_parent_id.as_deref(),
            usage,
        );
        // T4：持久化消息携带模型 finish_reason（此前恒为 None）
        structured.finish = finish_reason;
        // 持久化模型标识：实测输入按模型恢复（不同模型分词/计费口径不同，
        // 切换后旧实测值不得复用）
        structured.model_id = Some(ctx.model.to_string());
        *ctx.current_parent_id = Some(structured.id.clone());
        let persisted = structured.clone();
        // ADR-030：按策略选择持久化路径——主 agent 索引 FTS；子代理不索引
        // （ADR-026：子会话不参与回忆检索）
        let result = if self.turn_policy.persist_no_fts {
            self.session_manager
                .add_structured_message_no_fts(ctx.session_id, structured)
                .await
        } else {
            self.session_manager
                .add_structured_message(ctx.session_id, structured)
                .await
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "持久化消息失败");
        }
        persisted
    }
}

#[cfg(test)]
#[path = "loop_tests.rs"]
mod tests;
