use std::sync::Arc;

use crate::common::error::Result;
use crate::model::traits::{ChatService, EmbeddingService, VlmService};

use super::provider::middleware::{LoggedEmbeddingService, LoggedService, LoggedVlmService};
use super::provider::AsyncOpenAIClient;
use super::config::ModelConfig;

/// 一组已构建的模型服务。
///
/// 将 chat / embedding / vision 三种服务打包在一起，
/// 替代了原来的 ModelRouter 路由中间层。
pub struct ModelServices {
    /// 聊天补全服务（含日志装饰）。
    pub chat: Arc<dyn ChatService>,
    /// 文本嵌入服务（含日志装饰）。
    pub embedding: Arc<dyn EmbeddingService>,
    /// 视觉分析服务（含日志装饰）。
    pub vision: Arc<dyn VlmService>,
    /// 默认聊天模型名。
    pub chat_model: String,
    /// 默认嵌入模型名。
    pub embedding_model: Option<String>,
    /// 默认视觉模型名。
    pub vision_model: Option<String>,
}

impl ModelServices {
    /// 从配置构建一组服务。
    ///
    /// 每个配置创建一个 `AsyncOpenAIClient`，
    /// 第一次调用的配置会被设为默认值。
    pub async fn from_configs(configs: Vec<ModelConfig>) -> Result<Self> {
        let mut first_chat: Option<Arc<dyn ChatService>> = None;
        let mut first_embedding: Option<Arc<dyn EmbeddingService>> = None;
        let mut first_vision: Option<Arc<dyn VlmService>> = None;
        let mut chat_model = String::new();
        let mut embedding_model = None;
        let mut vision_model = None;

        for config in configs {
            if !config.enabled {
                continue;
            }

            let client = AsyncOpenAIClient::new(config.clone())?;

            let chat: Arc<dyn ChatService> = Arc::new(LoggedService(client.clone()));
            let embedding: Arc<dyn EmbeddingService> = Arc::new(LoggedEmbeddingService(client.clone()));
            let vision: Arc<dyn VlmService> = Arc::new(LoggedVlmService(client));

            if first_chat.is_none() {
                first_chat = Some(chat.clone());
                first_embedding = Some(embedding.clone());
                first_vision = Some(vision);
                chat_model = config.chat_model;
                embedding_model = config.embedding_model;
                vision_model = config.vision_model;
            } else {
                tracing::warn!(
                    "多余的模型服务配置被忽略。在本地单模型场景下，只注册第一个服务。被忽略的服务: {}",
                    config.name
                );
            }
        }

        Ok(Self {
            chat: first_chat.ok_or_else(|| {
                crate::common::error::TianyanError::Config("没有启用的模型服务".into())
            })?,
            embedding: first_embedding.ok_or_else(|| {
                crate::common::error::TianyanError::Config("没有启用的嵌入服务".into())
            })?,
            vision: first_vision.ok_or_else(|| {
                crate::common::error::TianyanError::Config("没有启用的视觉服务".into())
            })?,
            chat_model,
            embedding_model,
            vision_model,
        })
    }
}
