use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::common::error::TianyanError;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::types::{FunctionCall, Message, MessageRole, StructuredMessage, TokenUsage};
use crate::common::types::{ToolCall, ToolCallType};
use crate::context::ContextAssembler;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use crate::session::SessionManager;

/// Agent 循环配置。
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// 最大轮数。
    pub max_turns: usize,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 200,
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
        persisted_message: StructuredMessage,
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
}

// AgentLoopError replaced with TianyanError::Custom("agent_loop: ...")

/// Agent 迭代循环。
#[derive(Clone)]
pub struct AgentLoop {
    model_service: Arc<dyn ChatService>,
    tool_registry: ToolRegistry,
    session_manager: Arc<dyn SessionManager>,
    config: AgentLoopConfig,
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
        }
    }

    /// 获取工具注册表引用。
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// 运行迭代循环直到回答或追问（非流式）。
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
    ) -> Result<AgentLoopResult, TianyanError> {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();

        for turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(model, messages.clone())
                .with_tools(self.tool_registry.definitions().to_vec());

            let response = self
                .model_service
                .chat_completion(request)
                .await
                .map_err(|e| TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e.to_string())))?;

            let turn_usage = response.usage;
            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or(TianyanError::Custom("agent_loop: LLM 返回空响应".to_string()))?;

            if let Some(result) = self
                .handle_llm_response(
                    &choice.message,
                    Some(turn_usage),
                    &mut current_parent_id,
                    &mut total_tokens,
                    messages,
                    stream_sender.as_ref(),
                    session_id,
                    turn,
                )
                .await?
            {
                return Ok(result);
            }
        }

        Err(TianyanError::Custom(format!("agent_loop: 达到最大轮数限制：{}", self.config.max_turns)))
    }

    /// 运行迭代循环（流式），将 LLM 文本增量实时推送到前端。
    ///
    /// 与 [`run`] 的区别：每个 LLM 调用使用 `chat_completion_stream`，
    /// 文本 delta 通过 `stream_sender` 逐 token 发送，实现打字机效果。
    /// tool call 检测和执行仍为非流式（在完整消息累积后处理）。
    pub async fn run_stream(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: StreamEventSender,
        session_id: &str,
        initial_parent_id: Option<&str>,
        model: &str,
    ) -> Result<AgentLoopResult, TianyanError> {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();

        for turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(model, messages.clone())
                .with_stream(true)
                .with_tools(self.tool_registry.definitions().to_vec());

            let mut rx = self
                .model_service
                .chat_completion_stream(request)
                .await
                .map_err(|e| TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e.to_string())))?;

            // Accumulators for streaming chunks
            let mut accumulated_content = String::new();
            let mut accumulated_tool_calls: std::collections::BTreeMap<
                usize,
                (String, String, String),
            > = std::collections::BTreeMap::new();
            let mut turn_usage: Option<TokenUsage> = None;
            let mut stream_error: Option<String> = None;

            while let Some(chunk_result) = rx.recv().await {
                match chunk_result {
                    Ok(chunk) => {
                        // OpenAI sends usage in the final chunk
                        if let Some(ref usage) = chunk.usage {
                            turn_usage = Some(usage.clone());
                        }
                        for choice in &chunk.choices {
                            if let Some(ref content) = choice.delta.content {
                                accumulated_content.push_str(content);
                                stream_sender.send_answer_delta(content).await;
                            }
                            if let Some(ref tc_deltas) = choice.delta.tool_calls {
                                for tc in tc_deltas {
                                    let entry =
                                        accumulated_tool_calls.entry(tc.index).or_insert_with(
                                            || (String::new(), String::new(), String::new()),
                                        );
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
                return Err(TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", err_msg)));
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
                tool_calls,
                tool_call_id: None,
                reasoning_content: None,
            };

            if let Some(result) = self
                .handle_llm_response(
                    &assistant_msg,
                    turn_usage,
                    &mut current_parent_id,
                    &mut total_tokens,
                    messages,
                    Some(&stream_sender),
                    session_id,
                    turn,
                )
                .await?
            {
                return Ok(result);
            }
        }

        Err(TianyanError::Custom(format!("agent_loop: 达到最大轮数限制：{}", self.config.max_turns)))
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
        current_parent_id: &mut Option<String>,
        total_tokens: &mut TokenUsage,
        messages: &mut Vec<Message>,
        stream_sender: Option<&StreamEventSender>,
        session_id: &str,
        turn: usize,
    ) -> Result<Option<AgentLoopResult>, TianyanError> {
        // Accumulate turn token usage
        if let Some(ref usage) = turn_usage {
            total_tokens.prompt_tokens += usage.prompt_tokens;
            total_tokens.completion_tokens += usage.completion_tokens;
            total_tokens.total_tokens += usage.total_tokens;
        }

        // Check for ask_user before adding to history
        if let Some(ref tool_calls) = assistant_msg.tool_calls {
            if let Some(ask_call) = tool_calls
                .iter()
                .find(|tc| tc.function.name == "ask_user")
            {
                let params: AskUserParams = serde_json::from_str(&ask_call.function.arguments)
                .map_err(|e| TianyanError::Custom(format!("agent_loop: LLM 调用失败：{}", e.to_string())))?;
                return Ok(Some(AgentLoopResult::NeedsClarification {
                    question: params.question,
                    total_tokens: total_tokens.clone(),
                    turns: turn + 1,
                }));
            }
        }

        // Add assistant message to in-memory history
        messages.push(assistant_msg.clone());

        // Persist assistant message via SessionManager
        let assistant_for_persist = ContextAssembler::message_to_structured(
            assistant_msg,
            session_id,
            current_parent_id.as_deref(),
            turn_usage,
        );
        *current_parent_id = Some(assistant_for_persist.id.clone());
        // Clone before moving into add_structured_message — needed for the
        // Answer result in the no-tool-calls branch below.
        let persisted = assistant_for_persist.clone();
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, assistant_for_persist)
            .await
        {
            tracing::warn!(error = %e, "持久化 assistant 消息失败");
        }

        if let Some(ref tool_calls) = assistant_msg.tool_calls {
            // Notify about tool calls if sender is available
            if let Some(sender) = stream_sender {
                for tc in tool_calls {
                    sender
                        .send_tool_call(&format!("\u{8c03}\u{7528}: {}", tc.function.name))
                        .await;
                }
            }

            let results = self.tool_registry.execute_parallel(tool_calls).await;

            for (call_id, result) in results {
                let content = match result {
                    Ok(ref value) => serde_json::to_string(value).unwrap_or_else(|e| {
                        format!(r#"{{"error": "serialization failed: {}"}}"#, e)
                    }),
                    Err(ref e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                };

                // Persist tool result
                let tool_msg = Message::tool(&call_id, &content);
                let tool_for_persist = ContextAssembler::message_to_structured(
                    &tool_msg,
                    session_id,
                    current_parent_id.as_deref(),
                    None,
                );
                *current_parent_id = Some(tool_for_persist.id.clone());
                if let Err(e) = self
                    .session_manager
                    .add_structured_message(session_id, tool_for_persist)
                    .await
                {
                    tracing::warn!(error = %e, "持久化 tool result 消息失败");
                }

                messages.push(tool_msg);

                if let Some(sender) = stream_sender {
                    sender.send_observation(&content).await;
                }
            }

            Ok(None)
        } else if assistant_msg.content.is_empty() {
            // LLM returned neither content nor tool calls — treat as error
            // rather than silently continuing the loop (which would consume
            // up to max_turns with no progress).
            Err(TianyanError::Custom("agent_loop: LLM 返回空响应".to_string()))
        } else {
            // Already persisted above — just return.
            Ok(Some(AgentLoopResult::Answer {
                content: assistant_msg.content.clone(),
                total_tokens: total_tokens.clone(),
                turns: turn + 1,
                persisted_message: persisted,
            }))
        }
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
        async fn add_message(&self, _session_id: &str, _message: Message) -> Result<()> {
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
    }

    #[test]
    fn test_agent_loop_error_display() {
        let err = TianyanError::Custom(format!("agent_loop: 达到最大轮数限制：{}", 5));
        assert!(err.to_string().contains("5"));
    }
}
