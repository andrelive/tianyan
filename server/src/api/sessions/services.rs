use std::sync::Arc;

use tracing::{debug, info};

use tianyan::session::SessionManager;

use crate::api::sessions::types::{
    CreateSessionRequest, CreateSessionResponse, DeleteSessionResponse, ListSessionsResponse,
    Session, SessionDetail, SessionMessagesResponse, SessionMetadata, UpdateTitleRequest,
};
use crate::api::shared::short_uuid;
use crate::api::shared::types::ChatMessage;

/// 会话服务，管理对话会话
pub struct SessionService {
    session_manager: Arc<dyn SessionManager>,
}

impl SessionService {
    /// 创建新的会话服务
    pub fn new(session_manager: Arc<dyn SessionManager>) -> Self {
        Self { session_manager }
    }

    /// 列出所有会话
    pub async fn list_sessions(&self) -> anyhow::Result<ListSessionsResponse> {
        // 从会话管理器获取所有会话
        let core_sessions = self.session_manager.list_sessions().await?;

        // 将核心 Session 转换为 API Session
        let sessions: Vec<Session> = core_sessions
            .iter()
            .map(|s| Session {
                id: s.session_id.clone(),
                title: s.title.clone().unwrap_or_else(|| "新对话".to_string()),
                created_at: s.created_at.to_rfc3339(),
                updated_at: s.ended_at.unwrap_or(s.created_at).to_rfc3339(),
                message_count: s.messages.len() as u32,
                metadata: Some(SessionMetadata {
                    model: None,
                    tags: None,
                }),
            })
            .collect();

        let total = sessions.len();

        Ok(ListSessionsResponse { sessions, total })
    }

    /// 创建新会话
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> anyhow::Result<CreateSessionResponse> {
        let session_id = format!("session-{}", short_uuid());

        // 会话实际持久化由 ChatService 通过 SessionManager 完成
        let session = Session {
            id: session_id,
            title: request.title,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            message_count: if request.initial_message.is_some() {
                1
            } else {
                0
            },
            metadata: Some(SessionMetadata {
                model: Some("gpt-4".to_string()),
                tags: None,
            }),
        };

        Ok(CreateSessionResponse { session })
    }

    /// 获取会话详情
    pub async fn get_session_detail(&self, session_id: &str) -> anyhow::Result<SessionDetail> {
        info!("获取会话详情: {}", session_id);

        let core_session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("会话未找到: {}", session_id))?;

        let messages = core_session
            .messages
            .into_iter()
            .map(|m| ChatMessage {
                role: m.role.into(),
                content: m.content,
                timestamp: None,
            })
            .collect();

        Ok(SessionDetail {
            id: core_session.session_id,
            title: core_session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: core_session.created_at.to_rfc3339(),
            updated_at: core_session
                .ended_at
                .unwrap_or(core_session.created_at)
                .to_rfc3339(),
            messages,
        })
    }

    /// 获取会话消息
    pub async fn get_messages(&self, session_id: &str) -> anyhow::Result<SessionMessagesResponse> {
        info!("获取会话消息: {}", session_id);

        let detail = self.get_session_detail(session_id).await?;

        Ok(SessionMessagesResponse {
            session_id: detail.id,
            messages: detail.messages,
        })
    }

    /// 删除会话
    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<DeleteSessionResponse> {
        info!("删除会话: {}", session_id);

        self.session_manager.delete_session(session_id).await?;

        Ok(DeleteSessionResponse {
            success: true,
            message: format!("会话 {} 删除成功", session_id),
        })
    }

    /// 刷新会话到存储（VFS）
    ///
    /// 注意：消息已经由 add_message 直接追加到 JSONL，此方法保留用于兼容性
    pub async fn flush_session(&self, session_id: &str) -> anyhow::Result<()> {
        debug!("会话已直接保存到 VFS，无需刷新: {}", session_id);
        Ok(())
    }

    /// 更新会话标题
    pub async fn update_title(
        &self,
        session_id: &str,
        request: UpdateTitleRequest,
    ) -> anyhow::Result<Session> {
        info!("更新会话标题: {} -> {}", session_id, request.title);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("会话未找到: {}", session_id))?;

        session.title = Some(request.title);

        self.session_manager.update_session(&session).await?;

        Ok(Session {
            id: session.session_id,
            title: session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.ended_at.unwrap_or(session.created_at).to_rfc3339(),
            message_count: session.messages.len() as u32,
            metadata: Some(SessionMetadata {
                model: None,
                tags: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_service_creation() {
        // 编译时测试
    }
}
