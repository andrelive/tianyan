use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::error::TianyanError;
use crate::common::types::{FunctionCall, Message, MessageRole, StructuredMessage, TokenUsage};
use crate::common::types::{ToolCall, ToolCallType};
use crate::context::ContextAssembler;
use crate::model::spec::ModelSpec;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use crate::observability::trace::TraceCollector;
use crate::session::SessionManager;

/// Agent 循环配置。
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// 最大轮数。
    pub max_turns: usize,
    /// 允许模型输出空文本作为合法结束（ADR-013 唤醒轮语义）。
    ///
    /// 唤醒轮（后台任务全部完成/失败触发）中模型可能认为无需回复——
    /// 空输出是合法结束而非错误；普通用户轮保持 false（空输出视为错误）。
    pub allow_empty_answer: bool,
    /// 工具短路选择（G1）：LLM 可见工具数超过阈值时按相关性过滤 schema。
    pub shortlist_tools: bool,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 200,
            allow_empty_answer: false,
            shortlist_tools: true,
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
        /// 循环轮数。
        turns: usize,
        /// AgentLoop 已持久化的 StructuredMessage，coordinator 直接复用此消息加入状态，避免重复创建。
        persisted_message: Box<StructuredMessage>,
    },
    /// 需要追问。
    NeedsClarification {
        /// 追问问题。
        question: String,
        /// Token 用量。
        total_tokens: TokenUsage,
        /// 循环轮数。
        turns: usize,
    },
    /// 循环被取消（客户端断开或服务关停）。
    Cancelled {
        /// Token 用量。
        total_tokens: TokenUsage,
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
}

/// 单轮响应获取步骤返回的 future（`run` / `run_stream` 传入 [`AgentLoop::run_turns`]）。
/// 第三元素为模型响应的 finish_reason（非流式来自 choice，流式来自最终 chunk）。
type TurnStep<'a> = Pin<
    Box<
        dyn Future<Output = Result<(Message, Option<TokenUsage>, Option<String>), TianyanError>>
            + Send
            + 'a,
    >,
