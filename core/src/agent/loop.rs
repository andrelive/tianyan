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
    /// 跳过空响应重试（ADR-013 唤醒轮语义）。
    ///
    /// 空输出（无正文无工具调用）本身是正常结束（对齐 DSH：无工具调用即
    /// completed）；普通轮在结束前重试一次（思考模型"想完没说话"的恢复
    /// 机会），唤醒轮（后台任务完成触发，模型可能有意无需回复）不重试。
    pub allow_empty_answer: bool,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 200,
            allow_empty_answer: false,
        }
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
/// 第三元素为模型响应的 finish_reason（非流式来自 choice，流式来自最终 chunk）；
/// 第四元素为更新后的实测输入（`(model, prompt_tokens)`，供下一轮动态 max_tokens 校准）。
type TurnStep<'a> = Pin<
    Box<
        dyn Future<
                Output = Result<
                    (
                        Message,
                        Option<TokenUsage>,
                        Option<String>,
                        Option<(String, usize)>,
                    ),
                    TianyanError,
                >,
            > + Send
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
    /// LLM 用量日志（token 统计：每轮调用落库；None 时不记录）。
    usage_log: Option<Arc<UsageLog>>,
    /// model → provider 映射（配置注入；用量日志的 provider 维度）。
    provider_by_model: HashMap<String, String>,
    /// 单轮演进策略（ADR-030：主 agent 默认；子代理委托策略）。
    turn_policy: TurnPolicy,
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
        self.run_turns(
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
        self.run_turns(
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

                    let request = this
                        .prepare_request(
                            model,
                            &msgs,
                            true,
                            thinking_effort,
                            last_input_usage.clone(),
                        )
                        .await?;

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
                    // 本轮实测输入（流式 usage 到达时更新；供下一轮校准）
                    let mut updated_input: Option<(String, usize)> = None;

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
                                    updated_input = Some((model.to_string(), usage.prompt_tokens));
                                }
                                for choice in &chunk.choices {
                                    // finish_reason：最终 chunk 携带；None 保持之前值（if let Some 覆盖累计）
                                    if let Some(ref fr) = choice.finish_reason {
                                        finish_reason = Some(fr.clone());
                                    }
                                    // 顺序：reasoning 先于 content——同一 chunk 同时携带
                                    // reasoning_content 与 content（推理→正文切换边界）时，
                                    // 思考增量必须先发，正文后发，否则前端出现
                                    // "思考未结束正文已插入"的乱序。
                                    if let Some(ref reasoning) = choice.delta.reasoning_content {
                                        if !reasoning.is_empty() {
                                            accumulated_reasoning.push_str(reasoning);
                                            sender.send_thought(reasoning).await;
                                        }
                                    }
                                    if let Some(ref content) = choice.delta.content {
                                        accumulated_content.push_str(content);
                                        sender.send_answer_delta(content).await;
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

                    // 流式中断容错（对齐 DSH：中断不丢弃已收内容）：
                    // - 已有内容累积（正文/推理/工具调用）→ 保留为截断输出
                    //   （finish=length 语义，前端可提示截断），不再整体报错；
                    // - 完全无内容（流一开始就断）→ 按失败上报。
                    let stream_interrupted = stream_error.is_some();
                    if let Some(err_msg) = &stream_error {
                        let has_partial = !accumulated_content.is_empty()
                            || !accumulated_reasoning.is_empty()
                            || !accumulated_tool_calls.is_empty();
                        if !has_partial {
                            return Err(TianyanError::Custom(format!(
                                "agent_loop: LLM 调用失败：{}",
                                err_msg
                            )));
                        }
                        tracing::warn!(
                            error = %err_msg,
                            content_len = accumulated_content.len(),
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

                    let assistant_msg = Message {
                        role: MessageRole::Assistant,
                        content: accumulated_content,
                        content_parts: None,
                        tool_calls,
                        tool_call_id: None,
                        tool_duration_ms: None,
                        tool_error: None,
                        reasoning_content: if accumulated_reasoning.is_empty() {
                            None
                        } else {
                            Some(accumulated_reasoning)
                        },
                    };

                    // usage 兜底（对齐 DSH token-meter 启发式）：部分网关
                    // （如 ollama 兼容层）流式响应不携带 usage 字段——真实值
                    // 缺失时用 TokenEstimator 估算，保证上下文占用/缓存命中
                    // 展示与压缩判定不因网关差异而失效。
                    let turn_usage = turn_usage.or_else(|| {
                        let estimator = crate::common::token_estimator::TokenEstimator::new();
                        let prompt = last_input_usage
                            .as_ref()
                            .filter(|(m, _)| m == model)
                            .map(|(_, tokens)| *tokens)
                            .unwrap_or_else(|| estimator.estimate_messages(&msgs));
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

                    Ok((assistant_msg, turn_usage, finish_reason, updated_input))
                })
            },
        )
        .await
    }

    /// 运行多轮迭代循环的共享脚手架（`run` / `run_stream` 共用）。
    ///
    /// 统一处理：轮次状态初始化（`current_parent_id` / `total_tokens` /
    /// 实测输入恢复）、[`TurnContext`] 构建、[`AgentLoop::handle_llm_response`]
    /// 调用与早返回、最大轮数错误、取消检查（轮顶 + step 错误时）。
    /// `step` 负责每轮获取模型响应，并统一转换为
    /// `(assistant_msg, turn_usage, finish_reason, 更新后的实测输入)` 形式。
    ///
    /// 实测输入（T6 动态 max_tokens 校准）：从会话消息恢复最后一条带 usage
    /// 的消息（重启后不退回估算——usage 已持久化在消息里），循环内逐轮用
    /// 最新实测更新局部变量（无跨轮共享字段，克隆安全）。
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
        // 实测输入恢复：从会话消息取最后一条带 usage 且 model 一致的消息
        // （重启后不退回估算——usage 已持久化在消息里；model 不一致 = 切换
        // 模型，旧实测值不适用，退回估算。与压缩判定的取数口径一致）。
        let mut last_input_usage: Option<(String, usize)> =
            match self.session_manager.get_session(session_id).await {
                Ok(Some(session)) => session
                    .messages
                    .iter()
                    .rev()
                    .find(|m| {
                        (m.tokens.input > 0 || m.tokens.cache.read > 0)
                            && m.model_id.as_deref() == Some(model)
                    })
                    .map(|m| (model.to_string(), m.tokens.input + m.tokens.cache.read)),
                _ => None,
            };

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
            let turn_start = std::time::Instant::now();
            if let Some(ref trace) = self.trace {
                trace.set_current_turn(session_id, turn as i64);
            }

            let mut step_result = match step(
                self,
                model,
                stream_sender,
                messages.clone(),
                cancel,
                last_input_usage.clone(),
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

            // 空响应重试（用户轮）：LLM 偶发返回既无正文也无工具调用的空响应
            // （思考模型"想完没说话"），直接结束会让任务在工具循环中途静默终止。
            // 消息历史此时未变（空响应尚未入史），重试一次通常能恢复；
            // 重试仍空则按正常结束处理（空输出 = completed，对齐 DSH）。
            // 唤醒轮（allow_empty_answer）不重试——模型可能有意无需回复。
            if !self.config.allow_empty_answer {
                let empty = matches!(
                    &step_result,
                    (m, _, _, _) if m.content.is_empty() && m.tool_calls.is_none()
                );
                if empty {
                    tracing::warn!(
                        session = %session_id,
                        turn,
                        "LLM 返回空响应，重试一次"
                    );
                    step_result = match step(
                        self,
                        model,
                        stream_sender,
                        messages.clone(),
                        cancel,
                        last_input_usage.clone(),
                    )
                    .await
                    {
                        Ok(v) => v,
                        Err(e) => {
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
                }
            }

            let (assistant_msg, turn_usage, finish_reason, updated_input) = step_result;
            // 更新实测输入（供下一轮动态 max_tokens 校准；None = 本轮无实测，保持旧值）
            if let Some(ui) = updated_input {
                last_input_usage = Some(ui);
            }

            // LLM 用量日志：每轮一次（聊天/子代理/演化任务统一记录）。
            // provider 优先查配置映射（模型名可能不含前缀），
            // 未命中时按 model 名前缀推导（provider/model 命名）。
            if let Some(ref usage_log) = self.usage_log {
                if let Some(usage) = &turn_usage {
                    let provider =
                        self.provider_by_model
                            .get(model)
                            .cloned()
                            .unwrap_or_else(|| {
                                model.split("/").next().unwrap_or("unknown").to_string()
                            });
                    usage_log.record(session_id, &provider, model, usage).await;
                }
            }

            let mut ctx = TurnContext {
                session_id,
                current_parent_id: &mut current_parent_id,
                total_tokens: &mut total_tokens,
                messages: &mut *messages,
                stream_sender,
                turn,
                model,
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

        // ADR-030：max_turns 行为按策略——主 agent 报错；子代理尽力而为
        // （返回最后输出，不因轮数上限丢失已完成的工作）
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
