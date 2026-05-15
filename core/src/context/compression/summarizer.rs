//! 对话摘要生成器。
//!
//! 使用 LLM 将对话历史压缩为简洁的摘要，保留关键信息。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::Message;
use crate::model::ModelService;

/// 摘要配置。
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    /// 使用的模型。
    pub model: String,
    /// 最大摘要长度（token 数）。
    pub max_summary_tokens: usize,
    /// 摘要提示词模板。
    pub prompt_template: String,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o-mini".to_string(),
            max_summary_tokens: 500,
            prompt_template: DEFAULT_SUMMARY_PROMPT.to_string(),
        }
    }
}

/// 默认摘要提示词。
const DEFAULT_SUMMARY_PROMPT: &str = r#"请将以下对话历史压缩为简洁的摘要。要求：

1. 保留所有重要的事实、决策和结论
2. 保留用户明确表达的需求和偏好
3. 保留已完成的任务和取得的结果
4. 保留遇到的错误和解决方案
5. 删除重复信息和闲聊内容
6. 使用第三人称客观描述

对话历史：
{conversation}

请输出简洁的结构化摘要（不超过 500 字）："#;

/// 对话摘要生成器。
#[derive(Clone)]
pub struct ConversationSummarizer {
    model_service: Arc<dyn ModelService>,
    config: SummaryConfig,
}

impl ConversationSummarizer {
    /// 创建新的摘要生成器。
    pub fn new(model_service: Arc<dyn ModelService>, config: SummaryConfig) -> Self {
        Self {
            model_service,
            config,
        }
    }

    /// 生成对话摘要。
    ///
    /// - `messages` - 需要摘要的对话消息
    pub async fn summarize(&self, messages: &[Message]) -> Result<String> {
        if messages.is_empty() {
            return Ok(String::new());
        }

        let conversation = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = self
            .config
            .prompt_template
            .replace("{conversation}", &conversation);

        let response = self
            .model_service
            .chat(&self.config.model, vec![Message::user(prompt)])
            .await?;

        Ok(response.trim().to_string())
    }

    /// 增量摘要：基于已有摘要和新消息生成更新后的摘要。
    ///
    /// - `existing_summary` - 已有摘要
    /// - `new_messages` - 新消息
    pub async fn incremental_summarize(
        &self,
        existing_summary: &str,
        new_messages: &[Message],
    ) -> Result<String> {
        if new_messages.is_empty() {
            return Ok(existing_summary.to_string());
        }

        let new_conversation = new_messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            r#"请将以下新对话内容合并到已有摘要中，生成更新后的摘要。

已有摘要：
{}

新对话内容：
{}

要求：
1. 保留已有摘要中的所有重要信息
2. 将新对话中的关键信息合并进去
3. 删除重复内容
4. 保持简洁，不超过 500 字

请输出更新后的摘要："#,
            existing_summary, new_conversation
        );

        let response = self
            .model_service
            .chat(&self.config.model, vec![Message::user(prompt)])
            .await?;

        Ok(response.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary_config_default() {
        let config = SummaryConfig::default();
        assert!(!config.model.is_empty());
        assert_eq!(config.max_summary_tokens, 500);
    }
}
