use std::collections::HashMap;
use std::sync::Arc;

use crate::common::error::{Result, TianyanError};
use crate::config::{find_provider, ModelCapability, ModelRef, ModelsConfig};
use crate::model::traits::{ChatService, EmbeddingService, VlmService};

use super::provider::middleware::{LoggedEmbeddingService, LoggedService, LoggedVlmService};
use super::provider::AsyncOpenAIClient;

/// 一组已构建的模型服务。
///
/// 将 chat / embedding / vision 三种服务打包在一起，
/// 每种能力可能来自不同的提供商。
#[derive(Clone)]
pub struct ModelServices {
    /// 聊天补全服务（含日志装饰）。
    pub chat: Arc<dyn ChatService>,
    /// 文本嵌入服务（含日志装饰）。
    pub embedding: Arc<dyn EmbeddingService>,
    /// 视觉分析服务（含日志装饰）。
    pub vision: Arc<dyn VlmService>,
    /// 聊天模型名。
    pub chat_model: String,
    /// 嵌入模型名。
    pub embedding_model: Option<String>,
    /// 视觉模型名。
    pub vision_model: Option<String>,
}

/// 缓存的 provider 客户端映射。
///
/// key = provider name，确保同一 provider 只创建一个 HTTP client 实例。
type ClientPool = HashMap<String, AsyncOpenAIClient>;

impl ModelServices {
    /// 从 ModelsConfig 构建一组服务。
    ///
    /// 流程：
    /// 1. 按能力 resolve 出 (provider, model) 对
    /// 2. 为每个用到的 provider 创建（或复用）AsyncOpenAIClient
    /// 3. 从 client 构造对应 trait object（Logged 包装）
    pub async fn from_config(config: &ModelsConfig) -> Result<Self> {
        // 解析每个能力需要的模型
        let chat_ref = config.resolve(ModelCapability::Chat).ok_or_else(|| {
            TianyanError::config("未找到可用的聊天模型（需要 chat 能力标签）")
        })?;
        let embedding_ref = config
            .resolve(ModelCapability::TextEmbedding)
            .or_else(|| config.resolve(ModelCapability::MultimodalEmbedding))
            .ok_or_else(|| {
                TianyanError::config(
                    "未找到可用的嵌入模型（需要 text-embedding 或 multimodal-embedding 能力标签）",
                )
            })?;
        let vision_ref = config.resolve(ModelCapability::Vision).ok_or_else(|| {
            TianyanError::config("未找到可用的视觉模型（需要 vision 能力标签）")
        })?;

        // 按需创建客户端（同一 provider 复用）
        let mut clients = ClientPool::new();

        let get_or_create_client = |clients: &mut ClientPool,
                                    r: &ModelRef|
         -> Result<AsyncOpenAIClient> {
            if let Some(c) = clients.get(&r.provider) {
                return Ok(c.clone());
            }
            let provider = find_provider(&config.providers, &r.provider).ok_or_else(|| {
                TianyanError::config(format!("提供商 '{}' 未找到或未启用", r.provider))
            })?;
            let client = AsyncOpenAIClient::from_provider(provider)?;
            clients.insert(r.provider.clone(), client.clone());
            Ok(client)
        };

        let chat_client = get_or_create_client(&mut clients, &chat_ref)?;
        let embedding_client = get_or_create_client(&mut clients, &embedding_ref)?;
        let vision_client = get_or_create_client(&mut clients, &vision_ref)?;

        let chat: Arc<dyn ChatService> = Arc::new(LoggedService(chat_client));
        let embedding: Arc<dyn EmbeddingService> =
            Arc::new(LoggedEmbeddingService(embedding_client));
        let vision: Arc<dyn VlmService> = Arc::new(LoggedVlmService(vision_client));

        tracing::info!(
            chat_provider = %chat_ref.provider,
            chat_model = %chat_ref.model,
            embedding_provider = %embedding_ref.provider,
            embedding_model = %embedding_ref.model,
            vision_provider = %vision_ref.provider,
            vision_model = %vision_ref.model,
            "模型服务构建完成"
        );

        Ok(Self {
            chat,
            embedding,
            vision,
            chat_model: chat_ref.model,
            embedding_model: Some(embedding_ref.model),
            vision_model: Some(vision_ref.model),
        })
    }
}
