//! 记忆提取服务。
//!
//! 本模块提供从会话中自动提取记忆的功能，使用 LLM 分析对话内容并生成结构化记忆。
//! 提取的记忆通过 VFS 持久化，并自动更新向量摘要，支持后续的语义检索。

use std::sync::Arc;

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, MemoryCategory, MemoryEntry, TianyanUri};
use crate::model::ChatService;
use crate::storage::VirtualFileSystem;

/// 默认记忆提取服务使用的模型。
const DEFAULT_MEMORY_EXTRACTION_MODEL: &str = "gpt-4";

/// 记忆提取服务 trait。
///
/// 定义从会话中自动提取结构化记忆的能力，支持依赖反转。
#[async_trait]
pub trait MemoryExtractionTrait: Send + Sync {
    /// 从会话内容中提取并保存记忆。
    ///
    /// - `conversation` - 对话内容
    /// - `session_id` - 会话标识
    /// - returns: 存储成功的记忆条目列表
    async fn extract_and_store(
        &self,
        conversation: &str,
        session_id: &str,
    ) -> Result<Vec<MemoryEntry>>;

    /// 从会话 URI 中提取记忆。
    ///
    /// - `session_uri` - 会话的 Tianyan URI
    /// - returns: 提取的记忆条目列表
    async fn extract_from_session(&self, session_uri: &TianyanUri) -> Result<Vec<MemoryEntry>>;
}

/// 记忆提取服务配置。
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    /// 用于提取的模型名称。
    pub model: String,
    /// 提取提示词模板。
    pub prompt_template: String,
    /// 最小对话长度才触发提取（字符数）。
    pub min_conversation_length: usize,
    /// 提取的最大记忆数量。
    pub max_extracted_memories: usize,
    /// 最小重要性阈值（低于此值不存储）。
    pub min_importance_threshold: f32,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MEMORY_EXTRACTION_MODEL.to_string(),
            prompt_template: DEFAULT_EXTRACTION_PROMPT.to_string(),
            min_conversation_length: 50,
            max_extracted_memories: 10,
            min_importance_threshold: 0.3,
        }
    }
}

/// 增强的提取提示词。
///
/// 指导 LLM 从对话中提取结构化记忆，分类存储到不同命名空间。
const DEFAULT_EXTRACTION_PROMPT: &str = r#"分析以下对话，提取值得长期保存的结构化记忆。

对话内容：
{conversation}

请提取以下类别的记忆（仅提取重要、持久的信息）：

1. **preferences** - 用户偏好（工作习惯、格式偏好、沟通风格等）
2. **decisions** - 重要决策（用户做出的选择、确认的方案等）
3. **facts** - 事实信息（关于用户、项目、环境的事实）
4. **entities** - 实体信息（提到的人、项目、工具、组织等）
5. **patterns** - 模式（重复出现的工作模式、代码风格等）
6. **successful_cases** - 成功案例（有效的问题解决方法）
7. **failed_cases** - 失败教训（需要避免的错误）

请以 JSON 格式返回，结构如下：
{
  "memories": [
    {
      "id": "唯一标识符（使用小写英文和连字符）",
      "category": "类别名称（preference/decision/fact/entity/pattern/successful_case/failed_case）",
      "content": "记忆的简洁描述",
      "importance": 0.0-1.0,
      "tags": ["标签1", "标签2"]
    }
  ]
}

提取原则：
- 只提取跨会话有价值的信息
- 避免提取临时性、一次性的信息
- 重要性评分：0.9+ 非常关键，0.7-0.9 重要，0.4-0.7 一般，0.4以下不提取
- 每个记忆应该简洁明了，不超过100字
- 最多提取 {max_memories} 条记忆"#;

/// 记忆提取服务。
///
/// 使用 LLM 分析对话内容，自动提取结构化记忆，并通过 VFS 持久化。
pub struct MemoryExtractionService {
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    config: ExtractionConfig,
}

impl MemoryExtractionService {
    /// 创建新的记忆提取服务。
    pub fn new(
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        config: ExtractionConfig,
    ) -> Self {
        Self {
            model_service,
            vfs,
            config,
        }
    }

    /// 使用默认配置创建服务。
    pub fn with_defaults(
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
    ) -> Self {
        Self::new(model_service, vfs, ExtractionConfig::default())
    }

