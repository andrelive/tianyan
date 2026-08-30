//! 会话权威存储（ADR-018：SQLite 表为唯一真相）。
//!
//! **SQL 已收敛到 [`crate::db::session::SessionRepo`]**：本组件是面向领域中上层
//! 的封装（保持既有公共 API：`SessionStore::new` + 各方法），数据访问全部委托仓储。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::common::types::StructuredMessage;
use crate::db::Database;
use crate::db::session::SessionRepo;
use crate::session::types::{SessionHeader, SessionMeta};

/// 会话权威存储（封装 [`SessionRepo`]，公共 API 稳定）。
pub struct SessionStore {
    repo: Arc<SessionRepo>,
}

impl SessionStore {
    /// 创建会话存储（共享 Database 单连接；SQL 经 SessionRepo 收敛）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self {
            repo: Arc::new(SessionRepo::new(db)),
        }))
    }

    /// 创建会话（写入元数据；已存在 → 冲突）。
    pub async fn create(
        &self,
        session_id: &str,
        header: &SessionHeader,
    ) -> Result<(), TianyanError> {
        self.repo.create(session_id, header).await
    }

    /// 追加一条消息（单事务内原子取号 + 写完整消息 + FTS）。
    pub async fn append_message(
        &self,
        session_id: &str,
        msg: &StructuredMessage,
    ) -> Result<(), TianyanError> {
        self.repo.append_message(session_id, msg).await
    }

    /// 加载会话：元数据 + 全量消息（按 seq 升序）。
    pub async fn load(
        &self,
        session_id: &str,
    ) -> Result<Option<(SessionHeader, Vec<StructuredMessage>)>, TianyanError> {
        self.repo.load(session_id).await
    }

    /// 全量重写会话（事务内清 FTS + 消息 + 重写 meta + 逐条写入）。
    pub async fn rewrite(
        &self,
        session_id: &str,
        header: &SessionHeader,
        messages: &[StructuredMessage],
    ) -> Result<(), TianyanError> {
        self.repo.rewrite(session_id, header, messages).await
    }

    /// 读取会话头部。
    pub async fn header(&self, session_id: &str) -> Result<Option<SessionHeader>, TianyanError> {
        self.repo.header(session_id).await
    }

    /// 更新会话头部。
    pub async fn update_header(
        &self,
        session_id: &str,
        header: &SessionHeader,
    ) -> Result<(), TianyanError> {
        self.repo.update_header(session_id, header).await
    }

    /// 轻量列出所有会话（仅元数据，按创建时间倒序）。
    pub async fn list_meta(&self) -> Result<Vec<SessionMeta>, TianyanError> {
        self.repo.list_meta().await
    }

    /// 会话是否存在。
    pub async fn exists(&self, session_id: &str) -> Result<bool, TianyanError> {
        self.repo.exists(session_id).await
    }

    /// 删除会话（单事务：清 FTS + 消息 + 元数据）。
    pub async fn delete(&self, session_id: &str) -> Result<(), TianyanError> {
        self.repo.delete(session_id).await
    }

    /// 导出会话为 JSONL 文本（vfs_read 兼容层用）。
    pub async fn export_jsonl(&self, session_id: &str) -> Result<Option<String>, TianyanError> {
        self.repo.export_jsonl(session_id).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime};

    fn msg(id: &str, role: MessageRole, text: &str) -> StructuredMessage {
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
            tokens: DetailedTokenUsage {
                total: 10,
                ..Default::default()
            },
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: 1_700_000_000_000 + id.len() as i64,
                completed: 0,
            },
            session_id: "s1".to_string(),
            finish: None,
            compression_marker: false,
        }
    }

    async fn make_store() -> Arc<SessionStore> {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        SessionStore::new(db).unwrap()
    }

    #[tokio::test]
    async fn test_append_load_roundtrip() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "你好"))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::Assistant, "世界"))
            .await
            .unwrap();

        let (loaded_header, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(loaded_header.session_header, Some(SessionHeader::MARKER));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[1].id, "m2");
        assert!(store.load("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_append_seq_monotonic_per_session() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        for i in 0..5 {
            store
                .append_message("s1", &msg(&format!("m{i}"), MessageRole::User, "x"))
                .await
                .unwrap();
        }
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 5);
        // 消息行独立于 meta：追加到不存在的会话仍成功（meta 由 create 负责）
        store
            .append_message("ghost", &msg("g1", MessageRole::User, "y"))
            .await
            .unwrap();
        let (_, messages) = store.load("ghost").await.unwrap().unwrap();
        assert_eq!(messages.len(), 1);
        // 不同会话序号独立（ghost 有 1 条，s1 有 5 条）
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 5);
    }

    #[tokio::test]
    async fn test_empty_message_occupies_seq_but_skips_fts() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m-empty", MessageRole::User, ""))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::User, "正文内容"))
            .await
            .unwrap();
        let (_, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].id, "m2");
        let recall = crate::session::search::SessionRecall::new(store.repo.database().clone()).unwrap();
        let hits = recall.search("正文内容", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        let hits = recall.search("不存在的词", 10).await.unwrap();
        assert_eq!(hits.len(), 0);
    }

    #[tokio::test]
    async fn test_rewrite_preserves_order_and_header() {
        let store = make_store().await;
        let header = SessionHeader {
            title: Some("标题".to_string()),
            ..SessionHeader::default()
        };
        store.create("s1", &header).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "旧"))
            .await
            .unwrap();
        let mut new_header = header.clone();
        new_header.title = Some("新标题".to_string());
        store
            .rewrite("s1", &new_header, &[msg("m2", MessageRole::User, "新")])
            .await
            .unwrap();
        let (loaded_header, messages) = store.load("s1").await.unwrap().unwrap();
        assert_eq!(loaded_header.title.as_deref(), Some("新标题"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "m2");
    }

    #[tokio::test]
    async fn test_meta_delete_and_recall() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "x"))
            .await
            .unwrap();
        store.create("s2", &SessionHeader::default()).await.unwrap();
        let metas = store.list_meta().await.unwrap();
        assert_eq!(metas.len(), 2);
        assert!(store.exists("s1").await.unwrap());
        assert!(!store.exists("s3").await.unwrap());
        store.delete("s1").await.unwrap();
        assert!(!store.exists("s1").await.unwrap());
        assert_eq!(store.list_meta().await.unwrap().len(), 1);
        // 删除后回忆也查不到
        let recall = crate::session::search::SessionRecall::new(store.repo.database().clone()).unwrap();
        assert!(recall.search("x", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_export_jsonl_roundtrip() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        store
            .append_message("s1", &msg("m1", MessageRole::User, "你好"))
            .await
            .unwrap();
        let jsonl = store.export_jsonl("s1").await.unwrap().unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(SessionHeader::parse_line(lines[0]).is_some());
        let parsed = crate::session::parse_message_lines(&jsonl);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "m1");
        assert_eq!(store.export_jsonl("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_create_duplicate_is_conflict() {
        let store = make_store().await;
        store.create("s1", &SessionHeader::default()).await.unwrap();
        let err = store
            .create("s1", &SessionHeader::default())
            .await
            .unwrap_err();
        assert!(err.is_conflict());
    }
}
