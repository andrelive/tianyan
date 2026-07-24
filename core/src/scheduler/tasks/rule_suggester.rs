//! 规则建议器。
//!
//! 监控 Memory 命名空间中的 FailedCase 和 Pattern 记忆，
//! 当同类失败 ≥ 阈值时自动提炼为 Learned Rule，
//! 写入 agent/learned/，实现从记忆到规则的转化。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::model::ChatService;
use crate::vfs::VirtualFileSystem;

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
    model_service: Arc<dyn ChatService>,
    /// 用于规则提炼的模型名称（如 "gpt-4o-mini"）。
    model_name: String,
    model_version: String,
    pipeline_version: String,
}

impl RuleSuggester {
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        model_service: Arc<dyn ChatService>,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            vfs,
            model_service,
            model_name: model_name.into(),
            model_version: "unknown".to_string(),
            pipeline_version: "1.0".to_string(),
        }
    }

    /// 设置模型版本（用于记录规则生成来源）。
    pub fn with_model_version(mut self, version: impl Into<String>) -> Self {
        self.model_version = version.into();
        self
    }

    /// 设置 Pipeline 版本（用于记录规则生成来源）。
    pub fn with_pipeline_version(mut self, version: impl Into<String>) -> Self {
        self.pipeline_version = version.into();
        self
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
                &self.model_name,
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

            let recorder = RuleRecorder::new(self.vfs.clone())
                .with_model_version(&self.model_version)
                .with_pipeline_version(&self.pipeline_version);
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::common::types::{
        AgentPath, ContentLevel, ContextNamespace, SearchResult, TianyanUri,
    };
    use crate::model::types::{ChatChoice, ChatCompletionRequest, ChatCompletionResponse};
    use crate::test_utils::MockChatService;
    use crate::test_utils::MockVfs;
    use crate::vfs::ContextEntry;

    /// 创建返回指定 JSON 响应的 mock ChatService。
    fn mock_chat_json(json: &str) -> Arc<dyn ChatService> {
        let json = json.to_string();
        let mut mock = MockChatService::new();
        mock.expect_chat_completion().returning(move |_| {
            Ok(ChatCompletionResponse {
                id: "mock".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![ChatChoice {
                    index: 0,
                    message: crate::common::types::Message::assistant(json.clone()),
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

    // ── scan: no entries = no suggestions ───────────────────────────

    #[tokio::test]
    async fn test_scan_no_entries_returns_empty() {
        let vfs = Arc::new(MockVfs::new());
        let chat = mock_chat_json("{}");
        let suggester = RuleSuggester::new(vfs, chat, "test-model");

        let suggestions = suggester.scan().await.unwrap();
        assert!(suggestions.is_empty());
    }

    // ── scan: enough entries returns suggestions ────────────────────

    #[tokio::test]
    async fn test_scan_with_entries_returns_suggestions() {
        let vfs = Arc::new(MockVfs::new());
        let pattern_uri = make_uri(ContextNamespace::Agent, &["patterns"]);
        let entry1 = make_uri(ContextNamespace::Agent, &["patterns", "pat1"]);
        let entry2 = make_uri(ContextNamespace::Agent, &["patterns", "pat2"]);

        vfs.set_exists(&pattern_uri, true);
        vfs.add_entry(&pattern_uri, &entry1);
        vfs.add_entry(&pattern_uri, &entry2);
        vfs.set_content(&entry1, ContentLevel::Abstract, "Pattern A");
        vfs.set_content(&entry2, ContentLevel::Abstract, "Pattern B");

        let chat = mock_chat_json("{}");
        let suggester = RuleSuggester::new(vfs, chat, "test-model");

        let suggestions = suggester.scan().await.unwrap();
        assert_eq!(
            suggestions.len(),
            1,
            "should find 1 category with >= 2 entries"
        );
        assert_eq!(suggestions[0].source_category, "pattern");
        assert_eq!(suggestions[0].source_count, 2);
    }

    // ── scan: below threshold (< MIN_OCCURRENCES) ──────────────────

    #[tokio::test]
    async fn test_scan_below_threshold_returns_empty() {
        let vfs = Arc::new(MockVfs::new());
        let failed_uri = make_uri(ContextNamespace::Memory, &["cases", "failed_tasks"]);
        let entry1 = make_uri(ContextNamespace::Memory, &["cases", "failed_tasks", "f1"]);

        vfs.set_exists(&failed_uri, true);
        vfs.add_entry(&failed_uri, &entry1);
        vfs.set_content(&entry1, ContentLevel::Abstract, "Failed task 1");

        let chat = mock_chat_json("{}");
        let suggester = RuleSuggester::new(vfs, chat, "test-model");

        let suggestions = suggester.scan().await.unwrap();
        assert!(
            suggestions.is_empty(),
            "1 entry is below MIN_OCCURRENCES (2)"
        );
    }

    // ── promote_to_rule: has_pattern=true writes rule ───────────────

    #[tokio::test]
    async fn test_promote_to_rule_with_pattern() {
        let vfs = Arc::new(MockVfs::new());
        let learned_uri = AgentPath::Learned.uri();
        vfs.set_exists(&learned_uri, true);
        // Learned dir is empty → no dedup will block

        let response = serde_json::json!({
            "has_pattern": true,
            "rule_abstract": "Always validate input.",
            "rule_detail": "Check input before processing to avoid errors."
        })
        .to_string();

        let chat = mock_chat_json(&response);
        let suggester = RuleSuggester::new(vfs, chat, "test-model");

        let suggestion = RuleSuggestion {
            source_category: "failed_case".to_string(),
            source_count: 3,
            source_contents: vec![
                "Fail 1".to_string(),
                "Fail 2".to_string(),
                "Fail 3".to_string(),
            ],
        };

        // Should call promote_to_rule → LLM → record_with_kind → create_file + write
        let result = suggester.promote_to_rule(&suggestion, "session-1").await;
        assert!(result.is_ok(), "promote_to_rule should succeed");
    }

    // ── promote_to_rule: has_pattern=false skips write ──────────────

    #[tokio::test]
    async fn test_promote_to_rule_no_pattern_skips_recording() {
        let vfs = Arc::new(MockVfs::new());
        let response = serde_json::json!({"has_pattern": false}).to_string();
        let chat = mock_chat_json(&response);
        let suggester = RuleSuggester::new(vfs, chat, "test-model");

        let suggestion = RuleSuggestion {
            source_category: "pattern".to_string(),
            source_count: 2,
            source_contents: vec!["Pat 1".to_string(), "Pat 2".to_string()],
        };

        // Should return Ok without writing anything
        let result = suggester.promote_to_rule(&suggestion, "session-1").await;
        assert!(result.is_ok());
    }
}
