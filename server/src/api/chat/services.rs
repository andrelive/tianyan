use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use tianyan::agent::{AgentCoordinator, SessionState};
use tianyan::common::types::Part;
use tianyan::session::SessionManager;

use crate::api::chat::types::{
    ChatRequest, ChatResponse, ChatStreamEvent, EditMessageRequest, RegenerateRequest,
    SkillCallInfo,
};
use crate::api::shared::short_uuid;
use crate::api::shared::types::{ChatMessage, MessageRole, TokenUsage};
use crate::core_bridge::convert_message;

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

    /// 根据消息内容自动生成会话标题
    fn generate_title(content: &str) -> String {
        let trimmed = content.trim().replace(['\n', '\r'], " ");
        let chars: Vec<char> = trimmed.chars().collect();
        if chars.len() > 20 {
            let truncated: String = chars.into_iter().take(20).collect();
            format!("{}...", truncated)
        } else {
            trimmed
        }
    }

    /// 检查并更新会话标题（如果标题为空或默认）
    async fn maybe_update_session_title(&self, session_id: &str, message_content: &str) {
        match self.session_manager.get_session(session_id).await {
            Ok(Some(session)) => {
                let need_update = session
                    .title
                    .as_ref()
                    .map(|t| t.is_empty() || t == "新对话")
                    .unwrap_or(true);
                if need_update {
                    let mut updated = session;
                    updated.title = Some(Self::generate_title(message_content));
                    if let Err(e) = self.session_manager.update_session(&updated).await {
                        error!("更新会话标题失败: {}", e);
                    } else {
                        info!("自动生成会话标题: {}", updated.title.as_ref().unwrap());
                    }
                }
            }
            Ok(None) => {
                debug!("会话不存在，跳过标题更新: {}", session_id);
            }
            Err(e) => {
                error!("获取会话失败 {}: {}", session_id, e);
            }
        }
    }

    /// 引导会话上下文：持久化消息、生成标题、构建 SessionState
    async fn bootstrap_session(
        &self,
        session_id: &str,
        messages: &[ChatMessage],
    ) -> (String, SessionState) {
        let last_message = messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let mut state = SessionState::new(session_id);
        for msg in messages {
            state.add_user_message(msg.content.clone());
        }

        if let Some(last_msg) = messages.last() {
            let msg = convert_message(last_msg);
            let is_new_session = self
                .session_manager
                .create_session(session_id, msg.clone())
                .await
                .is_ok();
            if !is_new_session {
                if let Err(e) = self.session_manager.add_message(session_id, msg).await {
                    error!("添加消息到会话失败 {}: {}", session_id, e);
                }
            }
            self.maybe_update_session_title(session_id, &last_msg.content)
                .await;
        }

        (last_message, state)
    }

    /// 处理对话消息（非流式）
    ///
    /// # Errors
    ///
    /// - 如果 `session_id` 未提供，返回错误
    /// - 如果会话不存在且创建失败，返回错误
    pub async fn process_message(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let (last_message, mut state) = self.bootstrap_session(session_id, &request.messages).await;

        let response = self
            .agent
            .process_message(&mut state, &last_message)
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

        // 添加助手回复到会话
        let assistant_msg = tianyan::Message::assistant(&chat_response.message.content);
        if let Err(e) = self
            .session_manager
            .add_message(session_id, assistant_msg)
            .await
        {
            error!("添加助手消息失败：{}", e);
        }

        Ok(chat_response)
    }

    /// 处理对话消息（流式响应）
    ///
    /// # Errors
    ///
    /// - 如果 `session_id` 未提供，返回错误
    /// - 如果会话不存在且创建失败，返回错误
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> anyhow::Result<()> {
        let session_id = request
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session_id 是必填字段"))?;

        let (last_message, mut state) = self.bootstrap_session(session_id, &request.messages).await;

        let mut stream = self
            .agent
            .process_message_stream(&mut state, &last_message)
            .await?;

        let mut chunk_id = 0;
        let mut full_content = String::new();

        while let Some(chunk_result) = stream.recv().await {
            match chunk_result {
                Ok(chunk) => {
                    full_content.push_str(&chunk.delta);

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
                    // 保存已生成的部分内容（如果有）
                    if !full_content.is_empty() {
                        let assistant_msg = tianyan::Message::assistant(&full_content);
                        if let Err(save_err) = self
                            .session_manager
                            .add_message(session_id, assistant_msg)
                            .await
                        {
                            error!("保存部分助手消息失败: {}", save_err);
                        }
                    }
                    return Err(e.into());
                }
            }
        }

        // 添加助手回复到会话
        if !full_content.is_empty() {
            let assistant_msg = tianyan::Message::assistant(&full_content);
            if let Err(e) = self
                .session_manager
                .add_message(session_id, assistant_msg)
                .await
            {
                error!("添加助手消息失败: {}", e);
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

        let messages: Vec<ChatMessage> = session
            .messages
            .iter()
            .map(|m| ChatMessage {
                role: m.role.into(),
                content: m.parts.iter().filter_map(|p| {
                    if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }
                }).collect::<Vec<_>>().join("\n"),
                timestamp: None,
            })
            .collect();

        // 验证message_index指向的是用户消息
        let target_msg = messages
            .get(request.message_index)
            .ok_or_else(|| anyhow::anyhow!("消息索引超出范围"))?;

        if !matches!(target_msg.role, MessageRole::User) {
            return Err(anyhow::anyhow!("只能重新生成用户消息之后的回复"));
        }

        // 构建到该用户消息为止的历史（包含该消息）
        let history: Vec<ChatMessage> = messages
            .iter()
            .take(request.message_index + 1)
            .cloned()
            .collect();

        let chat_request = ChatRequest {
            session_id: Some(session_id.clone()),
            messages: history,
            stream: false,
            temperature: 0.7,
            max_tokens: 2048,
        };

        self.process_message(chat_request).await
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

        let history: Vec<ChatMessage> = session
            .messages
            .iter()
            .map(|m| ChatMessage {
                role: m.role.into(),
                content: m.parts.iter().filter_map(|p| {
                    if let Part::Text { text, .. } = p { Some(text.clone()) } else { None }
                }).collect::<Vec<_>>().join("\n"),
                timestamp: None,
            })
            .collect();

        let chat_request = ChatRequest {
            session_id: Some(session_id.clone()),
            messages: history,
            stream: false,
            temperature: 0.7,
            max_tokens: 2048,
        };

        self.process_message(chat_request).await
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
