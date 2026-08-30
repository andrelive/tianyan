//! 会话回忆服务（ADR-017 决策 6：FTS5 消息索引）。
//!
//! **SQL 已收敛到 [`crate::db::session::SessionRepo`]**：本组件封装全文回忆
//! 查询（保持公共 API），数据访问委托仓储。

use std::sync::Arc;

use crate::common::error::TianyanError;
use crate::db::Database;
use crate::db::session::SessionRepo;
use crate::session::types::RecallHit;

/// 会话回忆服务（封装 [`SessionRepo`] 的全文查询）。
pub struct SessionRecall {
    repo: Arc<SessionRepo>,
}

impl SessionRecall {
    /// 创建会话回忆服务（共享 Database 单连接）。
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>, TianyanError> {
        Ok(Arc::new(Self {
            repo: Arc::new(SessionRepo::new(db)),
        }))
    }

    /// 全文检索消息（bm25 排序；空查询返回空）。
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<RecallHit>, TianyanError> {
        self.repo.search(query, limit).await
    }

    /// 命中附近窗口（center_seq 前后 radius 条）。
    pub async fn window(
        &self,
        session_id: &str,
        center_seq: i64,
        radius: i64,
    ) -> Result<Vec<crate::session::types::RecallMessage>, TianyanError> {
        self.repo.window(session_id, center_seq, radius).await
    }

    /// 水位线后的消息（增量；供记忆提取/演化任务）。
    pub async fn messages_since(
        &self,
        session_id: &str,
        since_seq: i64,
        limit: usize,
    ) -> Result<Vec<crate::session::types::RecallMessage>, TianyanError> {
        self.repo.messages_since(session_id, since_seq, limit).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{
        DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime, StructuredMessage,
    };
    use crate::session::store::SessionStore;
    use crate::db::session::sanitize_fts_query;

    fn msg(
        id: &str,
        role: MessageRole,
        text: &str,
        tool: Option<(&str, &str)>,
    ) -> StructuredMessage {
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                time: PartTime::default(),
            });
        }
        if let Some((name, args)) = tool {
            parts.push(Part::ToolCall {
                id: format!("call-{id}"),
                name: name.to_string(),
                arguments: args.to_string(),
                time: PartTime::default(),
            });
            parts.push(Part::ToolResult {
                tool_call_id: format!("call-{id}"),
                content: format!("结果-{id}"),
                error: None,
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

    /// 建内存库：store（写数据）+ recall（查数据）共享同一 SqliteDb。
    async fn make_pair() -> (Arc<SessionStore>, Arc<SessionRecall>) {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        (
            SessionStore::new(db.clone()).unwrap(),
            SessionRecall::new(db).unwrap(),
        )
    }

    #[tokio::test]
    async fn test_index_and_search_chinese_substring() {
        let (store, recall) = make_pair().await;
        store
            .append_message(
                "s1",
                &msg("m1", MessageRole::User, "我们讨论部署方案", None),
            )
            .await
            .unwrap();
        store
            .append_message(
                "s1",
                &msg("m2", MessageRole::Assistant, "建议用 docker 部署", None),
            )
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m3", MessageRole::User, "今天天气不错", None))
            .await
            .unwrap();

        // 中文子串命中（trigram）
        let hits = recall.search("部署方案", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m1");
        assert!(hits[0].score > 0.0);

        // 英文子串命中
        let hits = recall.search("docker", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m2");

        // 无命中
        let hits = recall.search("不存在的词", 5).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_tool_parts_not_in_fts() {
        let (store, recall) = make_pair().await;
        store
            .append_message(
                "s1",
                &msg(
                    "m1",
                    MessageRole::Assistant,
                    "我来查一下",
                    Some(("read_file", "{\"path\": \"a.rs\"}")),
                ),
            )
            .await
            .unwrap();

        // 工具名/参数不进 FTS（text 列只有 user/assistant 文本）
        let hits = recall.search("read_file", 5).await.unwrap();
        assert!(hits.is_empty());
        let hits = recall.search("我来查一下", 5).await.unwrap();
        assert_eq!(hits.len(), 1);

        // tool_text 保留（回忆窗口展示）
        let since = recall.messages_since("s1", -1, 10).await.unwrap();
        assert_eq!(since.len(), 1);
        assert!(since[0].tool_text.contains("read_file"));
        assert!(since[0].tool_text.contains("结果-m1"));
    }

    #[tokio::test]
    async fn test_window_around_hit() {
        let (store, recall) = make_pair().await;
        for i in 0..10 {
            store
                .append_message(
                    "s1",
                    &msg(
                        &format!("m{i}"),
                        MessageRole::User,
                        &format!("消息 {i}"),
                        None,
                    ),
                )
                .await
                .unwrap();
        }
        let win = recall.window("s1", 5, 2).await.unwrap();
        assert_eq!(win.len(), 5);
        assert_eq!(win[0].seq, 3);
        assert_eq!(win[4].seq, 7);
    }

    #[tokio::test]
    async fn test_messages_since_watermark() {
        let (store, recall) = make_pair().await;
        for i in 0..5 {
            store
                .append_message(
                    "s1",
                    &msg(
                        &format!("m{i}"),
                        MessageRole::User,
                        &format!("消息 {i}"),
                        None,
                    ),
                )
                .await
                .unwrap();
        }
        let since = recall.messages_since("s1", 2, 10).await.unwrap();
        assert_eq!(since.len(), 2);
        assert_eq!(since[0].seq, 3);
        assert_eq!(since[1].seq, 4);
    }

    #[tokio::test]
    async fn test_empty_message_excluded_from_recall_queries() {
        let (store, recall) = make_pair().await;
        // 空正文消息占 seq 但不参与窗口/增量查询
        store
            .append_message("s1", &msg("m-empty", MessageRole::User, "", None))
            .await
            .unwrap();
        store
            .append_message("s1", &msg("m2", MessageRole::User, "有正文", None))
            .await
            .unwrap();
        let since = recall.messages_since("s1", -1, 10).await.unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].message_id, "m2");
        let win = recall.window("s1", 1, 1).await.unwrap();
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].message_id, "m2");
    }

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(sanitize_fts_query("部署方案"), "\"部署方案\"");
        assert_eq!(sanitize_fts_query("a b c"), "\"abc\"");
        assert_eq!(sanitize_fts_query("ab"), "");
        assert_eq!(sanitize_fts_query("带\"引号\"的词"), "\"带引号的词\"");
    }
}
