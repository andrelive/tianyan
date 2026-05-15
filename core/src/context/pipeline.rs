//! 上下文工程管线。
//!
//! 将上下文装配的完整流程封装为可复用、可测试的管线：
//! load_system_prompt → search → compress → build ContextWindow

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{AgentPath, ContentLevel, ContextNamespace, Message};
use crate::context::compression::{estimate_tokens, ContextCompressor};
use crate::context::retrieval::ContextRetriever;
use crate::context::types::{ContextTokenUsage, ContextWindow, RetrievalResult};
use crate::storage::VirtualFileSystem;

/// 上下文工程管线。
#[derive(Clone)]
pub struct ContextPipeline {
    vfs: Arc<dyn VirtualFileSystem>,
    retriever: Arc<dyn ContextRetriever>,
    compressor: ContextCompressor,
    default_top_k: usize,
    learned_rules_top_k: usize,
    learned_rules_max_tokens: usize,
}

impl ContextPipeline {
    /// 创建新的上下文管线。
    ///
    /// - `vfs` - 虚拟文件系统
    /// - `retriever` - 上下文检索器
    /// - `compressor` - 压缩器
    /// - `default_top_k` - 默认检索结果数
    /// - `learned_rules_top_k` - learned rules 注入 Top-K
    /// - `learned_rules_max_tokens` - learned rules 段最大 token 数
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        retriever: Arc<dyn ContextRetriever>,
        compressor: ContextCompressor,
        default_top_k: usize,
        learned_rules_top_k: usize,
        learned_rules_max_tokens: usize,
    ) -> Self {
        Self {
            vfs,
            retriever,
            compressor,
            default_top_k,
            learned_rules_top_k,
            learned_rules_max_tokens,
        }
    }

    /// 执行完整上下文管线。
    ///
    /// - `query` - 用户查询（用于检索和规则匹配）
    /// - `conversation` - 当前对话历史（可变引用，压缩可能修改）
    pub async fn run(&self, query: &str, conversation: &mut Vec<Message>) -> Result<ContextWindow> {
        let system_prompt = self.load_system_prompt(query).await?;
        let retrieved = self.search(query).await?;
        let summary = self.compress_if_needed(conversation).await?;

        let mut token_usage = ContextTokenUsage::default();
        token_usage.system_prompt_tokens = estimate_tokens(&system_prompt);
        token_usage.retrieved_tokens = retrieved.iter().map(|r| r.token_count).sum();

        if let Some(ref s) = summary {
            token_usage.summary_tokens = estimate_tokens(s);
        }

        Ok(ContextWindow {
            system_prompt,
            summary,
            retrieved,
            token_usage,
        })
    }

    /// 加载系统提示词：soul（完整） + learned rules（渐进式披露）。
    ///
    /// Soul 永远完整注入。Learned rules 通过 retriever 按 query 语义搜索，
    /// 仅注入 Top-K 条相关性最高的规则，实现渐进式披露。
    async fn load_system_prompt(&self, query: &str) -> Result<String> {
        let mut prompt = String::new();

        let soul_uri = AgentPath::Soul.uri();
        match self.vfs.read_content(&soul_uri, ContentLevel::Detail).await {
            Ok(content) => {
                prompt.push_str(&content);
                prompt.push_str("\n\n");
            }
            Err(e) => {
                tracing::warn!(error = %e, uri = %soul_uri, "加载核心提示词失败");
            }
        }

        // 渐进式披露：用 retriever 搜索与当前 query 最相关的 learned rules
        let rules = self.load_relevant_rules(query).await;
        if !rules.is_empty() {
            let mut rule_section = String::from("---\n");
            rule_section.push_str("## 相关经验教训\n\n");
            let mut token_budget = self.learned_rules_max_tokens;

            for rule in &rules {
                let line = format!("- {}\n", rule.trim());
                let line_tokens = estimate_tokens(&line);
                if line_tokens > token_budget {
                    break;
                }
                token_budget = token_budget.saturating_sub(line_tokens);
                rule_section.push_str(&line);
            }
            prompt.push_str(&rule_section);
        }

        Ok(prompt)
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
                // 回退：兼容向量存储中暂无 Agent 命名空间数据的情况
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

        // 回退时最多保留 learned_rules_top_k * 2 条（无相关性排序）
        let fallback_limit = self.learned_rules_top_k * 2;
        if rules.len() > fallback_limit {
            rules.truncate(fallback_limit);
        }

        rules
    }

    /// 统一检索（Memory + Knowledge 一次搜索），带 Token 预算控制。
    async fn search(&self, query: &str) -> Result<Vec<RetrievalResult>> {
        let budget = self.retriever.default_token_budget();
        self.retriever
            .retrieve_with_budget(query, self.default_top_k, budget)
            .await
    }

    /// 如需压缩则执行压缩，返回摘要文本。
    async fn compress_if_needed(&self, conversation: &mut Vec<Message>) -> Result<Option<String>> {
        let mut compressor = self.compressor.clone();
        if !compressor.should_compress(conversation) {
            return Ok(None);
        }

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
}
