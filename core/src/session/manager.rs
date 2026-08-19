//! 天演智能体系统的会话管理。
//!
//! 本模块提供会话创建、维护、关键信息提取和摘要生成功能。

use async_trait::async_trait;
use serde_json;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{ContentLevel, Message, MessageRole, StructuredMessage, TianyanUri};
use crate::vfs::VirtualFileSystem;

use super::search::SessionRecall;
use super::types::parse_message_lines;
use super::types::SessionHeader;
use super::{types::Session, MAX_SESSION_MESSAGES};

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

    /// 直接持久化 StructuredMessage（不经过 Message 转换）。
    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()>;

    /// 全量重写会话消息（用于编辑/截断后持久化，覆盖 JSONL 内容）。
    ///
    /// 会话必须已存在。传入空切片将清空会话历史。
    async fn rewrite_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()>;

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
    /// 会话消息索引（ADR-017 决策 6：FTS5 回忆；None 时不索引）。
    recall: Option<Arc<SessionRecall>>,
}

impl PersistentSessionManager {
    /// 创建新的持久化会话管理器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs, recall: None }
    }

    /// 设置会话消息索引（append 时同步索引；ADR-017 决策 6）。
    pub fn with_recall(mut self, recall: Arc<SessionRecall>) -> Self {
        self.recall = Some(recall);
        self
    }

    /// 从 VFS 加载会话。
    ///
    /// 直接通过 session ID 构建 URI 并读取 JSONL 文件。
    async fn load_session_from_vfs(&self, id: &str) -> Result<Option<Session>> {
        // 直接构建 URI 并读取
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", id)).map_err(|e| {
            TianyanError::Custom(format!("会话存储错误：无效的 session URI: {}", e))
        })?;

        // 尝试读取 JSONL 内容
        let content = match self.vfs.read_content(&uri, ContentLevel::Detail).await {
            Ok(c) => c,
            Err(_) => return Ok(None), // 文件不存在或读取失败，返回 None
        };

        // 解析 JSONL 重建 Session
        let mut session = Session::new(id);
        let mut lines = content.lines();
        // 首行可能是会话头部（SessionHeader：injectable 快照 + 会话元数据）；
        // 旧格式会话（首行即消息）兼容跳过
        if let Some(first) = lines.next() {
            if !first.trim().is_empty() {
                match SessionHeader::parse_line(first) {
                    Some(header) => {
                        // 从头部恢复会话元数据（title/ended_at 用户可见；
                        // created_at 旧会话缺省时保持 Session::new 的加载时刻）
                        if let Some(created) = header.created_at {
                            session.created_at = created;
                        }
                        session.title = header.title.clone();
                        session.ended_at = header.ended_at;
                        session.header = header;
                    }
                    None => match serde_json::from_str::<StructuredMessage>(first) {
                        Ok(msg) => session.add_structured_message(msg),
                        Err(e) => {
                            tracing::warn!("解析消息记录失败：{} - {}", uri, e);
                        }
                    },
                }
            }
        }
        // 其余行统一走共享 JSONL 解析（格式知识收敛点
        // [`parse_message_lines`]：跳过空行与解析失败行）
        let rest: String = lines.collect::<Vec<_>>().join("\n");
        let parsed = parse_message_lines(&rest);
        let non_empty_lines = rest.lines().filter(|l| !l.trim().is_empty()).count();
        if non_empty_lines != parsed.len() {
            tracing::warn!(
                skipped = non_empty_lines - parsed.len(),
                session_id = %id,
                "会话消息解析失败行数"
            );
        }
        for msg in parsed {
            session.add_structured_message(msg);
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

        // 限制加载的消息数量，保留最近的 MAX_SESSION_MESSAGES 条（安全上限）。
        // 截断优先丢弃 assistant/tool/system——**用户消息是上下文锚点，
        // 必须保留**（否则切回会话找不到自己的输入）；仅当用户消息本身
        // 超限时才从最旧处丢弃 user 消息。
        if session.messages.len() > MAX_SESSION_MESSAGES {
            let mut skipped = 0usize;
            while session.messages.len() > MAX_SESSION_MESSAGES {
                let over = session.messages.len() - MAX_SESSION_MESSAGES;
                // 第一遍：从最旧开始丢弃非 user 消息
                let dropped = session
                    .messages
                    .iter()
                    .take(over)
                    .take_while(|m| m.role != MessageRole::User)
                    .count();
                if dropped == 0 {
                    // 无可丢弃的非 user 消息（user 消息本身超限）：
                    // 退化为从最旧丢弃 user 消息
                    session.messages.remove(0);
                    skipped += 1;
                    continue;
                }
                session.messages.drain(..dropped);
                skipped += dropped;
            }
            tracing::info!(
                skipped_messages = skipped,
                session_id = %id,
                "会话消息超过上限，截断至最近 {} 条（保留用户消息）",
                session.messages.len(),
            );
        }

        Ok(Some(session))
    }

    /// 追加消息到 VFS（ADR-017 决策 6：装配消息索引时同步索引，零 LLM）。
    async fn append_message_to_vfs(&self, uri: &TianyanUri, msg: &StructuredMessage) -> Result<()> {
        let json_line = serde_json::to_string(msg)
            .map_err(|e| TianyanError::Custom(format!("序列化错误：{}", e)))?;
        let jsonl_line = format!("{}\n", json_line);
        // seq = 追加前消息数（JSONL 行号语义；头部行不计入）
        let seq = if self.recall.is_some() {
            let existing = self
                .vfs
                .read_content(uri, ContentLevel::Detail)
                .await
                .unwrap_or_default();
            parse_message_lines(&existing).len() as i64
        } else {
            0
        };
        self.vfs.append_content(uri, &jsonl_line).await?;
        if let Some(ref recall) = self.recall {
            let session_id = uri.path().last().cloned().unwrap_or_default();
            if let Err(e) = recall.index_message(&session_id, seq, msg).await {
                tracing::debug!(
                    session_id = %session_id,
                    error = %e,
                    "会话消息索引失败（回忆检索跳过该消息）"
                );
            }
        }
        Ok(())
    }

    /// 将会话头部（injectable 快照 + 会话元数据）写回 JSONL 首行。
    ///
    /// 已有头部则替换，否则前置插入（旧格式会话首次更新时补齐）。
    /// 低频操作（标题更新 / 快照固化 / 压缩点清空），全量读写可接受。
    async fn write_session_header(&self, session_id: &str, header: &SessionHeader) -> Result<()> {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id)).map_err(|e| {
            TianyanError::Custom(format!("会话存储错误：无效的 session URI: {}", e))
        })?;

        let existing = self
            .vfs
            .read_content(&uri, ContentLevel::Detail)
            .await
            .unwrap_or_default();
        let header_line = serde_json::to_string(header)
            .map_err(|e| TianyanError::Custom(format!("序列化错误：{}", e)))?;

        let mut lines: Vec<&str> = existing.lines().collect();
        let first_is_header = lines
            .first()
            .and_then(|l| SessionHeader::parse_line(l))
            .is_some();

        let mut content = String::new();
        content.push_str(&header_line);
        content.push('\n');
        if first_is_header && !lines.is_empty() {
            lines.remove(0);
        }
        for line in lines {
            content.push_str(line);
            content.push('\n');
        }
        self.vfs.write_content(&uri, &content).await?;
        Ok(())
    }
}