>;

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
    /// 上次请求实测输入 token 数（T6 动态 max_tokens 校准）。
    ///
    /// `Arc` 保证 AgentLoop 克隆后校准值共享；0 表示尚无实测（退化为估算）。
    pub(crate) last_input_usage: Arc<AtomicUsize>,
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
            last_input_usage: Arc::new(AtomicUsize::new(0)),
        }
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

    /// 获取工具注册表引用。
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// 允许模型空输出合法结束（ADR-013 唤醒轮语义）。
    ///
    /// 返回克隆实例（AgentLoop 为值类型，字段仅 config 变化）。
    pub fn with_allow_empty_answer(mut self) -> Self {
        self.config.allow_empty_answer = true;
        self
    }

    /// 取消标志检查（None 视为未取消）。
    fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
        cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
    }

    /// 提取工具短路选择的查询信号（G1）：最后一条用户消息的文本。
    ///
    /// 过滤以"当前任务意图"为相关性锚点；取不到用户消息时返回 None
    /// （短路层保守回退为全量工具）。
    fn shortlist_query(msgs: &[Message]) -> Option<String> {
        msgs.iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
            .map(|m| m.content.clone())
    }

    /// 获取本轮的 LLM 可见工具定义（G1 工具短路选择）。
    ///
    /// 配置开启且工具总数超阈值时按 query 相关性过滤 schema；
    /// 否则全量（行为零变化）。执行层不受影响（被过滤工具仍可执行）。
    async fn tools_for_turn(&self, msgs: &[Message]) -> Vec<crate::model::types::ToolDefinition> {
        if self.config.shortlist_tools {
            self.tool_registry
                .definitions_shortlisted(Self::shortlist_query(msgs).as_deref())
                .await
        } else {
            self.tool_registry.definitions().await
        }
    }

    /// 运行迭代循环直到回答或追问（非流式）。
    ///
    /// `cancel` 为 `Some` 时，每轮开始前检查取消标志；被取消返回
    /// [`AgentLoopResult::Cancelled`]（而非错误），调用方可区分"用户取消"与"失败"。
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
        cancel: Option<&AtomicBool>,
        thinking_effort: Option<String>,
    ) -> Result<AgentLoopResult, TianyanError> {
        self.run_turns(
            messages,
            stream_sender.as_ref(),
            session_id,
            initial_parent_id,
            model,
            cancel,
            |this, model, _sender, msgs, _cancel| {
                // FnMut 闭包按值捕获 thinking_effort，每次调用 clone 供本轮使用
                let thinking_effort = thinking_effort.clone();
                Box::pin(async move {
                    let tools = this.tools_for_turn(&msgs).await;

                    // T6 动态 max_tokens：优先上次请求实测输入（usage.prompt_tokens），
                    // 无实测时退化为 TokenEstimator 估算；预算不足 1024 时提前报错
                    // （不发送请求），交由压缩链路在后续轮次恢复。
                    let max_tokens = this.chat_spec.and_then(|spec| {
                        let measured = (this.last_input_usage.load(Ordering::Relaxed) > 0)
                            .then(|| this.last_input_usage.load(Ordering::Relaxed));
                        let input_est = measured.unwrap_or_else(|| {
                            crate::common::token_estimator::TokenEstimator::new()
                                .estimate_messages(&msgs)
                        });
                        let max = crate::model::spec::dynamic_max_tokens(&spec, measured, &msgs);
                        tracing::debug!(
                            model = %model,
                            max_tokens = ?max,
                            input_est,
                            "agent_loop: 动态 max_tokens"
                        );
                        max
                    });

                    let request = ChatCompletionRequest::new(model, msgs).with_tools(tools);
                    let request = if let Some(t) = thinking_effort {
                        request.with_thinking_effort(t)
                    } else {
                        request
                    };
                    let request = if let Some(n) = max_tokens {
                        request.with_max_tokens(n)
                    } else if this.chat_spec.is_some() {
                        return Err(TianyanError::Custom(
                            "agent_loop: 上下文预算不足（不足 1024 tokens），将由压缩链路在后续轮次恢复"
                                .to_string(),
                        ));
                    } else {
                        request
                    };

                    let response =
                        this.model_service
                            .chat_completion(request)
                            .await
                            .map_err(|e| {
                                TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e))
                            })?;

                    // T6：记录本次实测输入，供下一轮动态 max_tokens 校准（跨轮共享）。
                    this.last_input_usage
                        .store(response.usage.prompt_tokens, Ordering::Relaxed);
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

                    Ok((choice.message, Some(turn_usage), finish_reason))
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
        self.run_turns(
            messages,
            Some(&stream_sender),
            session_id,
            initial_parent_id,
            model,
            cancel,
            |this, model, sender, msgs, cancel| {
                // FnMut 闭包按值捕获 thinking_effort，每次调用 clone 供本轮使用
                let thinking_effort = thinking_effort.clone();
                Box::pin(async move {
                    let sender = sender.ok_or_else(|| {
                        TianyanError::Custom("agent_loop: 流式路径缺少 stream_sender".to_string())
                    })?;

                    let tools = this.tools_for_turn(&msgs).await;

                    // T6 动态 max_tokens：与 run() 同语义（见非流式注释）。
                    let max_tokens = this.chat_spec.and_then(|spec| {
                        let measured = (this.last_input_usage.load(Ordering::Relaxed) > 0)
                            .then(|| this.last_input_usage.load(Ordering::Relaxed));
                        let input_est = measured.unwrap_or_else(|| {
                            crate::common::token_estimator::TokenEstimator::new()
                                .estimate_messages(&msgs)
                        });
                        let max = crate::model::spec::dynamic_max_tokens(&spec, measured, &msgs);
                        tracing::debug!(
                            model = %model,
                            max_tokens = ?max,
                            input_est,
                            "agent_loop: 动态 max_tokens"
                        );
                        max
                    });

                    let request = ChatCompletionRequest::new(model, msgs)
                        .with_stream(true)
                        .with_tools(tools);
                    let request = if let Some(t) = thinking_effort {
                        request.with_thinking_effort(t)
                    } else {
                        request
                    };
                    let request = if let Some(n) = max_tokens {
                        request.with_max_tokens(n)
                    } else if this.chat_spec.is_some() {
                        return Err(TianyanError::Custom(
                            "agent_loop: 上下文预算不足（不足 1024 tokens），将由压缩链路在后续轮次恢复"
                                .to_string(),
                        ));
                    } else {
                        request
                    };

                    let mut rx = this
                        .model_service
                        .chat_completion_stream(request)
                        .await
                        .map_err(|e| {
                            TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e))
                        })?;

                    // Accumulators for streaming chunks
                    let mut accumulated_content = String::new();
                    let mut accumulated_reasoning = String::new();
                    let mut accumulated_tool_calls: std::collections::BTreeMap<
                        usize,
                        (String, String, String),
                    > = std::collections::BTreeMap::new();
                    let mut turn_usage: Option<TokenUsage> = None;
                    let mut stream_error: Option<String> = None;
                    let mut finish_reason: Option<String> = None;

                    while let Some(chunk_result) = rx.recv().await {
                        // 客户端断开/服务关停：chunk 循环内及时中断（长响应时不必等流结束）
                        if AgentLoop::is_cancelled(cancel) {
                            return Err(TianyanError::Custom("agent_loop: 任务已取消".to_string()));
                        }
                        match chunk_result {
                            Ok(chunk) => {
                                // OpenAI sends usage in the final chunk
                                if let Some(ref usage) = chunk.usage {
                                    turn_usage = Some(usage.clone());
                                    // T6：记录本次实测输入，供下一轮动态 max_tokens 校准。
                                    this.last_input_usage.store(
                                        usage.prompt_tokens,
                                        Ordering::Relaxed,
                                    );
                                }
                                for choice in &chunk.choices {
                                    // finish_reason：最终 chunk 携带；None 保持之前值（if let Some 覆盖累计）
                                    if let Some(ref fr) = choice.finish_reason {
                                        finish_reason = Some(fr.clone());
                                    }
                                    if let Some(ref content) = choice.delta.content {
                                        accumulated_content.push_str(content);
                                        sender.send_answer_delta(content).await;
                                    }
                                    if let Some(ref reasoning) = choice.delta.reasoning_content {
                                        if !reasoning.is_empty() {
                                            accumulated_reasoning.push_str(reasoning);
                                            sender.send_thought(reasoning).await;
                                        }
                                    }
                                    if let Some(ref tc_deltas) = choice.delta.tool_calls {
                                        for tc in tc_deltas {
                                            let entry = accumulated_tool_calls
                                                .entry(tc.index)
                                                .or_insert_with(|| {
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
                                stream_error = Some(err_msg);
                                break;
                            }
                        }
                    }

                    // If streaming failed mid-response, report error rather than using partial data
                    if let Some(err_msg) = stream_error {
                        return Err(TianyanError::Custom(format!(
                            "agent_loop: LLM 调用失败：{}",
                            err_msg
                        )));
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

                    let assistant_msg = Message {
                        role: MessageRole::Assistant,
                        content: accumulated_content,
                        content_parts: None,
                        tool_calls,
                        tool_call_id: None,
                        reasoning_content: if accumulated_reasoning.is_empty() {
                            None
                        } else {
                            Some(accumulated_reasoning)
                        },
                    };

                    Ok((assistant_msg, turn_usage, finish_reason))
                })
            },
        )
        .await
    }

    /// 运行多轮迭代循环的共享脚手架（`run` / `run_stream` 共用）。
    ///
    /// 统一处理：轮次状态初始化（`current_parent_id` / `total_tokens`）、
    /// [`TurnContext`] 构建、[`AgentLoop::handle_llm_response`] 调用与早返回、
    /// 最大轮数错误、取消检查（轮顶 + step 错误时）。
    /// `step` 负责每轮获取模型响应，并统一转换为
    /// `(assistant_msg, turn_usage)` 形式。
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
        ) -> TurnStep<'a>,
    {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();

        for turn in 0..self.config.max_turns {
            // 轮顶取消检查：工具执行结束后、进入下一轮 LLM 调用前响应取消
            if Self::is_cancelled(cancel) {
                return Ok(AgentLoopResult::Cancelled {
                    total_tokens,
                    turns: turn,
                });
            }

            // G6 结构化 Trace：设置当前轮次（工具 span 归属本轮）
            let turn_start = std::time::Instant::now();
            if let Some(ref trace) = self.trace {
                trace.set_current_turn(session_id, turn as i64);
            }

            let mut step_result =
                match step(self, model, stream_sender, messages.clone(), cancel).await {
                    Ok(v) => v,
                    Err(e) => {
                        // step 内部（如流式 chunk 循环）检测到取消会上抛"任务已取消"，
                        // 此处统一转换为 Cancelled 结果，避免被当作失败
                        if Self::is_cancelled(cancel) {
                            return Ok(AgentLoopResult::Cancelled {
                                total_tokens,
                                turns: turn + 1,
                            });
                        }
                        return Err(e);
                    }
                };

            // 空响应重试（用户轮）：LLM 偶发返回既无正文也无工具调用的空响应
            // （思考模型"想完没说话"），直接报错会让任务在工具循环中途静默终止。
            // 消息历史此时未变（空响应尚未入史），重试一次通常能恢复；
            // 重试仍空则按原有空响应错误路径处理。唤醒轮（allow_empty_answer）不重试。
            if !self.config.allow_empty_answer {
                let empty = matches!(
                    &step_result,
                    (m, _, _) if m.content.is_empty() && m.tool_calls.is_none()
                );
                if empty {
                    tracing::warn!(
                        session = %session_id,
                        turn,
                        "LLM 返回空响应，重试一次"
                    );
                    step_result =
                        match step(self, model, stream_sender, messages.clone(), cancel).await {
                            Ok(v) => v,
                            Err(e) => {
                                if Self::is_cancelled(cancel) {
                                    return Ok(AgentLoopResult::Cancelled {
                                        total_tokens,
                                        turns: turn + 1,
                                    });
                                }
                                return Err(e);
                            }
                        };
                }
            }

            let (assistant_msg, turn_usage, finish_reason) = step_result;

            let mut ctx = TurnContext {
                session_id,
                current_parent_id: &mut current_parent_id,
                total_tokens: &mut total_tokens,
                messages: &mut *messages,
                stream_sender,
                turn,
            };

            if let Some(result) = self
                .handle_llm_response(&assistant_msg, turn_usage, finish_reason, &mut ctx)
                .await?
            {
                // G6：终轮 span（含 token 消耗）
                if let Some(ref trace) = self.trace {
                    trace.record_turn(
                        session_id,
                        model,
                        turn_start.elapsed().as_millis() as i64,
                        (total_tokens.prompt_tokens + total_tokens.completion_tokens) as i64,
                        true,
                    );
                }
                return Ok(result);
            }

            // G6：非终轮 span（工具执行后继续下一轮）
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

        Err(TianyanError::Custom(format!(
            "agent_loop: 达到最大轮数限制：{}",
            self.config.max_turns
        )))
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

        // Check for ask_user before adding to history
        if let Some(ref tool_calls) = assistant_msg.tool_calls {
            if let Some(ask_call) = tool_calls.iter().find(|tc| tc.function.name == "ask_user") {
                let params: AskUserParams = serde_json::from_str(&ask_call.function.arguments)
                    .map_err(|e| {
                        TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e))
                    })?;
                return Ok(Some(AgentLoopResult::NeedsClarification {
                    question: params.question,
                    total_tokens: ctx.total_tokens.clone(),
                    turns: ctx.turn + 1,
                }));
            }
        }

        // Add assistant message to in-memory history
        ctx.messages.push(assistant_msg.clone());

        // Persist assistant message via SessionManager
        // Clone before moving into persist_message — needed for the
        // Answer result in the no-tool-calls branch below.
        let persisted = self
            .persist_message(ctx, assistant_msg, turn_usage, finish_reason)
            .await;

        if let Some(ref tool_calls) = assistant_msg.tool_calls {
            // Notify about tool calls if sender is available
            if let Some(sender) = ctx.stream_sender {
                for tc in tool_calls {
                    // A2 展示契约：携带结构化工具信息（名称/参数/展示意图），
                    // 前端据此渲染 tool card；delta 保持人类可读文本。
                    let event = crate::agent::types::ToolCallEvent {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                        presentation: self
                            .tool_registry
                            .presentation(&tc.function.name)
                            .as_str()
                            .to_string(),
                    };
                    sender
                        .send_tool_call(
                            &format!("\u{8c03}\u{7528}: {}", tc.function.name),
                            Some(event),
                        )
                        .await;
                }
            }

            // 执行工具（session_id 随调用链传递：后台委托归属父会话，完成
            // 通知注入该会话；subagent=false 主循环——审批可交互）
            let results = self
                .tool_registry
                .execute_parallel(tool_calls, ctx.session_id, false)
                .await;

            // 审批降级：任一工具因审批门控被拒（已入队待确认指纹）时，
            // 不再继续循环，直接转为追问用户（用户批准后重试工具调用）。
            // 通过 has_pending_approval() 类型化信号判断，而非解析错误字符串。
            let denied_action = if self.tool_registry.has_pending_approval().await {
                results.iter().find_map(|(_call_id, result)| match result {
                    Err(e) => Some(e.to_string()),
                    _ => None,
                })
            } else {
                None
            };
            if let Some(err_msg) = denied_action {
                return Ok(Some(AgentLoopResult::NeedsClarification {
                    question: format!(
                        "系统安全策略要求确认后才能执行该操作。\n\n操作详情：{}\n\n请回复「允许」继续执行，或回复「拒绝」终止。",
                        err_msg
                    ),
                    total_tokens: ctx.total_tokens.clone(),
                    turns: ctx.turn + 1,
                }));
            }

            for (call_id, result) in results {
                let content = match result {
                    Ok(ref value) => serde_json::to_string(value).unwrap_or_else(|e| {
                        format!(r#"{{"error": "serialization failed: {}"}}"#, e)
                    }),
                    Err(ref e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                };

                // Persist tool result
                let tool_msg = Message::tool(&call_id, &content);
                self.persist_message(ctx, &tool_msg, None, None).await;

                ctx.messages.push(tool_msg);

                if let Some(sender) = ctx.stream_sender {
                    sender.send_observation(&content).await;
                }
            }

            Ok(None)
        } else if assistant_msg.content.is_empty() {
            if self.config.allow_empty_answer {
                // ADR-013 唤醒轮：模型空输出 = 合法结束（无需回复）。
                // 空消息已在上方 persist_message 持久化（无害，下一轮组装跳过空文本）。
                Ok(Some(AgentLoopResult::Answer {
                    content: String::new(),
                    total_tokens: ctx.total_tokens.clone(),
                    turns: ctx.turn + 1,
                    persisted_message: Box::new(persisted),
                }))
            } else {
                // LLM returned neither content nor tool calls — treat as error
                // rather than silently continuing the loop (which would consume
                // up to max_turns with no progress).
                Err(TianyanError::Custom(
                    "agent_loop: LLM 返回空响应".to_string(),
                ))
            }
        } else {
            // Already persisted above — just return.
            Ok(Some(AgentLoopResult::Answer {
                content: assistant_msg.content.clone(),
                total_tokens: ctx.total_tokens.clone(),
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
        *ctx.current_parent_id = Some(structured.id.clone());
        let persisted = structured.clone();
        if let Err(e) = self
            .session_manager
            .add_structured_message(ctx.session_id, structured)
            .await
        {
            tracing::warn!(error = %e, "持久化消息失败");
        }
        persisted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::error::Result;
    use crate::common::types::StructuredMessage;
    use crate::session::Session;
    use async_trait::async_trait;

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

    #[test]
    fn test_agent_loop_config_default() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_turns, 200);
        // Touch the mock to keep it "constructed" (dead-code lint).
        let _mgr = MockSessionManager;
    }

    #[test]
    fn test_with_chat_spec_chain() {
        // 链式注入：with_chat_spec(Some(spec)) 设置 chat_spec，
        // with_chat_spec(None) 清除；默认 new() 为 None。
        let spec = ModelSpec {
            context_length: 100_000,
            max_output_tokens: 8_192,
            max_input_tokens: 91_808,
        };
        let base = make_loop(MockChatService::new(), 5);
        assert_eq!(
            base.chat_spec, None,
            "新建 AgentLoop 的 chat_spec 应为 None"
        );

        let with_spec = base.with_chat_spec(Some(spec));
        assert_eq!(
            with_spec.chat_spec,
            Some(spec),
            "注入 spec 后 chat_spec 应为 Some(spec)"
        );

        let cleared = with_spec.with_chat_spec(None);
        assert_eq!(cleared.chat_spec, None, "with_chat_spec(None) 应清除 spec");
    }

    #[test]
    fn test_agent_loop_error_display() {
        let err = TianyanError::Custom(format!("agent_loop: 达到最大轮数限制：{}", 5));
        assert!(err.to_string().contains("5"));
    }

    // ── 完整循环测试（MockChatService + 真实 ToolRegistry） ────

    use crate::common::types::TokenUsage as CommonTokenUsage;
    use crate::common::types::{FunctionCall, ToolCall, ToolCallType};
    use crate::executor::SecurityPolicy;
    use crate::model::types::{
        ChatChoice, ChatCompletionChunk, ChatCompletionResponse, ChunkChoice, DeltaContent,
        ToolCallDelta, ToolCallFunctionDelta,
    };
    use crate::model::MockChatService;

    // 流式回归测试：AgentStreamChunk / StreamChunkType
    use crate::agent::types::{AgentStreamChunk, StreamChunkType};

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
            usage: CommonTokenUsage::default(),
        }
    }

    fn tool_call_msg(name: &str) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: format!("call_{}", name),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        )
    }

    fn make_loop(mock: MockChatService, max_turns: usize) -> AgentLoop {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        AgentLoop::new(
            Arc::new(mock),
            registry,
            Arc::new(MockSessionManager),
            AgentLoopConfig {
                max_turns,
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn test_run_completes_tool_loop() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|req| {
            // 第一轮：仅 user 消息 → 返回工具调用（未知工具 → 工具错误结果）
            // 第二轮：user + assistant + tool → 返回最终回答
            if req.messages.len() <= 1 {
                Ok(response_with(tool_call_msg("nonexistent_tool")))
            } else {
                Ok(response_with(Message::assistant("最终回答")))
            }
        });
        let agent_loop = make_loop(mock, 5);

        let mut messages = vec![Message::user("帮我做点事")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer { content, turns, .. } => {
                assert_eq!(content, "最终回答");
                assert_eq!(turns, 2);
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }
        // 循环结束后消息历史包含 user + assistant + tool + assistant
        assert!(messages.len() >= 4);
    }

    #[tokio::test]
    async fn test_run_ask_user_returns_clarification() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|_| {
            Ok(response_with(Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: "call_ask".to_string(),
                    call_type: ToolCallType::Function,
                    function: FunctionCall {
                        name: "ask_user".to_string(),
                        arguments: r#"{"question": "你希望我怎么处理？"}"#.to_string(),
                    },
                }],
            )))
        });
        let agent_loop = make_loop(mock, 5);

        let mut messages = vec![Message::user("帮我决定一下")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::NeedsClarification {
                question, turns, ..
            } => {
                assert_eq!(question, "你希望我怎么处理？");
                assert_eq!(turns, 1);
            }
            other => panic!("期望 NeedsClarification，得到 {:?}", other),
        }
    }

    /// 回归测试：审批门控拒绝必须通过 `has_pending_approval()` 类型化信号
    /// 降级为追问，而不是解析错误消息中的字符串标记。
    #[tokio::test]
    async fn test_run_approval_denied_returns_clarification() {
        use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
        use std::sync::Arc as StdArc;

        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|_| {
            // write_file 到非 test/temp/tmp 路径 → Medium 风险 → 审批拒绝
            Ok(response_with(Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: "call_write".to_string(),
                    call_type: ToolCallType::Function,
                    function: FunctionCall {
                        name: "write_file".to_string(),
                        arguments: r#"{"path": "project/src/main.rs", "content": "fn main() {}"}"#
                            .to_string(),
                    },
                }],
            )))
        });

        let mut registry = ToolRegistry::new(SecurityPolicy::default());
        registry = registry.with_approval_workflow(StdArc::new(ApprovalWorkflow::new(
            ApprovalWorkflowConfig::default(),
        )));
        let agent_loop = AgentLoop::new(
            Arc::new(mock),
            registry,
            Arc::new(MockSessionManager),
            AgentLoopConfig {
                max_turns: 5,
                ..Default::default()
            },
        );

        let mut messages = vec![Message::user("请帮我写入文件")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::NeedsClarification { question, .. } => {
                assert!(
                    question.contains("安全策略要求确认"),
                    "追问应包含审批确认提示，实际: {question}"
                );
            }
            other => panic!("期望审批降级为 NeedsClarification，得到 {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_run_empty_response_errors() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .returning(|_| Ok(response_with(Message::assistant(""))));
        let agent_loop = make_loop(mock, 5);

        let mut messages = vec![Message::user("你好")];
        let err = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("空响应"));
    }

    #[tokio::test]
    async fn test_run_max_turns_exceeded() {
        let mut mock = MockChatService::new();
        // 每轮都返回工具调用 → 永远不结束 → 达到 max_turns 报错
        mock.expect_chat_completion()
            .returning(|_| Ok(response_with(tool_call_msg("nonexistent_tool"))));
        let agent_loop = make_loop(mock, 2);

        let mut messages = vec![Message::user("循环测试")];
        let err = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("最大轮数"));
    }

    #[tokio::test]
    async fn test_run_llm_error_propagates() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .returning(|_| Err(TianyanError::Custom("模型服务错误：连接超时".to_string())));
        let agent_loop = make_loop(mock, 5);

        let mut messages = vec![Message::user("测试")];
        let err = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("LLM 调用失败"));
    }

    // ── 取消路径（cancel 标志） ──────────────────────────────────────

    /// 预置取消标志：run 应在轮顶立即返回 Cancelled（不发起 LLM 调用）。
    #[tokio::test]
    async fn test_run_returns_cancelled_when_flag_pre_set() {
        // 不配置任何 LLM expectation：若循环发起调用会 panic
        let agent_loop = make_loop(MockChatService::new(), 5);
        let cancel = AtomicBool::new(true);

        let mut messages = vec![Message::user("测试")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                Some(&cancel),
                None,
            )
            .await
            .unwrap();

        assert!(
            matches!(result, AgentLoopResult::Cancelled { turns: 0, .. }),
            "预置取消应返回 Cancelled，实际: {:?}",
            result
        );
    }

    /// 流式中途取消：chunk 循环检测到 cancel 后中断，返回 Cancelled（而非错误）。
    #[tokio::test]
    async fn test_run_stream_returns_cancelled_mid_chunk() {
        use std::sync::atomic::Ordering;

        // mock 流式响应：发第一段后等待测试信号（期间测试置位 cancel）再发后续
        let (gate_tx, gate_rx) = tokio::sync::mpsc::channel::<()>(1);
        let mut gate_rx = Some(gate_rx);
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(move |_| {
            let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(16);
            let mut gate_rx = gate_rx.take();
            tokio::spawn(async move {
                chunk_tx
                    .send(Ok(stream_chunk(Some("第一段"), None, None)))
                    .await
                    .ok();
                if let Some(rx) = &mut gate_rx {
                    rx.recv().await; // 等测试置位 cancel
                }
                chunk_tx
                    .send(Ok(stream_chunk(Some("第二段"), None, None)))
                    .await
                    .ok();
                chunk_tx
                    .send(Ok(stream_chunk(None, None, Some(TokenUsage::default()))))
                    .await
                    .ok();
            });
            Ok(chunk_rx)
        });
        let agent_loop = make_loop(mock, 5);

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<AgentStreamChunk>>(8);
        let sender = StreamEventSender::new(tx);

        // 旁路任务：收到第一段 delta 后置位 cancel 并释放 mock 的 gate
        let cancel_clone = cancel.clone();
        let gate_tx = gate_tx.clone();
        let cancel_watcher = tokio::spawn(async move {
            while let Some(Ok(chunk)) = rx.recv().await {
                if chunk.chunk_type == StreamChunkType::Answer && !chunk.delta.is_empty() {
                    cancel_clone.store(true, Ordering::Relaxed);
                    gate_tx.send(()).await.ok();
                    break;
                }
            }
        });

        let mut messages = vec![Message::user("测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                Some(&cancel),
                None,
            )
            .await
            .unwrap();
        cancel_watcher.await.ok();

        assert!(
            matches!(result, AgentLoopResult::Cancelled { .. }),
            "流式中途取消应返回 Cancelled，实际: {:?}",
            result
        );
    }

    // ── 流式回归测试（run_stream） ──────────────────────────────────

    /// 构造单个流式 chunk（文本 / tool_call delta / usage 三选一组合）。
    fn stream_chunk(
        content: Option<&str>,
        tool_calls: Option<Vec<ToolCallDelta>>,
        usage: Option<TokenUsage>,
    ) -> ChatCompletionChunk {
        stream_chunk_with_finish(content, tool_calls, usage, None)
    }

    /// 构造携带 finish_reason 的流式 chunk（OpenAI 语义：最终 chunk 携带）。
    fn stream_chunk_with_finish(
        content: Option<&str>,
        tool_calls: Option<Vec<ToolCallDelta>>,
        usage: Option<TokenUsage>,
        finish_reason: Option<&str>,
    ) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: "chunk-1".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaContent {
                    role: None,
                    content: content.map(|s| s.to_string()),
                    reasoning_content: None,
                    tool_calls,
                },
                finish_reason: finish_reason.map(|s| s.to_string()),
            }],
            usage,
        }
    }

    /// 构造仅携带 reasoning_content 的流式 chunk（思考模型的推理段）。
    fn stream_chunk_with_reasoning(reasoning: Option<&str>) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: "chunk-r".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaContent {
                    role: None,
                    content: None,
                    reasoning_content: reasoning.map(|s| s.to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    /// mock 固定 chunk 序列的流式响应（每次调用重放同一序列）。
    fn stream_mock(chunks: Vec<ChatCompletionChunk>) -> MockChatService {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(move |_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            for chunk in &chunks {
                tx.try_send(Ok(chunk.clone())).unwrap();
            }
            Ok(rx)
        });
        mock
    }

    /// 收集 stream_sender 上所有 Answer 类型 delta。
    async fn collect_answer_deltas(
        mut rx: tokio::sync::mpsc::Receiver<Result<AgentStreamChunk>>,
    ) -> Vec<String> {
        let mut deltas = Vec::new();
        while let Some(Ok(chunk)) = rx.recv().await {
            if chunk.chunk_type == StreamChunkType::Answer {
                deltas.push(chunk.delta);
            }
        }
        deltas
    }

    /// 流式路径：多 chunk 文本累积 —— 最终消息内容 = 各 chunk 拼接。
    #[tokio::test]
    async fn test_run_stream_accumulates_multi_chunk_content() {
        let chunks = vec![
            stream_chunk(Some("你好"), None, None),
            stream_chunk(Some("，世界"), None, None),
            stream_chunk(Some("！"), None, None),
        ];
        let agent_loop = make_loop(stream_mock(chunks), 5);

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("流式测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer { content, turns, .. } => {
                assert_eq!(content, "你好，世界！");
                assert_eq!(turns, 1);
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }

        // chunk 顺序：stream_sender 收到的 answer_delta 顺序与发送一致
        let deltas = collect_answer_deltas(event_rx).await;
        assert_eq!(deltas, vec!["你好", "，世界", "！"]);
    }

    /// 流式路径：reasoning_content delta 累积进 assistant 消息并逐段推送
    /// Thought 事件（修复思考模型推理内容被静默丢弃导致的空响应误判）。
    #[tokio::test]
    async fn test_run_stream_accumulates_reasoning_and_emits_thought() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            // 思考段：仅 reasoning_content（无正文、无 tool_calls）
            tx.try_send(Ok(stream_chunk_with_reasoning(Some("先分析项目结构"))))
                .unwrap();
            tx.try_send(Ok(stream_chunk_with_reasoning(Some("再检查测试"))))
                .unwrap();
            // 正文段
            tx.try_send(Ok(stream_chunk(Some("审查完成"), None, None)))
                .unwrap();
            Ok(rx)
        });
        let agent_loop = make_loop(mock, 5);

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("审查项目")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        // 最终回答不受推理段影响
        match result {
            AgentLoopResult::Answer { content, .. } => assert_eq!(content, "审查完成"),
            other => panic!("期望 Answer，得到 {:?}", other),
        }

        // assistant 消息携带累积的 reasoning_content（供后续轮次/持久化使用）
        let persisted = messages
            .iter()
            .find(|m| m.role == MessageRole::Assistant)
            .expect("应存在 assistant 消息");
        assert_eq!(
            persisted.reasoning_content.as_deref(),
            Some("先分析项目结构再检查测试")
        );

        // Thought 事件按推理段顺序实时推送（前端 thinking 渲染）
        let mut thoughts = Vec::new();
        while let Some(Ok(chunk)) = event_rx.recv().await {
            if chunk.chunk_type == StreamChunkType::Thought {
                thoughts.push(chunk.delta);
            }
        }
        assert_eq!(thoughts, vec!["先分析项目结构", "再检查测试"]);
    }

    /// 流式路径：tool_calls delta 按 index 累积 —— 分片 id/name/arguments 最终完整。
    #[tokio::test]
    async fn test_run_stream_accumulates_tool_call_deltas() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|req| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            if req.messages.len() <= 1 {
                // 第一轮：tool_call 分片 —— id/name 在首个 delta，arguments 分两次累积
                let chunks = [
                    stream_chunk(
                        None,
                        Some(vec![ToolCallDelta {
                            index: 0,
                            id: Some("call_abc".to_string()),
                            call_type: Some("function".to_string()),
                            function: Some(ToolCallFunctionDelta {
                                name: Some("get_weather".to_string()),
                                arguments: Some(String::new()),
                            }),
                        }]),
                        None,
                    ),
                    stream_chunk(
                        None,
                        Some(vec![ToolCallDelta {
                            index: 0,
                            id: None,
                            call_type: None,
                            function: Some(ToolCallFunctionDelta {
                                name: None,
                                arguments: Some(r#"{"city":"#.to_string()),
                            }),
                        }]),
                        None,
                    ),
                    stream_chunk(
                        None,
                        Some(vec![ToolCallDelta {
                            index: 0,
                            id: None,
                            call_type: None,
                            function: Some(ToolCallFunctionDelta {
                                name: None,
                                arguments: Some("\"北京\"}".to_string()),
                            }),
                        }]),
                        None,
                    ),
                ];
                for chunk in chunks {
                    tx.try_send(Ok(chunk)).unwrap();
                }
            } else {
                // 第二轮：最终回答
                tx.try_send(Ok(stream_chunk(Some("天气：晴"), None, None)))
                    .unwrap();
            }
            Ok(rx)
        });
        let agent_loop = make_loop(mock, 5);

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("北京天气如何？")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer { content, turns, .. } => {
                assert_eq!(content, "天气：晴");
                assert_eq!(turns, 2);
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }

        // 第一轮累积的 assistant 消息应包含完整的 ToolCall（id/name/arguments 拼接）
        let assistant_msg = &messages[1];
        let tool_calls = assistant_msg
            .tool_calls
            .as_ref()
            .expect("第一轮应有 tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_abc");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(tool_calls[0].function.arguments, r#"{"city":"北京"}"#);
    }

    /// 流式路径：第 2 个 chunk 后流中断 —— 返回 agent_loop 前缀的 Custom 错误。
    #[tokio::test]
    async fn test_run_stream_mid_stream_error_returns_custom() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|_| {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tx.try_send(Ok(stream_chunk(Some("部分回答"), None, None)))
                .unwrap();
            tx.try_send(Ok(stream_chunk(Some("继续"), None, None)))
                .unwrap();
            // 第 2 个 chunk 之后流中断
            tx.try_send(Err(TianyanError::Custom("网络中断".to_string())))
                .unwrap();
            Ok(rx)
        });
        let agent_loop = make_loop(mock, 5);

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("测试")];
        let err = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("agent_loop"), "应含 agent_loop 前缀：{msg}");
        assert!(msg.contains("LLM 调用失败"), "应含 LLM 调用失败：{msg}");
        assert!(msg.contains("流式接收中断"), "应含流式接收中断：{msg}");
    }

    /// 流式路径：usage 在最后 chunk —— 最终 token 统计正确。
    #[tokio::test]
    async fn test_run_stream_usage_from_final_chunk() {
        let chunks = vec![
            stream_chunk(Some("你好"), None, None),
            stream_chunk(Some("世界"), None, Some(TokenUsage::new(100, 50))),
        ];
        let agent_loop = make_loop(stream_mock(chunks), 5);

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("流式测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer { total_tokens, .. } => {
                assert_eq!(total_tokens.prompt_tokens, 100);
                assert_eq!(total_tokens.completion_tokens, 50);
                assert_eq!(total_tokens.total_tokens, 150);
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }
    }

    /// 流式路径：answer_delta 顺序与发送 chunk 顺序一致。
    #[tokio::test]
    async fn test_run_stream_answer_delta_order_preserved() {
        let chunks = vec![
            stream_chunk(Some("A"), None, None),
            stream_chunk(Some("B"), None, None),
            stream_chunk(Some("C"), None, None),
            stream_chunk(Some("D"), None, None),
        ];
        let agent_loop = make_loop(stream_mock(chunks), 5);

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("顺序测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(result, AgentLoopResult::Answer { .. }));

        let deltas = collect_answer_deltas(event_rx).await;
        assert_eq!(deltas, vec!["A", "B", "C", "D"]);
    }

    // ── finish_reason 链路（T4） ─────────────────────────────────

    /// 非流式路径：响应 choice 的 finish_reason → 持久化 StructuredMessage.finish。
    #[tokio::test]
    async fn test_run_persists_finish_reason() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|_| {
            let mut resp = response_with(Message::assistant("完成"));
            resp.choices[0].finish_reason = Some("length".to_string());
            Ok(resp)
        });
        let agent_loop = make_loop(mock, 5);

        let mut messages = vec![Message::user("测试")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer {
                persisted_message, ..
            } => {
                assert_eq!(persisted_message.finish.as_deref(), Some("length"));
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }
    }

    /// 流式路径：最终 chunk 的 finish_reason=stop → 持久化消息 finish。
    #[tokio::test]
    async fn test_run_stream_persists_finish_reason() {
        let chunks = vec![
            stream_chunk(Some("你好"), None, None),
            stream_chunk_with_finish(Some("世界"), None, None, Some("stop")),
        ];
        let agent_loop = make_loop(stream_mock(chunks), 5);

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("流式测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer {
                persisted_message, ..
            } => {
                assert_eq!(persisted_message.finish.as_deref(), Some("stop"));
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }
    }

    /// 流式路径：全部 chunk 无 finish_reason → finish 为 None（行为不变兼容）。
    #[tokio::test]
    async fn test_run_stream_no_finish_reason_keeps_none() {
        let chunks = vec![
            stream_chunk(Some("你好"), None, None),
            stream_chunk(Some("世界"), None, None),
        ];
        let agent_loop = make_loop(stream_mock(chunks), 5);

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("流式测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();

        match result {
            AgentLoopResult::Answer {
                persisted_message, ..
            } => {
                assert_eq!(persisted_message.finish, None);
            }
            other => panic!("期望 Answer，得到 {:?}", other),
        }
    }

    // ── 动态 max_tokens（T6） ────────────────────────────────────

    /// 128k/16k 规格的模型规格，供 T6 测试复用。
    fn spec_128k_16k() -> ModelSpec {
        ModelSpec {
            context_length: 128_000,
            max_output_tokens: 16_000,
            max_input_tokens: 112_000,
        }
    }

    /// 非流式：小消息 + 128k 规格 → 请求带 max_tokens = Some(16_000)（预算远超 cap）。
    #[tokio::test]
    async fn test_run_sets_dynamic_max_tokens_with_spec() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|req| {
            assert_eq!(
                req.max_tokens,
                Some(16_000),
                "小消息 + 128k 窗口应达到输出上限 16_000"
            );
            Ok(response_with(Message::assistant("回答")))
        });
        let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

        let mut messages = vec![Message::user("帮我做点事")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(result, AgentLoopResult::Answer { .. }));
    }

    /// 校准跨轮：首轮响应 usage.prompt_tokens=120_000 → 次轮 max_tokens=4096
    /// （剩余 8_000×0.9=7_200 → pow2_floor=4096）。
    #[tokio::test]
    async fn test_run_calibrates_max_tokens_from_prior_usage() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_mock = calls.clone();
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(2)
            .returning(
                move |req| match calls_in_mock.fetch_add(1, Ordering::Relaxed) {
                    0 => {
                        assert_eq!(
                            req.max_tokens,
                            Some(16_000),
                            "首轮无实测输入 → 估算 → 达到输出上限"
                        );
                        let mut resp = response_with(Message::assistant("第一轮"));
                        resp.usage = CommonTokenUsage::new(120_000, 0);
                        Ok(resp)
                    }
                    _ => {
                        assert_eq!(
                            req.max_tokens,
                            Some(4_096),
                            "120_000 实测 → 剩余 8_000×0.9=7_200 → pow2_floor=4096"
                        );
                        Ok(response_with(Message::assistant("第二轮")))
                    }
                },
            );
        let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

        let mut messages = vec![Message::user("帮我做点事")];
        let r1 = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        let r2 = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(r1, AgentLoopResult::Answer { .. }));
        assert!(matches!(r2, AgentLoopResult::Answer { .. }));
        assert_eq!(calls.load(Ordering::Relaxed), 2, "应恰好发起两次请求");
    }

    /// 流式：with_stream(true) 请求同样携带 max_tokens = Some(16_000)。
    #[tokio::test]
    async fn test_run_stream_sets_dynamic_max_tokens_with_spec() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion_stream().returning(|req| {
            assert_eq!(
                req.max_tokens,
                Some(16_000),
                "流式请求应携带动态 max_tokens"
            );
            assert_eq!(req.stream, Some(true));
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tx.try_send(Ok(stream_chunk(Some("你好"), None, None)))
                .unwrap();
            Ok(rx)
        });
        let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let sender = StreamEventSender::new(event_tx);

        let mut messages = vec![Message::user("流式测试")];
        let result = agent_loop
            .run_stream(
                &mut messages,
                sender,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(result, AgentLoopResult::Answer { .. }));
    }

    /// 1024 门槛：预置实测输入使剩余预算 < 1024 → 返回 Err 且不发送请求。
    #[tokio::test]
    async fn test_run_errors_when_budget_below_min() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .returning(|_| panic!("预算不足时不应发送请求"));
        let agent_loop = make_loop(mock, 5).with_chat_spec(Some(spec_128k_16k()));
        // 剩余 1137 × 0.9 = 1023 → pow2_floor = 512 < 1024 → dynamic_max_tokens 返回 None
        agent_loop
            .last_input_usage
            .store(128_000 - 1137, Ordering::Relaxed);

        let mut messages = vec![Message::user("测试")];
        let err = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("上下文预算不足"),
            "错误应提示预算不足，实际: {err}"
        );
    }

    /// 无规格：chat_spec=None → 不设 max_tokens、不报错（行为保持现状）。
    #[tokio::test]
    async fn test_run_without_spec_skips_max_tokens() {
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(|req| {
            assert_eq!(req.max_tokens, None, "无 chat_spec 时不应设置 max_tokens");
            Ok(response_with(Message::assistant("回答")))
        });
        let agent_loop = make_loop(mock, 5); // 默认 chat_spec = None

        let mut messages = vec![Message::user("测试")];
        let result = agent_loop
            .run(
                &mut messages,
                None,
                "session-1",
                None,
                "test-model",
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(result, AgentLoopResult::Answer { .. }));
    }
}