    /// 从会话内容中提取并保存记忆。
    ///
    /// - `conversation` - 对话内容
    /// - `session_id` - 会话 ID（用于标记记忆来源）
    pub async fn extract_and_store(
        &self,
        conversation: &str,
        session_id: &str,
    ) -> Result<Vec<MemoryEntry>> {
        if conversation.len() < self.config.min_conversation_length {
            return Ok(Vec::new());
        }

        let memories = self.extract_memories(conversation).await?;
        let mut stored = Vec::new();

        for mut memory in memories {
            // 过滤低重要性记忆
            if memory.importance < self.config.min_importance_threshold {
                continue;
            }

            memory.source_session = Some(session_id.to_string());

            // 通过 VFS 持久化
            if let Err(e) = self.store_memory(&memory).await {
                tracing::warn!(memory_id = %memory.id, error = %e, "存储记忆失败");
                continue;
            }

            stored.push(memory);
        }

        tracing::info!(count = stored.len(), session_id, "记忆提取完成");
        Ok(stored)
    }

    /// 从会话内容中提取记忆（不保存）。
    pub async fn extract_memories(&self, conversation: &str) -> Result<Vec<MemoryEntry>> {
        let prompt = self
            .config
            .prompt_template
            .replace("{conversation}", conversation)
            .replace(
                "{max_memories}",
                &self.config.max_extracted_memories.to_string(),
            );

        let response = self
            .model_service
            .chat(
                &self.config.model,
                vec![crate::common::types::Message::user(prompt)],
            )
            .await?;

        self.parse_extraction_response(&response)
    }

    /// 从会话 URI 中提取记忆。
    pub async fn extract_from_session(&self, session_uri: &TianyanUri) -> Result<Vec<MemoryEntry>> {
        let content = self
            .vfs
            .read_content(session_uri, ContentLevel::Detail)
            .await?;

        self.extract_memories(&content).await
    }

    /// 将记忆存储到 VFS。
    ///
    /// 记忆存储在对应的命名空间中：
    /// - user/preferences/ - 用户偏好
    /// - user/entities/ - 用户实体
    /// - memory/facts/ - 环境事实
    /// - memory/events/decisions/ - 决策记录
    /// - memory/cases/ - 成功/失败案例
    /// - agent/patterns/ - 学习到的模式
    async fn store_memory(&self, memory: &MemoryEntry) -> Result<()> {
        let uri = &memory.uri;

        // 确保父目录存在
        if let Some(parent) = uri.parent() {
            if !self.vfs.exists(&parent).await? {
                self.vfs.create_directory(&parent).await?;
            }
        }

        // 将记忆序列化为 Markdown 格式
        let content = format_memory_as_markdown(memory);

        // 写入 VFS
        self.vfs.write_content(uri, &content).await?;

        // 写入摘要层（用于向量检索）
        let abstract_content = format!(
            "{} | 重要性: {:.2} | 类别: {}",
            memory.content, memory.importance, memory.category
        );
        self.vfs.write_abstract(uri, &abstract_content).await?;

        tracing::debug!(memory_id = %memory.id, uri = %uri, "记忆已持久化");
        Ok(())
    }