#[async_trait]
impl SessionManager for PersistentSessionManager {
    async fn create_session(&self, id: &str, message: Message) -> Result<Session> {
        // 全保真转换（StructuredMessage::from_message：图片等非文本内容不丢失）
        let sm = StructuredMessage::from_message(&message, id, None, None);
        let mut session = Session::new(id);
        session.add_structured_message(sm.clone());

        let uri = session.uri();

        // 先检查会话是否已存在，避免重复创建
        if self.get_session(id).await?.is_some() {
            return Err(TianyanError::conflict(format!(
                "会话存储错误：会话已存在：{}",
                id
            )));
        }

        // 创建目录
        self.vfs.create_directory(&uri).await.map_err(|e| {
            TianyanError::Custom(format!("会话存储错误：会话创建失败：{} ({})", id, e))
        })?;

        // 写入会话头部（会话级状态：injectable 快照 + created_at 等元数据）
        // 与第一条消息
        let header = SessionHeader {
            created_at: Some(session.created_at),
            ..SessionHeader::default()
        };
        let header_line = serde_json::to_string(&header)
            .map_err(|e| TianyanError::Custom(format!("序列化错误：{}", e)))?;
        self.vfs
            .append_content(&uri, &format!("{header_line}\n"))
            .await?;
        self.append_message_to_vfs(&uri, &sm).await?;

        Ok(session)
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.load_session_from_vfs(id).await
    }

