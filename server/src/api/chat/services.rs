use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use tianyan::agent::AgentCoordinator;
use tianyan::session::SessionManager;
use tianyan::Message as CoreMessage;
use tianyan::MessageRole as CoreMessageRole;

use crate::api::chat::types::{ChatRequest, ChatResponse, ChatStreamEvent, SkillCallInfo};
use crate::api::shared::error::ApiError;
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
    /// 如果 `session_id` 未提供，会自动创建新会话。
    pub async fn process_message(&self, request: ChatRequest) -> Result<ChatResponse, ApiError> {
        let last_message = request
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let session_id = resolve_or_create_session(
            self.session_manager.as_ref(),
            request.session_id.as_deref(),
            &last_message,
        )
        .await?;

        let response = self
            .agent
            .process_message(&session_id, &last_message, request.model.as_deref())
            .await?;

        Ok(to_chat_response(&session_id, response))
    }

    /// 处理对话消息（流式响应）
    ///
    /// 如果 `session_id` 未提供，会自动创建新会话。
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> Result<(), ApiError> {
        let last_message = request
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let session_id = resolve_or_create_session(
            self.session_manager.as_ref(),
            request.session_id.as_deref(),
            &last_message,
        )
        .await?;

        let mut stream = self
            .agent
            .process_message_stream(&session_id, &last_message, request.model.as_deref())
            .await?;

        // 一次流式响应对应一个响应 id（与非流式 ChatResponse.id 语义一致）。
        // SSE 事件 id 用于 Last-Event-ID 重连，同一响应流的所有 chunk 共享该 id。
        let stream_id = format!("chatcmpl-{}", short_uuid());

        while let Some(chunk_result) = stream.recv().await {
            match chunk_result {
                Ok(chunk) => {
                    let skill_calls = chunk.skill_calls.map(convert_skill_calls);

                    let event = ChatStreamEvent {
                        id: stream_id.clone(),
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
                }
                Err(e) => {
                    error!("流式处理错误: {}", e);
                    let _ = tx
                        .send(ChatStreamEvent {
                            id: stream_id.clone(),
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

    /// 处理用户对追问的回答（非流式）。
    ///
    /// 会话必须存在待处理的追问（由 AgentLoop 在 ask_user 工具触发时设置）。
    pub async fn handle_clarification(
        &self,
        session_id: &str,
        answer: &str,
    ) -> Result<ChatResponse, ApiError> {
        let response = self.agent.handle_clarification(session_id, answer).await?;
        Ok(to_chat_response(session_id, response))
    }
}

/// 将核心 AgentResponse 转换为 API 层 ChatResponse。
fn to_chat_response(session_id: &str, response: tianyan::agent::AgentResponse) -> ChatResponse {
    ChatResponse {
        id: format!("chatcmpl-{}", short_uuid()),
        session_id: session_id.to_string(),
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

/// 根据请求中的 session_id 决定复用已有会话还是创建新会话。
/// 根据请求中的 session_id 决定复用已有会话还是创建新会话。
/// 新建会话时自动用第一条用户消息生成标题。
async fn resolve_or_create_session(
    session_manager: &dyn SessionManager,
    session_id: Option<&str>,
    initial_message: &str,
) -> Result<String, ApiError> {
    // 如果传了 session_id 且服务端已存在，直接复用
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if session_manager.get_session(sid).await?.is_some() {
            return Ok(sid.to_string());
        }
    }

    // 新建会话：优先用前端传来的 session_id，否则生成一个
    let new_id = session_id
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("session-{}", short_uuid()));
    let msg = CoreMessage::new(CoreMessageRole::User, initial_message);
    let mut session = session_manager
        .create_session(&new_id, msg)
        .await
        .map_err(|e| ApiError::Internal(format!("创建会话失败: {}", e)))?;

    // 用第一条用户消息生成标题（取第一行或前 30 个字符）
    let title = generate_session_title(initial_message);
    if !title.is_empty() {
        session.title = Some(title);
        if let Err(e) = session_manager.update_session(&session).await {
            tracing::warn!(error = %e, "会话标题更新失败");
        }
    }

    // 清空 create_session 预写的消息：会话历史统一由 agent.process_message 追加，
    // 避免同一条用户消息在历史中重复
    session.messages.clear();
    if let Err(e) = session_manager.rewrite_messages(&new_id, &[]).await {
        tracing::warn!(error = %e, "清空会话预写消息失败");
    }

    Ok(new_id)
}

/// 从用户消息中提取会话标题。
/// 优先取第一行（到第一个换行符），超过 30 字则截断并加 "…"。
fn generate_session_title(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return String::new();
    }
    if first_line.chars().count() > 30 {
        let truncated: String = first_line.chars().take(30).chain(['…']).collect();
        return truncated;
    }
    first_line.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_chat_service_creation() {
        // 这是一个编译时测试，确保 ChatService 可以被构造
        // 实际测试需要 mock 实现
    }
}
