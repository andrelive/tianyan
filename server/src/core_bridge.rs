//! Core 桥接模块
//!
//! 提供从 core 到 server 的类型转换和初始化辅助函数

use tianyan::model::types::ChatCompletionRequest;
use tianyan::model::ChatService;
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
) -> tianyan::common::error::Result<tianyan::model::AsyncOpenAIClient> {
    tianyan::model::AsyncOpenAIClient::new("test-connection", endpoint, api_key, 30)
}

/// 测试模型连接。
///
/// 创建一个临时客户端并发送一个简单的聊天请求来验证连接是否成功。
pub async fn test_model_connection(
    endpoint: &str,
    api_key: &str,
    model: &str,
) -> Result<Vec<String>, String> {
    let client =
        create_model_client(endpoint, api_key).map_err(|e| format!("创建客户端失败: {}", e))?;

    let request = ChatCompletionRequest::new(
        model.to_string(),
        vec![Message::system("You are a helpful assistant.")],
    )
    .with_temperature(0.7)
    .with_max_tokens(10)
    .with_stream(false)
    .with_enable_thinking(false);

    match client.chat_completion(request).await {
        Ok(_) => Ok(vec![model.to_string()]),
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("401") || error_msg.contains("unauthorized") {
                Err("API 密钥无效或已过期".to_string())
            } else if error_msg.contains("404") {
                Err("模型不存在，请检查模型名称".to_string())
            } else if error_msg.contains("timeout") {
                Err("连接超时，请检查网络或 API 端点".to_string())
            } else {
                Err(format!("连接失败: {}", error_msg))
            }
        }
    }
}
