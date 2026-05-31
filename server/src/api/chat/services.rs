use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use tianyan::agent::AgentCoordinator;
use tianyan::common::types::Part;
use tianyan::session::SessionManager;

use crate::api::chat::types::{
    ChatRequest, ChatResponse, ChatStreamEvent, EditMessageRequest, RegenerateRequest,
    SkillCallInfo,
};
use crate::api::shared::short_uuid;
use crate::api::shared::types::{ChatMessage, MessageRole, TokenUsage};

/// 对话服务，处理对话逻辑
pub struct ChatService {
    agent: Arc<dyn AgentCoordinator>,
    session_manager: Arc<dyn SessionManager>,
}

impl ChatService {
    /// 创建新的对话服务
    pub fn new(agent: Arc<dyn AgentCoordinator>, session_manager: Arc<dyn SessionManager>) -> Self {
        Self {
            agent,
            session_manager,
        }
    }

    /// 处理对话消息（非流式）
    ///
    /// # Errors
    ///
    /// - 如果 `session_id` 未提供，返回错误
    pub async fn process_message(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let last_message = request
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let response = self
            .agent
            .process_message(session_id, &last_message)
            .await?;

        let chat_response = ChatResponse {
            id: format!("chatcmpl-{}", short_uuid()),
            session_id: session_id.clone(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response.content,
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            usage: TokenUsage {
                prompt_tokens: response.token_usage.prompt_tokens as u32,
                completion_tokens: response.token_usage.completion_tokens as u32,
                total_tokens: response.token_usage.total_tokens as u32,
            },
        };

        Ok(chat_response)
    }

    /// 处理对话消息（流式响应）
    ///
    /// # Errors
    ///
    /// - 如果 `session_id` 未提供，返回错误
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> anyhow::Result<()> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let last_message = request
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let mut stream = self
            .agent
            .process_message_stream(session_id, &last_message)
            .await?;

        let mut chunk_id = 0;

        while let Some(chunk_result) = stream.recv().await {
            match chunk_result {
                Ok(chunk) => {

                    let skill_calls = chunk.skill_calls.map(convert_skill_calls);

                    let event = ChatStreamEvent {
                        id: format!("chatcmpl-{}", chunk_id),
                        session_id: session_id.clone(),
                        delta: chunk.delta,
                        finish_reason: if chunk.is_complete {
                            Some("stop".to_string())
                        } else {
                            None
                        },
                        chunk_type: chunk.chunk_type,
                        skill_calls,
                    };

                    if tx.send(event).await.is_err() {
                        debug!("客户端断开流式连接");
                        break;
                    }
                    chunk_id += 1;
                }
                Err(e) => {
                    error!("流式处理错误: {}", e);
                    let _ = tx
                        .send(ChatStreamEvent {
                            id: "chatcmpl-error".to_string(),
                            session_id: session_id.clone(),
                            delta: format!("错误: {}", e),
                            finish_reason: Some("error".to_string()),
                            chunk_type: tianyan::agent::StreamChunkType::Error,
                            skill_calls: None,
                        })
                        .await;
                    return Err(e.into());
                }
            }
        }

        info!("流式处理完成，会话：{}", session_id);

        Ok(())
    }

    /// 重新生成指定消息之后的助手回复
    pub async fn regenerate_message(
        &self,
        request: RegenerateRequest,
    ) -> anyhow::Result<ChatResponse> {
        let session_id = &request.session_id;

        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("会话未找到: {}", session_id))?;

        let target_msg = session
            .messages
            .get(request.message_index)
            .ok_or_else(|| anyhow::anyhow!("消息索引超出范围"))?;

        if !matches!(
            target_msg.role,
            tianyan::common::types::MessageRole::User
        ) {
            return Err(anyhow::anyhow!("只能重新生成用户消息之后的回复"));
        }

        let last_message: String = target_msg
            .parts
            .iter()
            .filter_map(|p| {
                if let Part::Text { text, .. } = p {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let response = self
            .agent
            .process_message(session_id, &last_message)
            .await?;

        Ok(ChatResponse {
            id: format!("chatcmpl-{}", short_uuid()),
            session_id: session_id.clone(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response.content,
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            usage: TokenUsage {
                prompt_tokens: response.token_usage.prompt_tokens as u32,
                completion_tokens: response.token_usage.completion_tokens as u32,
                total_tokens: response.token_usage.total_tokens as u32,
            },
        })
    }

    /// 编辑用户消息并重新生成回复
    pub async fn edit_message(&self, request: EditMessageRequest) -> anyhow::Result<ChatResponse> {
        let session_id = &request.session_id;

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("会话未找到: {}", session_id))?;

        if request.message_index >= session.messages.len() {
            return Err(anyhow::anyhow!("消息索引超出范围"));
        }

        if !matches!(
            session.messages[request.message_index].role,
            tianyan::common::types::MessageRole::User
        ) {
            return Err(anyhow::anyhow!("只能编辑用户消息"));
        }

        for part in &mut session.messages[request.message_index].parts {
            if let Part::Text { text, .. } = part {
                *text = request.new_content.clone();
                break;
            }
        }
        session.messages.truncate(request.message_index + 1);

        if let Err(e) = self.session_manager.update_session(&session).await {
            error!("更新会话失败: {}", e);
            return Err(anyhow::anyhow!("更新会话失败: {}", e));
        }

        let response = self
            .agent
            .process_message(session_id, &request.new_content)
            .await?;

        Ok(ChatResponse {
            id: format!("chatcmpl-{}", short_uuid()),
            session_id: session_id.clone(),
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: response.content,
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
            },
            usage: TokenUsage {
                prompt_tokens: response.token_usage.prompt_tokens as u32,
                completion_tokens: response.token_usage.completion_tokens as u32,
                total_tokens: response.token_usage.total_tokens as u32,
            },
        })
    }
}

/// 将核心 SkillCallInfo 转换为 API 层 SkillCallInfo
fn convert_skill_calls(calls: Vec<tianyan::agent::SkillCallInfo>) -> Vec<SkillCallInfo> {
    calls
        .into_iter()
        .map(|sc| {
            let skill_id = sc.skill_id.clone();
            SkillCallInfo {
                skill_id: skill_id.clone(),
                skill_name: skill_id,
                success: sc.success,
                execution_time_ms: sc.execution_time_ms,
                error: sc.error,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_service_creation() {
        // 这是一个编译时测试，确保 ChatService 可以被构造
        // 实际测试需要 mock 实现
    }
}
