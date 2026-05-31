use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::types::{Message, MessageRole};
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
    Answer(String),
    /// 需要追问。
    NeedsClarification { question: String },
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

    /// 运行迭代循环直到回答或追问。
    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
        session_id: &str,
        initial_parent_id: Option<&str>,
    ) -> Result<AgentLoopResult, AgentLoopError> {
        let mut current_parent_id = initial_parent_id.map(|s| s.to_string());
        for _turn in 0..self.config.max_turns {
            let request = ChatCompletionRequest::new(&self.config.model, messages.clone())
                .with_tools(self.tool_registry.definitions().to_vec());

            let response = self
                .model_service
                .chat_completion(request)
                .await
                .map_err(|e| AgentLoopError::LlmCallFailed(e.to_string()))?;

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
                // Persist the final answer before returning
                let answer_for_persist = ContextAssembler::message_to_structured(
                    &Message::assistant(&assistant_msg.content),
                    session_id,
                    current_parent_id.as_deref(),
                );
                if let Err(e) = self.session_manager.add_structured_message(session_id, answer_for_persist).await {
                    tracing::warn!(error = %e, "持久化 assistant answer 失败");
                }
                return Ok(AgentLoopResult::Answer(assistant_msg.content));
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
