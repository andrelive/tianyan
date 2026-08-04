//! Core 桥接模块
//!
//! 提供从 core 到 server 的类型转换和初始化辅助函数

use tianyan::model::types::ChatCompletionRequest;
use tianyan::model::ChatService;
use tianyan::Message;

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
