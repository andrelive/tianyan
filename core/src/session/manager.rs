//! 天演智能体系统的会话管理。
//!
//! 本模块提供会话创建、维护、关键信息提取和摘要生成功能。

use async_trait::async_trait;
use chrono::Utc;
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{
    DetailedTokenUsage, Message, MessageTime, Part, PartTime, StructuredMessage,
    TianyanUri,
};
use crate::vfs::VirtualFileSystem;

/// 每个会话加载的最大消息数量（保护内存和性能）
const MAX_SESSION_MESSAGES: usize = 100;

use super::types::Session;

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

    /// 直接持久化 StructuredMessage（不经过 Message 转换）。
    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()>;

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
            match serde_json::from_str::<StructuredMessage>(line) {
                Ok(msg) => {
                    session.add_structured_message(msg);
                }
                Err(e) => {
                    tracing::warn!("解析消息记录失败：{} - {}", uri, e);
                }
            }
        }

        // 从后向前扫描 compression_marker，只保留 marker 及之后的消息
        if let Some(marker_pos) = session.messages.iter().rposition(|m| m.compression_marker) {
            if marker_pos > 0 {
                let skipped = marker_pos;
                session.messages = session.messages.split_off(skipped);
                tracing::info!(
                    skipped_messages = skipped,
                    session_id = %id,
                    "按 compression_marker 截断会话至工作集"
                );
            }
        }

        // 限制加载的消息数量，保留最近的 MAX_SESSION_MESSAGES 条（安全上限）
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
    async fn append_message_to_vfs(&self, uri: &TianyanUri, msg: &StructuredMessage) -> Result<()> {
        let json_line = serde_json::to_string(msg)
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
        let now_ms = Utc::now().timestamp_millis();
        let sm = StructuredMessage {
            id: format!("msg_{}", now_ms),
            parent_id: None,
            role: message.role,
            parts: vec![Part::Text {
                text: message.content.clone(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now_ms,
                completed: now_ms,
            },
            session_id: id.to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new(id);
        session.add_structured_message(sm.clone());

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
        self.append_message_to_vfs(&uri, &sm).await?;

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

        let now_ms = Utc::now().timestamp_millis();
        let sm = StructuredMessage {
            id: format!("msg_{}", now_ms),
            parent_id: None,
            role: message.role,
            parts: vec![Part::Text {
                text: message.content.clone(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now_ms,
                completed: now_ms,
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: false,
        };

        // 追加消息到 VFS
        self.append_message_to_vfs(&uri, &sm).await?;

        Ok(())
    }

    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()> {
        use crate::ContentLevel;

        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id))
            .map_err(|e| TianyanError::MemorySystem(format!("无效的 session URI: {}", e)))?;

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

        self.append_message_to_vfs(&uri, &msg).await?;
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

    use crate::common::types::{ContentLevel, MessageRole};
    use crate::test_utils::MockVfs;
    use crate::vfs::{ContentStore, VfsCore};

    /// Helper to create a mock VFS with pre-existing session directory.
    async fn setup_session(vfs: &MockVfs, id: &str) {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", id)).unwrap();
        vfs.create_directory(&uri).await.unwrap();
    }

    /// Helper to write JSONL content to a session.
    async fn write_jsonl(vfs: &MockVfs, id: &str, jsonl: &str) {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", id)).unwrap();
        vfs.write(&uri, ContentLevel::Detail, jsonl).await.unwrap();
    }

    /// Helper to make a simple StructuredMessage.
    fn make_msg(id: &str, session_id: &str, role: MessageRole, text: &str, compression_marker: bool) -> StructuredMessage {
        let now = Utc::now().timestamp_millis();
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts: vec![Part::Text { text: text.to_string(), time: PartTime::default() }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime { created: now, completed: now },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker,
        }
    }

    #[tokio::test]
    async fn test_create_and_get_session_roundtrip() {
        let vfs = Arc::new(MockVfs::new());
        setup_session(&vfs, "test-1").await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        // Create session with first message
        let msg = Message::user("Hello");
        let session = mgr.create_session("test-1", msg).await.unwrap();

        assert_eq!(session.session_id, "test-1");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::User);

        // Verify persistence: read it back
        let loaded = mgr.get_session("test-1").await.unwrap().unwrap();
        assert_eq!(loaded.session_id, "test-1");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_session_returns_none() {
        let vfs = Arc::new(MockVfs::new());
        let mgr = PersistentSessionManager::new(vfs);

        let result = mgr.get_session("no-such").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_add_structured_message_persists() {
        let vfs = Arc::new(MockVfs::new());
        let id = "test-2";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        // First create session with user message
        let msg = Message::user("Hi");
        mgr.create_session(id, msg).await.unwrap();

        // Add assistant message via add_structured_message
        let sm = make_msg("assist-1", id, MessageRole::Assistant, "Hello there!", false);
        mgr.add_structured_message(id, sm).await.unwrap();

        // Load and verify both messages exist
        let session = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(session.messages.len(), 2, "should have 2 messages after adding");
        assert_eq!(session.messages[1].role, MessageRole::Assistant);
    }

    #[tokio::test]
    async fn test_compression_marker_truncation() {
        let vfs = Arc::new(MockVfs::new());
        let id = "test-3";
        setup_session(&vfs, id).await;

        // Write directly: 2 messages before marker, 1 marker, 2 after
        let msgs = vec![
            make_msg("m1", id, MessageRole::User, "old msg 1", false),
            make_msg("m2", id, MessageRole::Assistant, "old reply 1", false),
            make_msg("cmp", id, MessageRole::System, "summary", true),   // ← marker
            make_msg("m3", id, MessageRole::User, "recent 1", false),
            make_msg("m4", id, MessageRole::Assistant, "recent reply", false),
        ];
        let lines: Vec<String> = msgs.iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        write_jsonl(&vfs, id, &lines.join("\n")).await;

        let mgr = PersistentSessionManager::new(vfs);
        let session = mgr.get_session(id).await.unwrap().unwrap();

        // Should only have marker + messages after it = 3 messages
        assert_eq!(session.messages.len(), 3, "should skip pre-marker messages");
        assert!(session.messages[0].compression_marker, "first should be the marker");
        assert_eq!(session.messages[1].id, "m3");
        assert_eq!(session.messages[2].id, "m4");
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let vfs = Arc::new(MockVfs::new());
        let mgr = PersistentSessionManager::new(vfs.clone());

        // Create 2 sessions
        for id in &["s-a", "s-b"] {
            setup_session(&vfs, id).await;
            let msg = Message::user("start");
            mgr.create_session(id, msg).await.unwrap();
        }

        let sessions = mgr.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_session() {
        let vfs = Arc::new(MockVfs::new());
        let id = "test-del";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user("bye");
        mgr.create_session(id, msg).await.unwrap();

        mgr.delete_session(id).await.unwrap();
        let result = mgr.get_session(id).await.unwrap();
        assert!(result.is_none(), "session should be gone after delete");
    }
}