    /// 解析提取响应。
    fn parse_extraction_response(&self, response: &str) -> Result<Vec<MemoryEntry>> {
        let cleaned = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let json: serde_json::Value = match serde_json::from_str(cleaned) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, response = %response, "记忆提取响应 JSON 解析失败");
                return Ok(Vec::new());
            }
        };

        let mut memories = Vec::new();

        if let Some(memories_array) = json.get("memories").and_then(|v| v.as_array()) {
            for memory_json in memories_array
                .iter()
                .take(self.config.max_extracted_memories)
            {
                if let Some(entry) = self.parse_single_memory(memory_json) {
                    memories.push(entry);
                }
            }
        } else {
            // 兼容旧格式
            if let Ok(old_memories) = self.parse_legacy_format(&json) {
                memories.extend(old_memories);
            }
        }

        Ok(memories)
    }

    /// 解析单条记忆。
    fn parse_single_memory(&self, json: &serde_json::Value) -> Option<MemoryEntry> {
        let id = json.get("id")?.as_str()?;
        let category_str = json.get("category")?.as_str()?;
        let content = json.get("content")?.as_str()?;
        let importance = json.get("importance")?.as_f64()? as f32;

        let category = match category_str.to_lowercase().as_str() {
            "preference" | "preferences" => MemoryCategory::Preference,
            "decision" | "decisions" => MemoryCategory::Decision,
            "fact" | "facts" => MemoryCategory::Fact,
            "entity" | "entities" => MemoryCategory::Entity,
            "pattern" | "patterns" => MemoryCategory::Pattern,
            "successful_case" | "success" | "successful" => MemoryCategory::SuccessfulCase,
            "failed_case" | "failure" | "failed" => MemoryCategory::FailedCase,
            _ => MemoryCategory::Fact,
        };

        let mut entry = MemoryEntry::new(id, content, category);
        entry.importance = importance.clamp(0.0, 1.0);

        if let Some(tags) = json.get("tags").and_then(|v| v.as_array()) {
            entry.tags = tags
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect();
        }

        Some(entry)
    }

    /// 解析旧格式响应（兼容）。
    fn parse_legacy_format(&self, json: &serde_json::Value) -> Result<Vec<MemoryEntry>> {
        let mut memories = Vec::new();

        for (category_str, category) in [
            ("preferences", MemoryCategory::Preference),
            ("decisions", MemoryCategory::Decision),
            ("facts", MemoryCategory::Fact),
        ] {
            if let Some(items) = json.get(category_str).and_then(|v| v.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    if let Some(content) = item.get("content").and_then(|v| v.as_str()) {
                        let id = item
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("{}-{}", category_str, i));
                        let mut entry = MemoryEntry::new(&id, content, category);
                        if let Some(confidence) = item.get("confidence").and_then(|v| v.as_f64()) {
                            entry.importance = (confidence as f32).clamp(0.0, 1.0);
                        }
                        memories.push(entry);
                    }
                }
            }
        }

        Ok(memories)
    }
}

#[async_trait]
impl MemoryExtractionTrait for MemoryExtractionService {
    async fn extract_and_store(
        &self,
        conversation: &str,
        session_id: &str,
    ) -> Result<Vec<MemoryEntry>> {
        self.extract_and_store(conversation, session_id).await
    }

    async fn extract_from_session(&self, session_uri: &TianyanUri) -> Result<Vec<MemoryEntry>> {
        self.extract_from_session(session_uri).await
    }
}

/// 将记忆格式化为 Markdown。
fn format_memory_as_markdown(memory: &MemoryEntry) -> String {
    let mut md = String::new();

    md.push_str(&format!("# 记忆: {}\n\n", memory.id));
    md.push_str(&format!("- **类别**: {}\n", memory.category));
    md.push_str(&format!("- **重要性**: {:.2}\n", memory.importance));
    md.push_str(&format!("- **访问次数**: {}\n", memory.access_count));
    md.push_str(&format!(
        "- **创建时间**: {}\n",
        memory.created_at.to_rfc3339()
    ));
    md.push_str(&format!(
        "- **更新时间**: {}\n",
        memory.updated_at.to_rfc3339()
    ));

    if let Some(ref last_accessed) = memory.last_accessed {
        md.push_str(&format!("- **最后访问**: {}\n", last_accessed.to_rfc3339()));
    }

    if let Some(ref session_id) = memory.source_session {
        md.push_str(&format!("- **来源会话**: {}\n", session_id));
    }

    if !memory.tags.is_empty() {
        md.push_str(&format!("- **标签**: {}\n", memory.tags.join(", ")));
    }

    if !memory.related_memories.is_empty() {
        md.push_str(&format!(
            "- **相关记忆**: {}\n",
            memory.related_memories.join(", ")
        ));
    }

    md.push_str("\n## 内容\n\n");
    md.push_str(&memory.content);
    md.push('\n');

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extraction_config_default() {
        let config = ExtractionConfig::default();
        assert!(!config.model.is_empty());
        assert!(!config.prompt_template.is_empty());
        assert_eq!(config.max_extracted_memories, 10);
        assert_eq!(config.min_importance_threshold, 0.3);
    }

    #[test]
    fn test_format_memory_as_markdown() {
        let memory = MemoryEntry::new("test-1", "这是一个测试记忆", MemoryCategory::Preference)
            .with_importance(0.8)
            .with_tags(vec!["test".to_string(), "preference".to_string()]);

        let md = format_memory_as_markdown(&memory);
        assert!(md.contains("# 记忆: test-1"));
        assert!(md.contains("**类别**: preference"));
        assert!(md.contains("**重要性**: 0.80"));
        assert!(md.contains("这是一个测试记忆"));
    }
}
