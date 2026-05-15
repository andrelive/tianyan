use std::sync::Arc;

use crate::agent::tool_params::AskUserParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::agent::types::StreamEventSender;
use crate::common::types::{Message, MessageRole};
use crate::model::ChatCompletionRequest;
use crate::model::ModelService;

#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_turns: usize,
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

#[derive(Debug, Clone)]
pub enum AgentLoopResult {
    Answer(String),
    NeedsClarification { question: String },
}

#[derive(thiserror::Error, Debug)]
pub enum AgentLoopError {
    #[error("Reached maximum turns: {0}")]
    MaxTurnsReached(usize),
    #[error("LLM call failed: {0}")]
    LlmCallFailed(String),
    #[error("Empty response from LLM")]
    EmptyResponse,
}

#[derive(Clone)]
pub struct AgentLoop {
    model_service: Arc<dyn ModelService>,
    tool_registry: ToolRegistry,
    config: AgentLoopConfig,
}

impl AgentLoop {
    pub fn new(
        model_service: Arc<dyn ModelService>,
        tool_registry: ToolRegistry,
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            model_service,
            tool_registry,
            config,
        }
    }

    pub async fn run(
        &self,
        messages: &mut Vec<Message>,
        stream_sender: Option<StreamEventSender>,
    ) -> Result<AgentLoopResult, AgentLoopError> {
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
            });

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

                    messages.push(Message::tool(&call_id, &content));

                    if let Some(ref sender) = stream_sender {
                        sender.send_observation(&content).await;
                    }
                }
            } else {
                return Ok(AgentLoopResult::Answer(assistant_msg.content));
            }
        }

        Err(AgentLoopError::MaxTurnsReached(self.config.max_turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
