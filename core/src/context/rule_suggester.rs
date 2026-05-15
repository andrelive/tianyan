//! 规则建议器。
//!
//! 监控 Memory 命名空间中的 FailedCase 和 Pattern 记忆，
//! 当同类失败 ≥ 阈值时自动提炼为 Learned Rule，
//! 写入 agent/learned/，实现从记忆到规则的转化。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::model::ModelService;
use crate::storage::VirtualFileSystem;

use super::rule_recorder::{FailureKind, RuleRecorder};

const DEFAULT_CLUSTER_PROMPT: &str = r#"分析以下失败记忆列表，判断是否有重复出现的模式。

失败记忆：
{memories}

请判断：
1. 这些记忆中是否存在相同的根因模式？
2. 如果有，请提炼出一条通用的避免规则。

以 JSON 格式返回：
{
  "has_pattern": true/false,
  "rule_abstract": "规则摘要（50字以内）",
  "rule_detail": "规则详细说明，包含反例和正确做法"
}

如果没有明显模式，返回 { "has_pattern": false }"#;

const MIN_OCCURRENCES: usize = 2;

/// 规则建议器。
#[derive(Clone)]
pub struct RuleSuggester {
    vfs: Arc<dyn VirtualFileSystem>,
    model_service: Arc<dyn ModelService>,
}

impl RuleSuggester {
    pub fn new(vfs: Arc<dyn VirtualFileSystem>, model_service: Arc<dyn ModelService>) -> Self {
        Self { vfs, model_service }
    }

    /// 扫描 FailedCase 和 Pattern 记忆，检测可提炼的模式。
    pub async fn scan(&self) -> Result<Vec<RuleSuggestion>> {
        let mut suggestions = Vec::new();

        let pattern_uri = TianyanUri::new(ContextNamespace::Agent, vec!["patterns".to_string()]);
        if let Some(s) = self.analyze_category(&pattern_uri, "pattern").await {
            suggestions.push(s);
        }

        let failed_uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["cases".to_string(), "failed_tasks".to_string()],
        );
        if let Some(s) = self.analyze_category(&failed_uri, "failed_case").await {
            suggestions.push(s);
        }

        Ok(suggestions)
    }

    /// 分析一个类别下的记忆，检测聚类。
    async fn analyze_category(&self, uri: &TianyanUri, category: &str) -> Option<RuleSuggestion> {
        if !self.vfs.exists(uri).await.ok()? {
            return None;
        }

        let entries = match self.vfs.list(uri).await {
            Ok(e) => e,
            Err(_) => return None,
        };

        if entries.len() < MIN_OCCURRENCES {
            return None;
        }

        let mut contents = Vec::new();
        for entry in &entries {
            if let Ok(content) = self
                .vfs
                .read_content(entry.uri(), ContentLevel::Abstract)
                .await
            {
                if !content.trim().is_empty() {
                    contents.push(content);
                }
            }
        }

        if contents.len() < MIN_OCCURRENCES {
            return None;
        }

        Some(RuleSuggestion {
            source_category: category.to_string(),
            source_count: contents.len(),
            source_contents: contents,
        })
    }

    /// 使用 LLM 将聚类内容提炼为规则，并写入 agent/learned/。
    pub async fn promote_to_rule(
        &self,
        suggestion: &RuleSuggestion,
        session_id: &str,
    ) -> Result<()> {
        let memories_text = suggestion
            .source_contents
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = DEFAULT_CLUSTER_PROMPT.replace("{memories}", &memories_text);

        let response = self
            .model_service
            .chat(
                "gpt-4o-mini",
                vec![crate::common::types::Message::user(prompt)],
            )
            .await?;

        let json: serde_json::Value = {
            let cleaned = response
                .trim()
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str(cleaned).unwrap_or_default()
        };

        if json
            .get("has_pattern")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let abstract_text = json
                .get("rule_abstract")
                .and_then(|v| v.as_str())
                .unwrap_or("从历史记忆中提炼的规则");
            let detail_text = json
                .get("rule_detail")
                .and_then(|v| v.as_str())
                .unwrap_or("无详细说明");

            let recorder = RuleRecorder::new(self.vfs.clone());
            recorder
                .record_with_kind(abstract_text, detail_text, session_id, FailureKind::Logic)
                .await?;

            tracing::info!(
                category = %suggestion.source_category,
                source_count = suggestion.source_count,
                abstract_text = abstract_text,
                "已从记忆聚类提炼规则"
            );
        }

        Ok(())
    }
}

/// 规则建议（聚类结果）。
pub struct RuleSuggestion {
    pub source_category: String,
    pub source_count: usize,
    pub source_contents: Vec<String>,
}
