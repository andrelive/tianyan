use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::types::{Message, MessageRole, StructuredMessage, TokenUsage};
use crate::context::ContextAssembler;
use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use crate::session::SessionManager;

/// Agent 循环配置。
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    /// 最大轮数。
    pub max_turns: usize,
    /// 使用的模型。
    pub model: String,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 20,
            model: "default".to_string(),
        }
    }
}

/// Agent 循环结果。
#[derive(Debug, Clone)]
pub enum AgentLoopResult {
    /// 直接回答。
    Answer {
        content: String,
        total_tokens: TokenUsage,
        turns: usize,
        /// AgentLoop 已持久化的 StructuredMessage，coordinator 直接复用此消息加入状态，避免重复创建。
        persisted_message: StructuredMessage,
    },
    /// 需要追问。
    NeedsClarification {
        question: String,
        total_tokens: TokenUsage,
        turns: usize,
    },
}

/// Agent 循环错误。
#[derive(thiserror::Error, Debug)]
pub enum AgentLoopError {
    /// 达到最大轮数。
    #[error("Reached maximum turns: {0}")]
    MaxTurnsReached(usize),
    /// LLM 调用失败。
    #[error("LLM call failed: {0}")]
    LlmCallFailed(String),
    /// 空响应。
    #[error("Empty response from LLM")]
    EmptyResponse,
}

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

    /// 获取 tool_registry 引用（用于访问执行轨迹等）。
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// 运行迭代循环直到回答或追问。
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
    ) -> Result<AgentLoopResult, AgentLoopError> {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();

        for turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(&self.config.model, messages.clone())
                .with_tools(self.tool_registry.definitions().to_vec());

            let response = self
                .model_service
                .chat_completion(request)
                .await
                .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;

            let turn_usage = response.usage.clone();
            total_tokens.prompt_tokens += turn_usage.prompt_tokens;
            total_tokens.completion_tokens += turn_usage.completion_tokens;
            total_tokens.total_tokens += turn_usage.total_tokens;

            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or(AgentLoopError::EmptyResponse)?;

            let assistant_msg = choice.message;

            // Check for ask_user before adding to history
            if let Some(ref tool_calls) = assistant_msg.tool_calls {
                if let Some(ask_call) = tool_calls.iter().find(|tc| tc.function.name == "ask_user")
                {
                    let params: AskUserParams = serde_json::from_str(&ask_call.function.arguments)
                        .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;
                    return Ok(AgentLoopResult::NeedsClarification {
                        question: params.question,
                        total_tokens,
                        turns: turn + 1,
                    });
                }
            }

            // Add assistant message to history
            messages.push(Message {
                role: MessageRole::Assistant,
                content: assistant_msg.content.clone(),
                tool_calls: assistant_msg.tool_calls.clone(),
                tool_call_id: None,
                reasoning_content: assistant_msg.reasoning_content.clone(),
            });

            // Persist assistant message via SessionManager
            let assistant_for_persist = ContextAssembler::message_to_structured(
                &Message {
                    role: MessageRole::Assistant,
                    content: assistant_msg.content.clone(),
                    tool_calls: assistant_msg.tool_calls.clone(),
                    tool_call_id: None,
                    reasoning_content: assistant_msg.reasoning_content.clone(),
                },
                session_id,
                current_parent_id.as_deref(),
                Some(turn_usage.clone()),
            );
            current_parent_id = Some(assistant_for_persist.id.clone());
            if let Err(e) = self.session_manager.add_structured_message(session_id, assistant_for_persist).await {
                tracing::warn!(error = %e, "持久化 assistant 消息失败");
            }

            if let Some(ref tool_calls) = assistant_msg.tool_calls {
                if let Some(ref sender) = stream_sender {
                    for tc in tool_calls {
                        sender
                            .send_tool_call(&format!("\u{8c03}\u{7528}: {}", tc.function.name))
                            .await;
                    }
                }

                let results = self.tool_registry.execute_parallel(tool_calls).await;

                for (call_id, result) in results {
                    let content = match result {
                        Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
                        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                    };

                    // Persist tool result
                    let tool_msg = Message::tool(&call_id, &content);
                    let tool_for_persist = ContextAssembler::message_to_structured(
                        &tool_msg,
                        session_id,
                        current_parent_id.as_deref(),
                        None,
                    );
                    current_parent_id = Some(tool_for_persist.id.clone());
                    if let Err(e) = self.session_manager.add_structured_message(session_id, tool_for_persist).await {
                        tracing::warn!(error = %e, "持久化 tool result 消息失败");
                    }

                    messages.push(tool_msg);

                    if let Some(ref sender) = stream_sender {
                        sender.send_observation(&content).await;
                    }
                }
            } else {
                // Persist the final answer before returning — with token_usage
                let answer_for_persist = ContextAssembler::message_to_structured(
                    &Message::assistant(&assistant_msg.content),
                    session_id,
                    current_parent_id.as_deref(),
                    Some(turn_usage.clone()),
                );
                if let Err(e) = self.session_manager.add_structured_message(session_id, answer_for_persist.clone()).await {
                    tracing::warn!(error = %e, "持久化 assistant answer 失败");
                }
                return Ok(AgentLoopResult::Answer {
                    content: assistant_msg.content,
                    total_tokens,
                    turns: turn + 1,
                    persisted_message: answer_for_persist,
                });
            }
        }

        Err(AgentLoopError::MaxTurnsReached(self.config.max_turns))
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
    ) -> Result<AgentLoopResult, AgentLoopError> {
        use crate::common::types::{FunctionCall, ToolCall as CoreToolCall, ToolCallType};

        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        let mut total_tokens = TokenUsage::default();

        for turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(&self.config.model, messages.clone())
                .with_stream(true)
                .with_tools(self.tool_registry.definitions().to_vec());

            let mut rx = self
                .model_service
                .chat_completion_stream(request)
                .await
                .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;

            // Accumulators for streaming chunks
            let mut accumulated_content = String::new();
            let mut accumulated_tool_calls: std::collections::BTreeMap<usize, (String, String, String)> =
                std::collections::BTreeMap::new();
            let mut turn_usage: Option<TokenUsage> = None;
            // (index, (id, name, arguments))

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
                                    let entry = accumulated_tool_calls
                                        .entry(tc.index)
                                        .or_insert_with(|| (String::new(), String::new(), String::new()));
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
                        tracing::warn!(error = %e, "流式接收中断");
                    }
                }
            }

            // Accumulate turn token usage
            if let Some(ref usage) = turn_usage {
                total_tokens.prompt_tokens += usage.prompt_tokens;
                total_tokens.completion_tokens += usage.completion_tokens;
                total_tokens.total_tokens += usage.total_tokens;
            }

            // Check for ask_user from accumulated tool calls
            let has_ask_user = accumulated_tool_calls.values().any(|(_, name, _)| name == "ask_user");
            if has_ask_user {
                if let Some((_, _, args)) = accumulated_tool_calls.values().find(|(_, name, _)| name == "ask_user") {
                    let params: AskUserParams = serde_json::from_str(args)
                        .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;
                    return Ok(AgentLoopResult::NeedsClarification {
                        question: params.question,
                        total_tokens,
                        turns: turn + 1,
                    });
                }
            }

            // Build assistant message from accumulated content + tool calls
            let tool_calls: Option<Vec<CoreToolCall>> = if accumulated_tool_calls.is_empty() {
                None
            } else {
                Some(
                    accumulated_tool_calls
                        .into_values()
                        .map(|(id, name, arguments)| CoreToolCall {
                            id,
                            call_type: ToolCallType::Function,
                            function: FunctionCall { name, arguments },
                        })
                        .collect(),
                )
            };

            let assistant_msg = Message {
                role: MessageRole::Assistant,
                content: accumulated_content.clone(),
                tool_calls: tool_calls.clone(),
                tool_call_id: None,
                reasoning_content: None,
            };

            // Persist assistant message with turn token usage
            let assistant_for_persist = ContextAssembler::message_to_structured(
                &assistant_msg,
                session_id,
                current_parent_id.as_deref(),
                turn_usage.clone(),
            );
            current_parent_id = Some(assistant_for_persist.id.clone());
            if let Err(e) = self.session_manager.add_structured_message(session_id, assistant_for_persist).await {
                tracing::warn!(error = %e, "持久化 assistant 消息失败");
            }

            messages.push(assistant_msg);

            if let Some(ref tcs) = tool_calls {
                for tc in tcs {
                    stream_sender
                        .send_tool_call(&format!("\u{8c03}\u{7528}: {}", tc.function.name))
                        .await;
                }

                let calls: Vec<crate::model::types::ToolCall> = tcs
                    .iter()
                    .map(|tc| crate::model::types::ToolCall {
                        id: tc.id.clone(),
                        call_type: ToolCallType::Function,
                        function: FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    })
                    .collect();

                let results = self.tool_registry.execute_parallel(&calls).await;

                for (call_id, result) in results {
                    let content = match result {
                        Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
                        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
                    };

                    let tool_msg = Message::tool(&call_id, &content);
                    let tool_for_persist = ContextAssembler::message_to_structured(
                        &tool_msg,
                        session_id,
                        current_parent_id.as_deref(),
                        None,
                    );
                    current_parent_id = Some(tool_for_persist.id.clone());
                    if let Err(e) = self.session_manager.add_structured_message(session_id, tool_for_persist).await {
                        tracing::warn!(error = %e, "持久化 tool result 消息失败");
                    }

                    messages.push(tool_msg);
                    stream_sender.send_observation(&content).await;
                }
            } else {
                // Persist final answer with turn token usage
                let answer_for_persist = ContextAssembler::message_to_structured(
                    &Message::assistant(&accumulated_content),
                    session_id,
                    current_parent_id.as_deref(),
                    turn_usage.clone(),
                );
                if let Err(e) = self.session_manager.add_structured_message(session_id, answer_for_persist.clone()).await {
                    tracing::warn!(error = %e, "持久化 assistant answer 失败");
                }
                return Ok(AgentLoopResult::Answer {
                    content: accumulated_content,
                    total_tokens,
                    turns: turn + 1,
                    persisted_message: answer_for_persist,
                });
            }
        }

        Err(AgentLoopError::MaxTurnsReached(self.config.max_turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::common::error::Result;
    use crate::common::types::StructuredMessage;
    use crate::session::Session;

    struct MockSessionManager;
    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn add_structured_message(&self, _session_id: &str, _msg: StructuredMessage) -> Result<()> { Ok(()) }
        async fn add_message(&self, _session_id: &str, _message: Message) -> Result<()> { Ok(()) }
        async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> { unimplemented!() }
        async fn get_session(&self, _id: &str) -> Result<Option<Session>> { unimplemented!() }
        async fn update_session(&self, _session: &Session) -> Result<()> { unimplemented!() }
        async fn list_sessions(&self) -> Result<Vec<Session>> { unimplemented!() }
        async fn delete_session(&self, _id: &str) -> Result<()> { unimplemented!() }
    }

    #[test]
    fn test_agent_loop_config_default() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_turns, 20);
        assert_eq!(config.model, "default");
    }

    #[test]
    fn test_agent_loop_error_display() {
        let err = AgentLoopError::MaxTurnsReached(5);
        assert!(err.to_string().contains("5"));
    }
}
