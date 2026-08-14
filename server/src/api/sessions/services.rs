use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;

use tianyan::common::types::Part;
use tianyan::session::SessionManager;

use crate::api::sessions::types::{
    DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse, RedoRequest, Session,
    SessionDetail, SessionMessagesResponse, SessionMetadata, UpdateTitleRequest,
};
use crate::api::shared::error::ApiError;
use crate::api::shared::types::ChatMessage;
use tianyan::common::types::{MessageRole, StructuredMessage};
use tianyan::snapshot::SnapshotManager;

/// 会话服务，管理对话会话
pub struct SessionService {
    session_manager: Arc<dyn SessionManager>,
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory；会话级绑定缺省时的快照根）。
    default_working_dir: Option<PathBuf>,
}

impl SessionService {
    /// 创建新的会话服务
    pub fn new(
        session_manager: Arc<dyn SessionManager>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
        default_working_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            session_manager,
            snapshot_manager,
            default_working_dir,
        }
    }

    /// 解析会话生效的工作目录（会话级绑定优先，缺省回退全局配置）。
    fn resolve_workdir(&self, session: &tianyan::session::Session) -> Option<PathBuf> {
        session.working_directory(self.default_working_dir.as_deref())
    }

    /// 列出所有会话
    pub async fn list_sessions(&self) -> Result<ListSessionsResponse, ApiError> {
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
                working_directory: s.header.working_directory.clone(),
                metadata: Some(SessionMetadata {
                    model: None,
                    tags: None,
                }),
            })
            .collect();

        let total = sessions.len();

        Ok(ListSessionsResponse { sessions, total })
    }

    /// 获取会话详情
    pub async fn get_session_detail(&self, session_id: &str) -> Result<SessionDetail, ApiError> {
        info!("获取会话详情: {}", session_id);

        let core_session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        let messages = core_session
            .messages
            .into_iter()
            .map(|m| {
                let content = m.parts.iter().fold(String::new(), |mut acc, p| {
                    match p {
                        Part::Text { text, .. } => {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            acc.push_str(text);
                        }
                        Part::Reasoning { text, .. } => {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            acc.push_str(&format!("[思考] {}", text));
                        }
                        Part::ToolCall {
                            name, arguments, ..
                        } => {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            let truncated_args = if arguments.len() > 200 {
                                format!("{}...", &arguments[..200])
                            } else {
                                arguments.clone()
                            };
                            acc.push_str(&format!("[调用工具: {}({})]", name, truncated_args));
                        }
                        Part::ToolResult {
                            tool_call_id,
                            content,
                            ..
                        } => {
                            if !acc.is_empty() {
                                acc.push('\n');
                            }
                            let truncated = if content.len() > 500 {
                                format!("{}...", &content[..500])
                            } else {
                                content.clone()
                            };
                            acc.push_str(&format!("[工具结果 {}] {}", tool_call_id, truncated));
                        }
                        // 图片不进文本拼接（前端经 images 字段渲染）
                        Part::Image { .. } => {}
                    }
                    acc
                });
                // 提取历史消息中的图片 data URL（Part::Image → API images 字段）
                let images = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Image { url, .. } => Some(url.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                ChatMessage {
                    // Tool 角色在 API 层映射为 Assistant（与旧 core_bridge 转换一致），
                    // 工具结果已展平为 [工具结果] 文本，前端不消费 tool 角色。
                    role: match m.role {
                        MessageRole::Tool => MessageRole::Assistant,
                        role => role,
                    },
                    content,
                    images: if images.is_empty() {
                        None
                    } else {
                        Some(images)
                    },
                    timestamp: None,
                }
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
    pub async fn get_messages(
        &self,
        session_id: &str,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!("获取会话消息: {}", session_id);

        let detail = self.get_session_detail(session_id).await?;

        Ok(SessionMessagesResponse {
            session_id: detail.id,
            messages: detail.messages,
        })
    }

    /// 删除消息及其后的所有消息（与编辑/重新生成一致的截断语义）。
    ///
    /// 保留 `message_index` 之前的消息，删除该消息及之后，持久化后返回剩余消息。
    pub async fn delete_message(
        &self,
        session_id: &str,
        request: DeleteMessageRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!(
            "删除消息: 会话={}, 索引={}",
            session_id, request.message_index
        );

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        if request.message_index >= session.messages.len() {
            return Err(ApiError::BadRequest("消息索引超出范围".to_string()));
        }

        // 保存重做状态：当前工作区 + 被截断的消息（配置了 working_directory 时；
        // 快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let truncated: Vec<StructuredMessage> =
                session.messages[request.message_index..].to_vec();
            if let Some(workdir) = self.resolve_workdir(&session) {
                if let Err(e) = sm
                    .with_workdir(workdir)
                    .save_redo(session_id, request.message_index, &truncated)
                    .await
                {
                    tracing::warn!(error = %e, "保存重做状态失败");
                }
            }
        }

        // 截断到该消息之前（不含）
        session.messages.truncate(request.message_index);

        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::error!("更新会话失败: {}", e);
            return Err(e.into());
        }
        if let Err(e) = self
            .session_manager
            .rewrite_messages(session_id, &session.messages)
            .await
        {
            tracing::error!("重写会话历史失败: {}", e);
            return Err(e.into());
        }

        // 回退工作区文件到该消息处理前的快照（配置了 working_directory 时生效；
        // 快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let Some(workdir) = self.resolve_workdir(&session) else {
                return self.get_messages(session_id).await;
            };
            match sm
                .with_workdir(workdir)
                .restore(session_id, request.message_index)
                .await
            {
                Ok(n) => {
                    info!(
                        "回退工作区文件: 会话={}, 索引={}, 恢复 {} 个文件",
                        session_id, request.message_index, n
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = %session_id,
                        index = request.message_index,
                        "工作区快照恢复失败（会话已回退，文件未回退）"
                    );
                }
            }
        }

        // 返回剩余消息，前端可直接替换本地状态
        self.get_messages(session_id).await
    }

    /// 重做：恢复被回退的消息与工作区文件。
    pub async fn redo_message(
        &self,
        session_id: &str,
        request: RedoRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!(
            "重做消息: 会话={}, 索引={}",
            session_id, request.message_index
        );

        // 0. 存在性检查：不存在的会话 → 404（优先于重做状态检查）；
        //    同时解析会话生效的工作目录（快照根，会话级绑定优先）
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;
        let workdir = self
            .resolve_workdir(&session)
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?;

        // 1. 恢复工作区文件 + 取出被截断的消息（快照管理器一次性语义）
        let Some((messages, restored)) = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?
            .with_workdir(workdir)
            .load_redo(session_id, request.message_index)
            .await?
        else {
            return Err(ApiError::BadRequest("没有可重做的状态".to_string()));
        };

        // 2. 恢复会话消息
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        for msg in messages {
            session.add_structured_message(msg);
        }
        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::error!("更新会话失败: {}", e);
            return Err(e.into());
        }
        if let Err(e) = self
            .session_manager
            .rewrite_messages(session_id, &session.messages)
            .await
        {
            tracing::error!("重写会话历史失败: {}", e);
            return Err(e.into());
        }

        tracing::info!(session = %session_id, restored, "重做完成");
        self.get_messages(session_id).await
    }

    /// 删除会话
    pub async fn delete_session(
        &self,
        session_id: &str,
    ) -> Result<DeleteSessionResponse, ApiError> {
        info!("删除会话: {}", session_id);

        // 存在性检查：不存在的会话删除 → 404（幂等语义，避免 500）
        let exists = self
            .session_manager
            .get_session(session_id)
            .await?
            .is_some();
        if !exists {
            return Err(ApiError::NotFound(format!("会话未找到: {}", session_id)));
        }

        self.session_manager.delete_session(session_id).await?;

        Ok(DeleteSessionResponse {
            success: true,
            message: format!("会话 {} 删除成功", session_id),
        })
    }

    /// 更新会话标题
    pub async fn update_title(
        &self,
        session_id: &str,
        request: UpdateTitleRequest,
    ) -> Result<Session, ApiError> {
        info!("更新会话标题: {} -> {}", session_id, request.title);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        session.title = Some(request.title);

        self.session_manager.update_session(&session).await?;

        Ok(Session {
            id: session.session_id,
            title: session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.ended_at.unwrap_or(session.created_at).to_rfc3339(),
            message_count: session.messages.len() as u32,
            working_directory: session.header.working_directory.clone(),
            metadata: Some(SessionMetadata {
                model: None,
                tags: None,
            }),
        })
    }

    /// 更新会话绑定的工作目录（工作区归属；空串清除绑定）。
    ///
    /// 目录必须存在，否则 400。
    pub async fn update_workspace(
        &self,
        session_id: &str,
        working_directory: &str,
    ) -> Result<Session, ApiError> {
        info!("更新会话工作目录: {} -> {}", session_id, working_directory);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        let trimmed = working_directory.trim();
        if trimmed.is_empty() {
            session.header.working_directory = None;
        } else {
            if !std::path::Path::new(trimmed).is_dir() {
                return Err(ApiError::BadRequest(format!("目录不存在：{trimmed}")));
            }
            session.header.working_directory = Some(trimmed.to_string());
        }

        self.session_manager.update_session(&session).await?;

        Ok(Session {
            id: session.session_id,
            title: session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.ended_at.unwrap_or(session.created_at).to_rfc3339(),
            message_count: session.messages.len() as u32,
            working_directory: session.header.working_directory.clone(),
            metadata: Some(SessionMetadata {
                model: None,
                tags: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_session_service_creation() {
        // 编译时测试
    }
}
