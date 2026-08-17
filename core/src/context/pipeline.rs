//! 上下文工程管线。
//!
//! 将上下文装配的完整流程封装为可复用、可测试的管线：
//! load soul → load rules → load memories → compress → build InjectableContext

use std::sync::Arc;

use chrono;
use tokio::sync::Mutex as TokioMutex;

use crate::common::error::Result;
use crate::common::types::InjectableContext;
use crate::common::types::{
    AgentPath, ContentLevel, ContextNamespace, DetailedTokenUsage, Message, MessageRole,
    MessageTime, Part, PartTime, StructuredMessage,
};
use crate::context::compression::ContextCompressor;
use crate::context::retrieval::DualLayerRetriever;
use crate::vfs::VirtualFileSystem;

/// 上下文工程管线。
#[derive(Clone)]
pub struct ContextPipeline {
    vfs: Arc<dyn VirtualFileSystem>,
    retriever: Arc<DualLayerRetriever>,
    compressor: Arc<TokioMutex<ContextCompressor>>,
    default_top_k: usize,
    learned_rules_top_k: usize,
    /// 已缓存的 soul（首次加载后复用，避免每轮从 VFS 重复读取）
    cached_soul: Arc<TokioMutex<Option<String>>>,
}

impl ContextPipeline {
    /// 创建新的上下文管线。
    ///
    /// - `vfs` - 虚拟文件系统
    /// - `retriever` - 上下文检索器
    /// - `compressor` - 压缩器
    /// - `default_top_k` - 默认检索结果数
    /// - `learned_rules_top_k` - learned rules 注入 Top-K
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        retriever: Arc<DualLayerRetriever>,
        compressor: Arc<TokioMutex<ContextCompressor>>,
        default_top_k: usize,
        learned_rules_top_k: usize,
    ) -> Self {
        Self {
            vfs,
            retriever,
            compressor,
            default_top_k,
            learned_rules_top_k,
            cached_soul: Arc::new(TokioMutex::new(None)),
        }
    }

    /// 压缩器引用（crate 内测试断言压缩配置用）。
    #[cfg(test)]
    pub(crate) fn compressor(&self) -> &Arc<TokioMutex<ContextCompressor>> {
        &self.compressor
    }

    /// 加载可注入上下文（soul + rules + memories）。
    ///
    /// soul 通过 VFS 读取（有内部缓存），rules 和 memories 通过向量检索。
    /// 此方法有 I/O 开销，调用方应在会话级缓存结果。
    pub async fn load_injectable(&self, query: &str) -> Result<InjectableContext> {
        let mut injectable = InjectableContext::new();

        // Load soul — 仅首次加载，后续复用缓存
        {
            let soul_guard = self.cached_soul.lock().await;
            if let Some(soul) = soul_guard.as_ref() {
                injectable.soul = soul.clone();
            }
        }
        if injectable.soul.is_empty() {
            let soul_uri = AgentPath::Soul.uri();
            match self.vfs.read_content(&soul_uri, ContentLevel::Detail).await {
                Ok(content) => {
                    *self.cached_soul.lock().await = Some(content.clone());
                    injectable.soul = content;
                }
                Err(e) => {
                    tracing::warn!(error = %e, uri = %soul_uri, "加载核心提示词失败");
                }
            }
        }

        // Load learned rules and experiences
        let rules = self.load_relevant_rules(query).await;
        injectable.rules_and_experiences = rules;

        // Load memories
        let memories = self.load_memories(query).await;
        injectable.memories = memories;

        Ok(injectable)
    }

    /// 执行完整上下文管线（加载 + 压缩便捷方法。
    ///
    /// - `query` - 用户查询（用于检索和规则匹配）
    /// - `conversation` - 当前对话历史（可变引用，压缩可能修改）
    ///
    /// 返回 InjectableContext（soul + rules + memories）和压缩摘要。
    ///
    /// 仅测试使用（生产路径直接调用 load_injectable / compress_for_session）。
    #[cfg(test)]
    pub async fn run(
        &self,
        query: &str,
        conversation: &mut Vec<Message>,
    ) -> Result<(InjectableContext, Option<String>)> {
        let injectable = self.load_injectable(query).await?;
        let summary = self.compress_if_needed(conversation, 0).await?;
        Ok((injectable, summary))
    }

    /// 如需压缩则执行压缩，返回摘要文本。
    pub async fn compress_if_needed(
        &self,
        conversation: &mut Vec<Message>,
        recent_input_tokens: usize,
    ) -> Result<Option<String>> {
        let mut compressor = self.compressor.lock().await;
        if !compressor.should_compress(recent_input_tokens) {}

        let result = compressor.compress(conversation).await?;
        let summary = if result.summary.is_empty() {
            None
        } else {
            Some(result.summary)
        };

        *conversation = result.messages;
        tracing::info!(
            compressed = result.compressed_count,
            original_tokens = result.original_tokens,
            compressed_tokens = result.compressed_tokens,
            "上下文压缩完成"
        );

        Ok(summary)
    }

    /// 压缩对话并生成带 compression_marker 的摘要 StructuredMessage。
    /// 返回 None 如果不需要压缩或压缩结果为空。
    pub async fn compress_for_session(
        &self,
        messages: &[Message],
        session_id: &str,
        recent_input_tokens: usize,
    ) -> Option<StructuredMessage> {
        let mut conversation = messages.to_vec();

        let summary = match self
            .compress_if_needed(&mut conversation, recent_input_tokens)
            .await
        {
            Ok(Some(summary)) => summary,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "上下文压缩失败，本次会话以降级（不压缩）方式继续"
                );
                return None;
            }
        };

        Some(StructuredMessage {
            id: format!("cmp_{}", chrono::Utc::now().timestamp_millis()),
            parent_id: None,
            role: MessageRole::System,
            parts: vec![Part::Text {
                text: format!(
                    "[对话摘要] 以下是对历史对话的摘要：\n{}\n[摘要结束]",
                    summary
                ),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime {
                created: chrono::Utc::now().timestamp_millis(),
                completed: chrono::Utc::now().timestamp_millis(),
            },
            session_id: session_id.to_string(),
            finish: None,
            compression_marker: true,
        })
    }

    /// 使用 retriever 渐进式检索与当前 query 最相关的 learned rules。
    async fn load_relevant_rules(&self, query: &str) -> Vec<String> {
        match self
            .retriever
            .retrieve_by_namespace(query, self.learned_rules_top_k, ContextNamespace::Agent)
            .await
        {
            Ok(results) => results
                .into_iter()
                .filter_map(|r| r.content)
                .filter(|c| !c.trim().is_empty())
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, query = %query, "learned rules 检索失败，回退到全量列表");
                self.fallback_load_rules().await
            }
        }
    }

    /// 回退加载：当向量检索不可用时，从 VFS 目录遍历（兼容无向量的规则）。
    async fn fallback_load_rules(&self) -> Vec<String> {
        let learned_uri = AgentPath::Learned.uri();
        let entries = match self.vfs.list(&learned_uri).await {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut rules = Vec::new();
        for entry in &entries {
            if let Ok(content) = self
                .vfs
                .read_content(entry.uri(), ContentLevel::Abstract)
                .await
            {
                if !content.trim().is_empty() {
                    rules.push(content.trim().to_string());
                }
            }
        }

        let fallback_limit = self.learned_rules_top_k * 2;
        if rules.len() > fallback_limit {
            rules.truncate(fallback_limit);
        }

        rules
    }

    /// 检索相关记忆。
    async fn load_memories(&self, query: &str) -> Vec<String> {
        match self
            .retriever
            .retrieve_by_namespace(query, self.default_top_k, ContextNamespace::Memory)
            .await
        {
            Ok(results) => results
                .into_iter()
                .filter_map(|r| r.content)
                .filter(|c| !c.trim().is_empty())
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "记忆检索失败");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::MockVfs;

    use crate::common::types::{ContentLevel, ContextNamespace, Message, SearchResult, TianyanUri};
    use crate::context::compression::CompressionConfig;
    use crate::context::retrieval::DualLayerRetriever;
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::ChatService;
    use crate::test_utils::MockChatService;
    use crate::vfs::{ContextEntry, VirtualFileSystem};

    /// 创建返回指定响应文本的 mock ChatService。
    fn mock_chat(response: &str) -> Arc<dyn ChatService> {
        let response = response.to_string();
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(move |_| {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: Message::assistant(response.clone()),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Default::default(),
            })
        });
        Arc::new(mock)
    }

    fn make_uri(namespace: ContextNamespace, path: &[&str]) -> TianyanUri {
        TianyanUri::new(namespace, path.iter().map(|s| s.to_string()).collect())
    }

    fn make_search_result(uri: &TianyanUri, score: f32) -> SearchResult {
        SearchResult {
            uri: uri.clone(),
            score,
        }
    }

    fn make_pipeline(vfs: Arc<MockVfs>) -> (ContextPipeline, Arc<MockVfs>) {
        let retriever = Arc::new(DualLayerRetriever::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>
        ));
        let compressor = Arc::new(TokioMutex::new(ContextCompressor::new(
            mock_chat(""),
            CompressionConfig::default(),
        )));
        let pipeline = ContextPipeline::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>,
            retriever,
            compressor,
            10,
            5,
        );
        (pipeline, vfs)
    }

    fn make_pipeline_with_compressor(
        vfs: Arc<MockVfs>,
        compressor: ContextCompressor,
    ) -> ContextPipeline {
        let retriever = Arc::new(DualLayerRetriever::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>
        ));
        ContextPipeline::new(
            vfs as Arc<dyn VirtualFileSystem>,
            retriever,
            Arc::new(TokioMutex::new(compressor)),
            10,
            5,
        )
    }

    // ── Soul loading ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_loads_soul() {
        let soul_uri = AgentPath::Soul.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "You are a helpful AI.")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _summary) = pipeline.run("test", &mut conversation).await.unwrap();

        assert_eq!(ctx.soul, "You are a helpful AI.");
    }

    #[tokio::test]
    async fn test_run_soul_load_failure_does_not_crash() {
        let vfs = Arc::new(MockVfs::new());
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();
        assert!(ctx.soul.is_empty());
    }

    // ── Rules loading ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_loads_rules() {
        let soul_uri = AgentPath::Soul.uri();
        let rule_uri = make_uri(ContextNamespace::Agent, &["learned", "rule1"]);
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .with_content(&rule_uri, ContentLevel::Detail, "Always be concise.")
                .with_search_results(vec![make_search_result(&rule_uri, 0.9)])
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert_eq!(ctx.rules_and_experiences, vec!["Always be concise."]);
    }

    #[tokio::test]
    async fn test_run_rules_empty_results() {
        let soul_uri = AgentPath::Soul.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert!(ctx.rules_and_experiences.is_empty());
    }

    #[tokio::test]
    async fn test_run_rules_retrieval_error_falls_back() {
        let soul_uri = AgentPath::Soul.uri();
        let learned_uri = AgentPath::Learned.uri();
        let rule_uri = make_uri(ContextNamespace::Agent, &["learned", "rule1.md"]);
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .with_content(&rule_uri, ContentLevel::Abstract, "Fallback rule content.")
                .with_entries(&learned_uri, vec![ContextEntry::new_file(rule_uri.clone())])
                .with_search_error("vector store unavailable")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert_eq!(ctx.rules_and_experiences, vec!["Fallback rule content."]);
    }

    #[tokio::test]
    async fn test_run_rules_fallback_empty_dir() {
        let soul_uri = AgentPath::Soul.uri();
        let learned_uri = AgentPath::Learned.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .with_entries(&learned_uri, vec![])
                .with_search_error("vector store unavailable")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert!(ctx.rules_and_experiences.is_empty());
    }

    // ── Memories loading ────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_loads_memories() {
        let soul_uri = AgentPath::Soul.uri();
        let mem_uri = make_uri(ContextNamespace::Memory, &["mem1"]);
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .with_content(&mem_uri, ContentLevel::Detail, "User prefers dark mode.")
                .with_search_results(vec![make_search_result(&mem_uri, 0.9)])
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert_eq!(ctx.memories, vec!["User prefers dark mode."]);
    }

    #[tokio::test]
    async fn test_run_memories_retrieval_error_returns_empty() {
        let soul_uri = AgentPath::Soul.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .with_search_error("vector store unavailable")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![];

        let (ctx, _) = pipeline.run("test", &mut conversation).await.unwrap();

        assert!(ctx.memories.is_empty());
    }

    // ── Compression ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_no_compression_for_few_messages() {
        let soul_uri = AgentPath::Soul.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .build(),
        );
        let (pipeline, _) = make_pipeline(vfs);
        let mut conversation = vec![Message::user("hello"), Message::assistant("hi")];

        let (_, summary) = pipeline.run("test", &mut conversation).await.unwrap();

        assert!(
            summary.is_none(),
            "compression should not trigger with 2 messages"
        );
    }

    #[tokio::test]
    async fn test_run_compression_triggers_with_many_messages() {
        let soul_uri = AgentPath::Soul.uri();
        let vfs = Arc::new(
            MockVfs::builder()
                .with_content(&soul_uri, ContentLevel::Detail, "soul")
                .build(),
        );

        let config = CompressionConfig {
            context_window: 100,
            min_messages_to_compress: 3,
            preserve_recent_messages: 2,
            ..Default::default()
        };
        let compressor = ContextCompressor::new(mock_chat("Summarized conversation."), config);
        let pipeline = make_pipeline_with_compressor(vfs, compressor);

        let mut conversation = vec![
            Message::user("What is the weather?"),
            Message::assistant("It is sunny."),
            Message::user("And tomorrow?"),
            Message::assistant("Rainy."),
            Message::user("Thanks."),
            Message::assistant("You're welcome."),
        ];

        let (_, summary) = pipeline.run("test", &mut conversation).await.unwrap();

        assert!(
            summary.is_some(),
            "compression should trigger with 6 messages and 100 token window"
        );
        assert!(conversation.len() < 6);
    }
}
