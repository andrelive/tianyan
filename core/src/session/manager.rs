//! 天演智能体系统的会话管理。
//!
//! 本模块提供会话创建、维护、关键信息提取和摘要生成功能。

use async_trait::async_trait;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Message, TianyanUri};
use crate::storage::VirtualFileSystem;

/// 每个会话加载的最大消息数量（保护内存和性能）
const MAX_SESSION_MESSAGES: usize = 100;

use super::types::{MessageRecord, Session};

/// 会话管理操作 trait。
#[async_trait]
pub trait SessionManager: Send + Sync {
    /// 创建新会话并添加第一条消息。
    ///
    /// # 参数
    /// - `id`: 会话 ID
    /// - `message`: 第一条消息
    ///
    /// # 返回
    /// 创建后的会话
    async fn create_session(&self, id: &str, message: Message) -> Result<Session>;

    /// 通过 ID 获取会话。
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;

    /// 更新会话。
    async fn update_session(&self, session: &Session) -> Result<()>;

    /// 向会话添加消息。
    async fn add_message(&self, session_id: &str, message: Message) -> Result<()>;

    /// 列出所有会话。
    async fn list_sessions(&self) -> Result<Vec<Session>>;

    /// 删除会话。
    async fn delete_session(&self, id: &str) -> Result<()>;
}

/// 将会话持久化到虚拟文件系统的会话管理器。
///
/// 本实现直接基于 VFS 存储会话，按需加载，不维护内存缓存。
#[derive(Clone)]
pub struct PersistentSessionManager {
    vfs: Arc<dyn VirtualFileSystem>,
}

impl PersistentSessionManager {
    /// 创建新的持久化会话管理器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs }
    }

    /// 从 VFS 加载会话。
    ///
    /// 直接通过 session ID 构建 URI 并读取 JSONL 文件。
    async fn load_session_from_vfs(&self, id: &str) -> Result<Option<Session>> {
        use crate::ContentLevel;

        // 直接构建 URI 并读取
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", id))
            .map_err(|e| TianyanError::MemorySystem(format!("无效的 session URI: {}", e)))?;

        // 尝试读取 JSONL 内容
        let content = match self.vfs.read_content(&uri, ContentLevel::Detail).await {
            Ok(c) => c,
            Err(_) => return Ok(None), // 文件不存在或读取失败，返回 None
        };

        // 解析 JSONL 重建 Session
        let mut session = Session::new(id);
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<MessageRecord>(line) {
                Ok(record) => {
                    session.add_message(record.to_message());
                }
                Err(e) => {
                    tracing::warn!("解析消息记录失败：{} - {}", uri, e);
                }
            }
        }

        // 限制加载的消息数量，保留最近的 MAX_SESSION_MESSAGES 条
        if session.messages.len() > MAX_SESSION_MESSAGES {
            let skipped = session.messages.len() - MAX_SESSION_MESSAGES;
            session.messages = session.messages.split_off(skipped);
            tracing::info!(
                skipped_messages = skipped,
                session_id = %id,
                "会话消息超过上限，截断至最近 {} 条",
                MAX_SESSION_MESSAGES
            );
        }

        // 注意：元数据存储在向量库中，暂时不读取
        // 如果需要，可以通过 vector_storage().get_point() 获取

        Ok(Some(session))
    }

    /// 追加消息到 VFS。
    async fn append_message_to_vfs(&self, uri: &TianyanUri, message: &Message) -> Result<()> {
        let record = MessageRecord::from_message(message);
        let json_line = serde_json::to_string(&record)
            .map_err(|e| TianyanError::Serialization(e.to_string()))?;
        let jsonl_line = format!("{}\n", json_line);
        self.vfs.append_content(uri, &jsonl_line).await?;
        Ok(())
    }

    /// 更新会话元数据。
    async fn update_session_metadata(&self, session: &Session) -> Result<()> {
        let custom = self.build_session_custom(session);
        self.vfs
            .update_metadata(&session.uri(), 0.5, custom)
            .await?;
        Ok(())
    }

    /// 构建 Session 的 custom 元数据。
    fn build_session_custom(&self, session: &Session) -> HashMap<String, serde_json::Value> {
        let mut custom = HashMap::new();
        custom.insert(
            "session_id".to_string(),
            serde_json::json!(session.session_id),
        );
        custom.insert("ended_at".to_string(), serde_json::json!(session.ended_at));
        custom.insert("title".to_string(), serde_json::json!(session.title));
        custom.insert(
            "message_count".to_string(),
            serde_json::json!(session.messages.len()),
        );
        custom
    }
}

#[async_trait]
impl SessionManager for PersistentSessionManager {
    async fn create_session(&self, id: &str, message: Message) -> Result<Session> {
        let mut session = Session::new(id);
        session.add_message(message.clone());

        let uri = session.uri();

        // 先检查会话是否已存在，避免重复创建
        if self.get_session(id).await?.is_some() {
            return Err(TianyanError::MemorySystem(format!("会话已存在：{}", id)));
        }

        // 创建目录
        self.vfs
            .create_directory(&uri)
            .await
            .map_err(|e| TianyanError::MemorySystem(format!("会话创建失败：{} ({})", id, e)))?;

        // 追加第一条消息到 VFS
        self.append_message_to_vfs(&uri, &message).await?;

        // 更新元数据
        self.update_session_metadata(&session).await?;

        Ok(session)
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.load_session_from_vfs(id).await
    }

    async fn update_session(&self, session: &Session) -> Result<()> {
        // 检查会话是否存在
        if self.get_session(&session.session_id).await?.is_none() {
            return Err(TianyanError::MemorySystem(format!(
                "会话未找到：{}",
                session.session_id
            )));
        }

        // 更新元数据
        self.update_session_metadata(session).await?;

        Ok(())
    }

    async fn add_message(&self, session_id: &str, message: Message) -> Result<()> {
        use crate::ContentLevel;

        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id))
            .map_err(|e| TianyanError::MemorySystem(format!("无效的 session URI: {}", e)))?;

        // 检查会话是否存在（轻量检查）
        if self
            .vfs
            .read_content(&uri, ContentLevel::Detail)
            .await
            .is_err()
        {
            return Err(TianyanError::MemorySystem(format!(
                "会话未找到：{}",
                session_id
            )));
        }

        // 追加消息到 VFS
        self.append_message_to_vfs(&uri, &message).await?;

        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<Session>> {
        use crate::common::types::ContextNamespace;

        let session_ns = TianyanUri::new(ContextNamespace::Session, vec![]);

        let entries = match self.vfs.list(&session_ns).await {
            Ok(entries) => entries,
            Err(_) => return Ok(Vec::new()),
        };

        let mut sessions = Vec::new();
        for entry in entries {
            if let Some(session_id) = entry.uri().path().last().cloned() {
                if let Some(session) = self.get_session(&session_id).await? {
                    sessions.push(session);
                }
            }
        }

        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(sessions)
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        let session = self
            .get_session(id)
            .await?
            .ok_or_else(|| TianyanError::MemorySystem(format!("会话未找到：{}", id)))?;

        self.vfs.delete(&session.uri()).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注意：这些测试需要 Mock VFS 实现
    // 暂时只保留编译测试

    #[test]
    fn test_persistent_session_manager_creation() {
        // 编译时测试
    }
}