    async fn update_session(&self, session: &Session) -> Result<()> {
        // 检查会话是否存在
        if self.get_session(&session.session_id).await?.is_none() {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{}",
                session.session_id
            )));
        }

        // 同步会话级元数据到头部（title/ended_at 用户可见，唯一持久化 home；
        // created_at 新会话由 create_session 固化，旧会话首次更新时冻结当前值）
        let mut header = session.header.clone();
        header.created_at = Some(session.created_at);
        header.title = session.title.clone();
        header.ended_at = session.ended_at;
        self.write_session_header(&session.session_id, &header)
            .await?;

        Ok(())
    }

    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()> {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id)).map_err(|e| {
            TianyanError::Custom(format!("会话存储错误：无效的 session URI: {}", e))
        })?;

        self.append_message_to_vfs(&uri, &msg).await?;
        Ok(())
    }

    async fn rewrite_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()> {
        let uri = TianyanUri::parse(&format!("tianyan://session/{}", session_id)).map_err(|e| {
            TianyanError::Custom(format!("会话存储错误：无效的 session URI: {}", e))
        })?;

        // 会话必须已存在
        if !self.vfs.exists(&uri).await? {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{}",
                session_id
            )));
        }

        // 保留现有会话头部（injectable 快照等会话级状态）
        let mut header = SessionHeader::default();
        if let Ok(existing) = self.vfs.read_content(&uri, ContentLevel::Detail).await {
            if let Some(first) = existing.lines().next() {
                if let Some(h) = SessionHeader::parse_line(first) {
                    header = h;
                }
            }
        }

        // 序列化全部消息为 JSONL 并全量覆写 Detail 层级（首行恒为头部）
        let mut content = String::new();
        let header_line = serde_json::to_string(&header)
            .map_err(|e| TianyanError::Custom(format!("序列化错误：{}", e)))?;
        content.push_str(&header_line);
        content.push('\n');
        for msg in messages {
            let json_line = serde_json::to_string(msg)
                .map_err(|e| TianyanError::Custom(format!("序列化错误：{}", e)))?;
            content.push_str(&json_line);
            content.push('\n');
        }
        self.vfs.write_content(&uri, &content).await?;
        // ADR-017 决策 6：全量覆写后重建消息索引（幂等）
        if let Some(ref recall) = self.recall {
            if let Err(e) = recall.rebuild_session(session_id, &content).await {
                tracing::debug!(
                    session_id = %session_id,
                    error = %e,
                    "会话消息索引重建失败（回忆检索可能过期）"
                );
            }
        }
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
            // 会话是 Session 命名空间下的目录条目；旧版记忆提取水位线
            // （`_metadata` 子文件）等非目录条目不是会话，跳过防幽灵会话
            if !entry.is_directory() {
                continue;
            }
            if let Some(session_id) = entry.uri().path().last().cloned() {
                if let Some(session) = self.get_session(&session_id).await? {
                    sessions.push(session);
                }
            }
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));

        Ok(sessions)
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        let session = self
            .get_session(id)
            .await?
            .ok_or_else(|| TianyanError::not_found(format!("会话存储错误：会话未找到：{}", id)))?;

        self.vfs.delete(&session.uri()).await?;
        // ADR-017 决策 6：删除会话消息索引
        if let Some(ref recall) = self.recall {
            if let Err(e) = recall.delete_session(id).await {
                tracing::debug!(session_id = %id, error = %e, "会话消息索引删除失败");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::types::{
        ContentLevel, ContextNamespace, DetailedTokenUsage, InjectableContext, MessageRole,
        MessageTime, Part, PartTime,
    };
    use crate::test_utils::MockVfs;
    use crate::vfs::{ContentStore, VfsCore};
    use chrono::Utc;

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
    fn make_msg(
        id: &str,
        session_id: &str,
        role: MessageRole,
        text: &str,
        compression_marker: bool,
    ) -> StructuredMessage {
        let now = Utc::now().timestamp_millis();
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts: vec![Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: now,
                completed: now,
            },
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
    async fn test_create_session_persists_image_parts() {
        // 回归保护：create_session 曾用 to_structured_message（仅写 Part::Text，
        // 静默丢弃 content_parts 图片）；修复后走 StructuredMessage::from_message
        // 全保真路径，首条消息的图片必须持久化为 Part::Image。
        let vfs = Arc::new(MockVfs::new());
        let id = "test-img";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user_with_images("看图", vec!["data:image/png;base64,AAAA".to_string()]);
        mgr.create_session(id, msg).await.unwrap();

        let loaded = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 1);
        let parts = &loaded.messages[0].parts;
        assert!(
            parts.iter().any(|p| matches!(p, Part::Image { .. })),
            "首条消息的图片应持久化为 Part::Image，实际 parts: {:?}",
            parts
        );
        assert!(
            parts.iter().any(|p| matches!(p, Part::Text { .. })),
            "文本 part 应保留，实际 parts: {:?}",
            parts
        );
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
        let sm = make_msg(
            "assist-1",
            id,
            MessageRole::Assistant,
            "Hello there!",
            false,
        );
        mgr.add_structured_message(id, sm).await.unwrap();

        // Load and verify both messages exist
        let session = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(
            session.messages.len(),
            2,
            "should have 2 messages after adding"
        );
        assert_eq!(session.messages[1].role, MessageRole::Assistant);
    }

    #[tokio::test]
    async fn test_compression_marker_truncation() {
        let vfs = Arc::new(MockVfs::new());
        let id = "test-3";
        setup_session(&vfs, id).await;

        // Write directly: 2 messages before marker, 1 marker, 2 after
        let msgs = [
            make_msg("m1", id, MessageRole::User, "old msg 1", false),
            make_msg("m2", id, MessageRole::Assistant, "old reply 1", false),
            make_msg("cmp", id, MessageRole::System, "summary", true), // ← marker
            make_msg("m3", id, MessageRole::User, "recent 1", false),
            make_msg("m4", id, MessageRole::Assistant, "recent reply", false),
        ];
        let lines: Vec<String> = msgs
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        write_jsonl(&vfs, id, &lines.join("\n")).await;

        let mgr = PersistentSessionManager::new(vfs);
        let session = mgr.get_session(id).await.unwrap().unwrap();

        // Should only have marker + messages after it = 3 messages
        assert_eq!(session.messages.len(), 3, "should skip pre-marker messages");
        assert!(
            session.messages[0].compression_marker,
            "first should be the marker"
        );
        assert_eq!(session.messages[1].id, "m3");
        assert_eq!(session.messages[2].id, "m4");
    }

    #[tokio::test]
    #[ignore = "MockVfs::list_directory does not track sessions created via create_session — needs enhanced mock or temp VFS"]
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

    #[tokio::test]
    async fn test_injectable_snapshot_roundtrip() {
        // 回归保护：注入上下文快照随会话持久化（JSONL 首行 SessionHeader）——
        // 写入后重启（重新 load）应恢复同一份前缀内容，不重新检索。
        let vfs = Arc::new(MockVfs::new());
        let id = "test-snap";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user("Hello");
        mgr.create_session(id, msg).await.unwrap();

        let mut session = mgr.get_session(id).await.unwrap().unwrap();
        assert!(
            session.header.injectable_snapshot.is_none(),
            "新会话不应有快照"
        );
        session.header.injectable_snapshot = Some(InjectableContext {
            soul: "测试人格".to_string(),
            rules_and_experiences: vec!["先读后写".to_string()],
            ..Default::default()
        });
        mgr.update_session(&session).await.unwrap();

        // 模拟重启：重新加载
        let loaded = mgr.get_session(id).await.unwrap().unwrap();
        let snap = loaded.header.injectable_snapshot.unwrap();
        assert_eq!(snap.soul, "测试人格");
        assert_eq!(snap.rules_and_experiences, vec!["先读后写".to_string()]);
        // 消息不丢失
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_rewrite_messages_preserves_header() {
        // 回归保护：全量重写消息（会话编辑/清空）必须保留会话头部快照。
        let vfs = Arc::new(MockVfs::new());
        let id = "test-rewrite";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user("Hello");
        mgr.create_session(id, msg).await.unwrap();

        let mut session = mgr.get_session(id).await.unwrap().unwrap();
        session.header.injectable_snapshot = Some(InjectableContext {
            soul: "快照人格".to_string(),
            ..Default::default()
        });
        mgr.update_session(&session).await.unwrap();

        // 全量重写消息
        let new_msgs = vec![make_msg("n1", id, MessageRole::User, "新的消息", false)];
        mgr.rewrite_messages(id, &new_msgs).await.unwrap();

        let loaded = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(
            loaded.header.injectable_snapshot.unwrap().soul,
            "快照人格",
            "重写消息不得丢失会话头部快照"
        );
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].id, "n1");
    }

    #[tokio::test]
    async fn test_load_legacy_jsonl_without_header() {
        // 回归保护：旧格式会话（首行即消息，无 SessionHeader）兼容加载，
        // header 保持默认空值，消息完整解析。
        let vfs = Arc::new(MockVfs::new());
        let id = "test-legacy";
        setup_session(&vfs, id).await;

        let msgs = [
            make_msg("m1", id, MessageRole::User, "旧消息 1", false),
            make_msg("m2", id, MessageRole::Assistant, "旧回复", false),
        ];
        let lines: Vec<String> = msgs
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect();
        write_jsonl(&vfs, id, &lines.join("\n")).await;

        let mgr = PersistentSessionManager::new(vfs);
        let session = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(session.messages.len(), 2, "旧格式消息应完整加载");
        assert!(
            session.header.injectable_snapshot.is_none(),
            "旧格式无快照，header 应为空"
        );
    }

    #[tokio::test]
    async fn test_session_metadata_roundtrip() {
        // 回归保护：会话元数据（created_at / title / ended_at）持久化于 JSONL 首行
        // SessionHeader——重启（重新 load）后恢复，不再只写不读。
        let vfs = Arc::new(MockVfs::new());
        let id = "test-meta";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user("Hello");
        mgr.create_session(id, msg).await.unwrap();
        let original_created_at = mgr.get_session(id).await.unwrap().unwrap().created_at;

        let mut session = mgr.get_session(id).await.unwrap().unwrap();
        session.title = Some("我的标题".to_string());
        session.end();
        mgr.update_session(&session).await.unwrap();

        // 模拟重启：重新加载
        let loaded = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(
            loaded.title.as_deref(),
            Some("我的标题"),
            "标题应持久化恢复"
        );
        assert_eq!(
            loaded.created_at, original_created_at,
            "created_at 不应随加载漂移（旧实现每次加载取 Utc::now()）"
        );
        assert_eq!(loaded.ended_at, session.ended_at, "ended_at 应持久化恢复");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_list_sessions_skips_non_directory_entries() {
        // 回归保护：Session 命名空间下的非目录条目（旧版 `_metadata` 水位线）
        // 不得被当作会话列出（幽灵会话）。
        let vfs = Arc::new(MockVfs::new());
        let session_ns = TianyanUri::new(ContextNamespace::Session, vec![]);

        // 会话目录 + JSONL 内容（头部含 created_at + 一条消息）
        let session_uri = TianyanUri::parse("tianyan://session/s-a").unwrap();
        vfs.add_directory(&session_ns, &session_uri);
        let header = SessionHeader {
            created_at: Some(Utc::now()),
            ..SessionHeader::default()
        };
        let msg = make_msg("m1", "s-a", MessageRole::User, "hi", false);
        let jsonl = format!(
            "{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&msg).unwrap()
        );
        vfs.set_content(&session_uri, ContentLevel::Detail, &jsonl);

        // 幽灵条目：旧版记忆提取水位线文件（修复前会被当作空会话列出）
        let metadata_uri = TianyanUri::parse("tianyan://session/_metadata").unwrap();
        vfs.add_entry(&session_ns, &metadata_uri);
        vfs.set_content(
            &metadata_uri,
            ContentLevel::Detail,
            r#"{"last_extracted_message_count":1,"last_extracted_at":"2026-01-01T00:00:00Z"}"#,
        );

        let mgr = PersistentSessionManager::new(vfs);
        let sessions = mgr.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1, "非目录条目不得作为会话列出");
        assert_eq!(sessions[0].session_id, "s-a");
    }

    // ── ADR-014 错误分类 ─────────────────────────────────────────

    #[tokio::test]
    async fn test_create_duplicate_session_is_conflict() {
        let vfs = Arc::new(MockVfs::new());
        let id = "dup-1";
        setup_session(&vfs, id).await;
        let mgr = PersistentSessionManager::new(vfs.clone());

        let msg = Message::user("Hello");
        mgr.create_session(id, msg.clone()).await.unwrap();

        let err = mgr.create_session(id, msg).await.unwrap_err();
        assert!(err.is_conflict(), "重复创建应分类为冲突：{err}");
    }

    #[tokio::test]
    async fn test_update_missing_session_is_not_found() {
        let vfs = Arc::new(MockVfs::new());
        let mgr = PersistentSessionManager::new(vfs);
        let session = Session::new("no-such");

        let err = mgr.update_session(&session).await.unwrap_err();
        assert!(err.is_not_found(), "更新缺失会话应分类为未找到：{err}");
    }

    #[tokio::test]
    async fn test_delete_missing_session_is_not_found() {
        let vfs = Arc::new(MockVfs::new());
        let mgr = PersistentSessionManager::new(vfs);

        let err = mgr.delete_session("no-such").await.unwrap_err();
        assert!(err.is_not_found(), "删除缺失会话应分类为未找到：{err}");
    }

    #[tokio::test]
    async fn test_rewrite_messages_missing_session_is_not_found() {
        let vfs = Arc::new(MockVfs::new());
        let mgr = PersistentSessionManager::new(vfs);

        let err = mgr.rewrite_messages("no-such", &[]).await.unwrap_err();
        assert!(err.is_not_found(), "重写缺失会话应分类为未找到：{err}");
    }
}
