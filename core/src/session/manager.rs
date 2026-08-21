//! 天演智能体系统的会话管理。
//!
//! 持久化基于会话权威存储（ADR-018：`SessionStore`，SQLite 表为唯一真相，
//! 不经 VFS）。本模块负责会话业务语义：创建/加载（含 compression_marker
//! 截断与消息上限兜底）、元数据同步、全量重写、列表与删除。

use async_trait::async_trait;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::common::types::{Message, MessageRole, StructuredMessage};

use super::store::SessionStore;
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

    /// 全量重写会话消息（用于编辑/截断后持久化）。
    ///
    /// 会话必须已存在。传入空切片将清空会话历史。
    async fn rewrite_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()>;

    /// 列出所有会话（轻量元数据，不加载消息；message_count 预填）。
    async fn list_sessions(&self) -> Result<Vec<Session>>;

    /// 删除会话。
    async fn delete_session(&self, id: &str) -> Result<()>;
}

/// 基于会话权威存储（SQLite）的持久化会话管理器。
///
/// 按需加载，不维护内存缓存。
#[derive(Clone)]
pub struct PersistentSessionManager {
    store: Arc<SessionStore>,
}

impl PersistentSessionManager {
    /// 创建持久化会话管理器。
    pub fn new(store: Arc<SessionStore>) -> Self {
        Self { store }
    }

    /// 从会话权威存储加载会话。
    async fn load_session_from_store(&self, id: &str) -> Result<Option<Session>> {
        let Some((header, messages)) = self.store.load(id).await? else {
            return Ok(None);
        };

        let mut session = Session::new(id);
        // 从头部恢复会话元数据（title/ended_at 用户可见；created_at 缺省时
        // 保持 Session::new 的加载时刻）
        if let Some(created) = header.created_at {
            session.created_at = created;
        }
        session.title = header.title.clone();
        session.ended_at = header.ended_at;
        session.header = header;
        session.message_count = Some(messages.len());
        for msg in messages {
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
}

#[async_trait]
impl SessionManager for PersistentSessionManager {
    async fn create_session(&self, id: &str, message: Message) -> Result<Session> {
        // 全保真转换（StructuredMessage::from_message：图片等非文本内容不丢失）
        let sm = StructuredMessage::from_message(&message, id, None, None);
        let mut session = Session::new(id);
        session.add_structured_message(sm.clone());

        // 先检查会话是否已存在，避免重复创建
        if self.store.exists(id).await? {
            return Err(TianyanError::conflict(format!(
                "会话存储错误：会话已存在：{}",
                id
            )));
        }

        // 会话级状态（created_at 固化；title/ended_at 初始为空）
        let header = SessionHeader {
            created_at: Some(session.created_at),
            ..SessionHeader::default()
        };
        self.store.create(id, &header).await?;
        self.store.append_message(id, &sm).await?;

        Ok(session)
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.load_session_from_store(id).await
    }

    async fn update_session(&self, session: &Session) -> Result<()> {
        // 检查会话是否存在
        if !self.store.exists(&session.session_id).await? {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{}",
                session.session_id
            )));
        }

        // 同步会话级元数据（title/ended_at 用户可见，唯一持久化 home；
        // created_at 新会话由 create_session 固化，旧会话首次更新时冻结当前值）
        let mut header = session.header.clone();
        header.created_at = Some(session.created_at);
        header.title = session.title.clone();
        header.ended_at = session.ended_at;
        self.store
            .update_header(&session.session_id, &header)
            .await?;

