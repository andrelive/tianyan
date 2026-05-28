//! Core 桥接模块
//!
//! 提供从 core 到 server 的类型转换和初始化辅助函数

use tianyan::Message;

use crate::api::shared::types::MessageRole as ApiMessageRole;
use tianyan::common::types::MessageRole as CoreMessageRole;

impl From<CoreMessageRole> for ApiMessageRole {
    /// 将核心消息角色转换为 API 角色。
    ///
    /// 注意：Tool 角色在 API 层映射为 Assistant，因为 API 消息模型不包含 Tool 变体。
    /// Tool 消息的内容会通过技能调用信息展示。
    fn from(role: CoreMessageRole) -> Self {
        match role {
            CoreMessageRole::System => ApiMessageRole::System,
            CoreMessageRole::User => ApiMessageRole::User,
            CoreMessageRole::Assistant => ApiMessageRole::Assistant,
            CoreMessageRole::Tool => ApiMessageRole::Assistant,
        }
    }
}

/// 转换消息类型
///
/// # Panics
///
/// 不会 panic，空内容会被保留原样（由上层验证处理）
pub fn convert_message(msg: &crate::api::ChatMessage) -> Message {
    use crate::api::MessageRole;
    use tianyan::MessageRole as CoreRole;

    let role = match msg.role {
        MessageRole::System => CoreRole::System,
        MessageRole::User => CoreRole::User,
        MessageRole::Assistant => CoreRole::Assistant,
    };

    Message {
        role,
        content: msg.content.clone(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }
}

/// 转换 Token 使用量
pub fn convert_token_usage(usage: tianyan::TokenUsage) -> crate::api::TokenUsage {
    crate::api::TokenUsage {
        prompt_tokens: usage.prompt_tokens as u32,
        completion_tokens: usage.completion_tokens as u32,
        total_tokens: usage.total_tokens as u32,
    }
}

/// 创建临时模型客户端（用于连接测试）
pub fn create_model_client(
    endpoint: &str,
    api_key: &str,
) -> anyhow::Result<tianyan::model::AsyncOpenAIClient> {
    let config =
        tianyan::model::ModelConfig::new(tianyan::model::types::ModelProvider::OpenAI, api_key)
            .with_name("test-connection")
            .with_base_url(endpoint)
            .with_chat_model("test")
            .with_timeout(30);

    tianyan::model::AsyncOpenAIClient::new(config).map_err(|e| anyhow::anyhow!("{}", e))
}
