use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::common::error::{Result, TianyanError};
use crate::config::{find_provider, ModelCapability, ModelsConfig};
use crate::model::traits::{ChatService, EmbeddingService, VlmService};
use crate::model::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest,
    EmbeddingResponse, VisionRequest, VisionResponse,
};

use super::provider::middleware::{
    EmbeddingUsageHandle, EmbeddingUsageSink, LoggedEmbeddingService, LoggedService,
    LoggedVlmService,
};
use super::provider::AsyncOpenAIClient;
use super::SharedChatService;

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
    /// 嵌入用量上报槽位（装配层后置注入；见 [`ModelServices::set_embedding_usage_sink`]）。
    embedding_usage_sink: EmbeddingUsageHandle,
    /// 每 provider 的聊天客户端池（会话级模型选择的路由面；key = provider name）。
    ///
    /// 未命中的 provider 在路由时回落默认聊天服务（`chat`）。
    chat_clients: HashMap<String, SharedChatService>,
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
        // 按能力独立解析：缺失的能力用降级服务（服务端可启动，前端显示配置向导）
        let chat_ref = config.resolve(ModelCapability::Chat);
        let embedding_ref = config
            .resolve(ModelCapability::TextEmbedding)
            .or_else(|| config.resolve(ModelCapability::MultimodalEmbedding));
        let vision_ref = config.resolve(ModelCapability::Vision);

        // 嵌入用量上报槽位（与 LoggedEmbeddingService 共享；装配层后置注入）
        let embedding_usage_sink: EmbeddingUsageHandle = Arc::new(Mutex::new(None));

        // 按需创建客户端（同一 provider 复用）
        let mut clients = ClientPool::new();

        let get_or_create_client = |clients: &mut ClientPool,
                                    provider_name: &str|
         -> Result<AsyncOpenAIClient> {
            if let Some(c) = clients.get(provider_name) {
                return Ok(c.clone());
            }
            let provider = find_provider(&config.providers, provider_name).ok_or_else(|| {
                TianyanError::config(format!("提供商 '{}' 未找到或未启用", provider_name))
            })?;
            let client = AsyncOpenAIClient::from_provider(provider)?;
            clients.insert(provider_name.to_string(), client.clone());
            Ok(client)
        };

        let (chat, chat_model) = match &chat_ref {
            Some(r) => {
                let client = get_or_create_client(&mut clients, &r.provider)?;
                (
                    Arc::new(LoggedService(client)) as Arc<dyn ChatService>,
                    r.model.clone(),
                )
            }
            None => (
                Arc::new(UnconfiguredChatService) as Arc<dyn ChatService>,
                String::new(),
            ),
        };
        let (embedding, embedding_model) = match &embedding_ref {
            Some(r) => {
                let client = get_or_create_client(&mut clients, &r.provider)?;
                (
                    Arc::new(LoggedEmbeddingService::new(
                        client,
                        embedding_usage_sink.clone(),
                    )) as Arc<dyn EmbeddingService>,
                    Some(r.model.clone()),
                )
            }
            None => (
                Arc::new(UnconfiguredEmbeddingService) as Arc<dyn EmbeddingService>,
                None,
            ),
        };
        let (vision, vision_model) = match &vision_ref {
            Some(r) => {
                let client = get_or_create_client(&mut clients, &r.provider)?;
                (
                    Arc::new(LoggedVlmService(client)) as Arc<dyn VlmService>,
                    Some(r.model.clone()),
                )
            }
            None => (
                Arc::new(UnconfiguredVlmService) as Arc<dyn VlmService>,
                None,
            ),
        };

        // 每 provider 的聊天客户端池（会话级模型选择路由）：对全部"声明了 Chat
        // 能力模型"的已启用 provider 预建（数量少、构建便宜）；单个失败跳过并
        // 告警，路由时未命中的 provider 回落默认聊天服务。
        let mut chat_clients: HashMap<String, SharedChatService> = HashMap::new();
        for provider in &config.providers {
            if !provider.enabled
                || !provider
                    .models
                    .iter()
                    .any(|m| m.capabilities.contains(&ModelCapability::Chat))
            {
                continue;
            }
            match get_or_create_client(&mut clients, &provider.name) {
                Ok(client) => {
                    chat_clients.insert(provider.name.clone(), Arc::new(LoggedService(client)));
                }
                Err(e) => {
                    tracing::warn!(
                        provider = %provider.name,
                        error = %e,
                        "聊天客户端构建失败（该 provider 的会话级模型选择将回落默认）"
                    );
                }
            }
        }

        if chat_ref.is_none() || embedding_ref.is_none() || vision_ref.is_none() {
            tracing::warn!(
                chat_configured = chat_ref.is_some(),
                embedding_configured = embedding_ref.is_some(),
                vision_configured = vision_ref.is_some(),
                "部分模型能力未配置，使用降级服务（应用可启动，缺失能力调用将报'未配置'）"
            );
        }

        Ok(Self {
            chat,
            embedding,
            vision,
            chat_model,
            embedding_model,
            vision_model,
            embedding_usage_sink,
            chat_clients,
        })
    }

    /// 注入嵌入用量上报（装配层拿到 `UsageLog` 后调用；未注入则不上报）。
    ///
    /// 后置注入而非构造参数：`from_config` 只依赖 `ModelsConfig`，而用量落库
    /// 需要 `UsageLog`（数据库连接）——保持 model 层不依赖 observability/db。
    pub fn set_embedding_usage_sink(&self, sink: Arc<dyn EmbeddingUsageSink>) {
        *self
            .embedding_usage_sink
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(sink);
    }

    /// 每 provider 的聊天客户端池（装配层注入 AgentLoop 的会话级路由面）。
    pub fn chat_clients(&self) -> HashMap<String, SharedChatService> {
        self.chat_clients.clone()
    }

    /// 按提供商选择聊天服务（**会话级模型选择的路由单点**）。
    ///
    /// `None` 或未命中（该 provider 未构建成功 / 未声明 Chat 能力）→ 回落默认
    /// 聊天服务（`preferences.chat` 的客户端）。
    pub fn chat_for(&self, provider: Option<&str>) -> SharedChatService {
        provider
            .and_then(|p| self.chat_clients.get(p))
            .cloned()
            .unwrap_or_else(|| self.chat.clone())
    }
}

// ── 未配置降级服务 ────────────────────────────────────────────────────────
//
// 未配置对应模型能力时（首次启动/配置缺失），from_config 返回降级服务：
// 服务端仍能启动（前端显示配置向导），任何模型调用返回"应用未配置"错误。

fn unconfigured_error() -> TianyanError {
    TianyanError::config("应用未配置。请先完成配置向导。")
}

/// 未配置时的聊天服务（调用即报错）。
struct UnconfiguredChatService;

#[async_trait]
impl ChatService for UnconfiguredChatService {
    async fn chat_completion(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        Err(unconfigured_error())
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<ChatCompletionChunk>>> {
        Err(unconfigured_error())
    }
}

/// 未配置时的嵌入服务（调用即报错）。
struct UnconfiguredEmbeddingService;

#[async_trait]
impl EmbeddingService for UnconfiguredEmbeddingService {
    async fn embed(&self, _request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        Err(unconfigured_error())
    }

    fn embedding_dimension(&self, _model: &str) -> usize {
        0
    }
}

/// 未配置时的视觉服务（调用即报错）。
struct UnconfiguredVlmService;

#[async_trait]
impl VlmService for UnconfiguredVlmService {
    async fn analyze_image(&self, _request: VisionRequest) -> Result<VisionResponse> {
        Err(unconfigured_error())
    }
}