        Ok(())
    }

    async fn add_structured_message(&self, session_id: &str, msg: StructuredMessage) -> Result<()> {
        self.store.append_message(session_id, &msg).await?;
        Ok(())
    }

    async fn rewrite_messages(
        &self,
        session_id: &str,
        messages: &[StructuredMessage],
    ) -> Result<()> {
        // 会话必须已存在
        if !self.store.exists(session_id).await? {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{}",
                session_id
            )));
        }

        // 保留现有会话头部（injectable 快照等会话级状态）
        let header = self.store.header(session_id).await?.unwrap_or_default();

        self.store.rewrite(session_id, &header, messages).await?;
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<Session>> {
        let metas = self.store.list_meta().await?;
        let mut sessions = Vec::with_capacity(metas.len());
        for meta in metas {
            let mut session = Session::new(meta.session_id);
            session.created_at = meta.header.created_at.unwrap_or_else(|| session.created_at);
            session.title = meta.header.title.clone();
            session.ended_at = meta.header.ended_at;
            session.header = meta.header;
            session.message_count = Some(meta.message_count);
            sessions.push(session);
        }
        Ok(sessions)
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        if !self.store.exists(id).await? {
            return Err(TianyanError::not_found(format!(
                "会话存储错误：会话未找到：{}",
                id
            )));
        }
        self.store.delete(id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::types::{
        DetailedTokenUsage, InjectableContext, MessageRole, MessageTime, Part, PartTime,
    };
    use crate::vfs::backend::sqlite_db::SqliteDb;
    use chrono::Utc;

    /// Helper：内存库 + 会话管理器。
    async fn make_manager() -> Arc<PersistentSessionManager> {
        let db = SqliteDb::open_in_memory().unwrap();
        db.init_all_schemas().await.unwrap();
        let store = crate::session::store::SessionStore::new(db).unwrap();
        Arc::new(PersistentSessionManager::new(store))
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
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            });
        }
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role,
            parts,
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
        let mgr = make_manager().await;

        let msg = Message::user("Hello");
        let session = mgr.create_session("test-1", msg).await.unwrap();

        assert_eq!(session.session_id, "test-1");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::User);

        let loaded = mgr.get_session("test-1").await.unwrap().unwrap();
        assert_eq!(loaded.session_id, "test-1");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_create_session_persists_image_parts() {
        // 回归保护：create_session 曾用 to_structured_message（仅写 Part::Text，
        // 静默丢弃 content_parts 图片）；修复后走 StructuredMessage::from_message
        // 全保真路径，首条消息的图片必须持久化为 Part::Image。
        let mgr = make_manager().await;
        let id = "test-img";

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
        let mgr = make_manager().await;
        let result = mgr.get_session("no-such").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_add_structured_message_persists() {
        let mgr = make_manager().await;
        let id = "test-2";

        let msg = Message::user("Hi");
        mgr.create_session(id, msg).await.unwrap();

        let sm = make_msg(
            "assist-1",
            id,
            MessageRole::Assistant,
            "Hello there!",
            false,
        );
        mgr.add_structured_message(id, sm).await.unwrap();

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
        let mgr = make_manager().await;
        let id = "test-3";

        // 构造：marker 前 2 条 + marker + 后 2 条
        mgr.create_session(id, Message::user("m0")).await.unwrap();
        mgr.add_structured_message(
            id,
            make_msg("m1", id, MessageRole::User, "old msg 1", false),
        )
        .await
        .unwrap();
        mgr.add_structured_message(
            id,
            make_msg("m2", id, MessageRole::Assistant, "old reply 1", false),
        )
        .await
        .unwrap();
        mgr.add_structured_message(
            id,
            make_msg("cmp", id, MessageRole::System, "summary", true),
        )
        .await
        .unwrap();
        mgr.add_structured_message(id, make_msg("m3", id, MessageRole::User, "recent 1", false))
            .await
            .unwrap();
        mgr.add_structured_message(
            id,
            make_msg("m4", id, MessageRole::Assistant, "recent reply", false),
        )
        .await
        .unwrap();

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
    async fn test_list_sessions() {
        let mgr = make_manager().await;

        for id in &["s-a", "s-b"] {
            mgr.create_session(id, Message::user("start"))
                .await
                .unwrap();
        }
        // s-b 追加一条，验证 message_count
        mgr.add_structured_message(
            "s-b",
            make_msg("m2", "s-b", MessageRole::Assistant, "reply", false),
        )
        .await
        .unwrap();

        let sessions = mgr.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        let b = sessions.iter().find(|s| s.session_id == "s-b").unwrap();
        assert_eq!(b.message_count, Some(2), "轻量列表应预填消息数");
        assert!(b.messages.is_empty(), "轻量列表不加载消息");
    }

    #[tokio::test]
    async fn test_delete_session() {
        let mgr = make_manager().await;
        let id = "test-del";

        mgr.create_session(id, Message::user("bye")).await.unwrap();

        mgr.delete_session(id).await.unwrap();
        let result = mgr.get_session(id).await.unwrap();
        assert!(result.is_none(), "session should be gone after delete");
    }

    #[tokio::test]
    async fn test_injectable_snapshot_roundtrip() {
        // 回归保护：注入上下文快照随会话持久化（session_meta.header_json）——
        // 写入后重启（重新 load）应恢复同一份前缀内容，不重新检索。
        let mgr = make_manager().await;
        let id = "test-snap";

        mgr.create_session(id, Message::user("Hello"))
            .await
            .unwrap();

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
        let mgr = make_manager().await;
        let id = "test-rewrite";

        mgr.create_session(id, Message::user("Hello"))
            .await
            .unwrap();

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
    async fn test_session_metadata_roundtrip() {
        // 回归保护：会话元数据（created_at / title / ended_at）持久化于
        // session_meta.header_json——重启（重新 load）后恢复，不再只写不读。
        let mgr = make_manager().await;
        let id = "test-meta";

        mgr.create_session(id, Message::user("Hello"))
            .await
            .unwrap();
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
            "created_at 不应随加载漂移"
        );
        assert_eq!(loaded.ended_at, session.ended_at, "ended_at 应持久化恢复");
        assert_eq!(loaded.messages.len(), 1);
    }

    // ── ADR-014 错误分类 ─────────────────────────────────────────

    #[tokio::test]
    async fn test_create_duplicate_session_is_conflict() {
        let mgr = make_manager().await;
        let id = "dup-1";

        let msg = Message::user("Hello");
        mgr.create_session(id, msg.clone()).await.unwrap();

        let err = mgr.create_session(id, msg).await.unwrap_err();
        assert!(err.is_conflict(), "重复创建应分类为冲突：{err}");
    }

    #[tokio::test]
    async fn test_update_missing_session_is_not_found() {
        let mgr = make_manager().await;
        let session = Session::new("no-such");

        let err = mgr.update_session(&session).await.unwrap_err();
        assert!(err.is_not_found(), "更新缺失会话应分类为未找到：{err}");
    }

    #[tokio::test]
    async fn test_delete_missing_session_is_not_found() {
        let mgr = make_manager().await;

        let err = mgr.delete_session("no-such").await.unwrap_err();
        assert!(err.is_not_found(), "删除缺失会话应分类为未找到：{err}");
    }

    #[tokio::test]
    async fn test_rewrite_messages_missing_session_is_not_found() {
        let mgr = make_manager().await;

        let err = mgr.rewrite_messages("no-such", &[]).await.unwrap_err();
        assert!(err.is_not_found(), "重写缺失会话应分类为未找到：{err}");
    }

    #[tokio::test]
    async fn test_empty_message_persists_and_loads() {
        // 回归保护：纯图片/空正文消息必须完整持久化（content_parts 全保真）。
        let mgr = make_manager().await;
        let id = "test-empty";

        mgr.create_session(id, Message::user("start"))
            .await
            .unwrap();
        let empty = make_msg("empty-1", id, MessageRole::User, "", false);
        mgr.add_structured_message(id, empty).await.unwrap();

        let loaded = mgr.get_session(id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[1].id, "empty-1");
        assert!(
            loaded.messages[1].parts.is_empty(),
            "空正文消息的 parts 应为空"
        );
    }
}
