//! 模型服务容器（ADR-049 期 2 序 6：从 `core/src/model/services.rs` 搬进装配层）。
//!
//! ## 为什么搬出 core
//!
//! 这个类型（232 行）在 core 里**零生产使用者**：core 内只有 `model/mod.rs` 的导出与
//! 一处注释引用；真正的消费者是通道层——[`crate::agent_builder`] 的
//! [`create_model_services`] 与 [`crate::state::AppState::shared_model_services`]
//! （检索、摘要、角色路由、知识导入、后台任务、演化任务都从这里取服务）。
//!
//! 按 ADR-049 D5（装配器二分）它是**装配职责**，不是内核机制：
//!
//! 1. 它做的三件事——「按能力解析 provider/model」「建每 provider 的客户端池」「把
//!    三件服务打包」——都是装配决策；
//! 2. 三种能力今天分属**两个插件**（`tianyan-provider-llm` 出聊天 + 视觉，
//!    `tianyan-provider-embed` 出嵌入），容器只能活在认识这两个插槽的地方；
//! 3. 若留在 core，core 就得认识装配插槽（反向依赖）；若进任一个插件，另一个插件与
//!    通道层就得依赖它（破 D2-1）。
//!
//! 结果：core 不再有「容器 + 构造器」概念，只留 S 级契约（`ChatService` /
//! `EmbeddingService` / `VlmService`）、载荷类型、模型规格与重试策略。
//!
//! ## 行为零变化
//!
//! 能力解析、客户端池复用、降级服务、`chat_for` 的路由回落、`set_embedding_usage_sink`
//! 的后置注入、「部分模型能力未配置」的告警文案——逐字保留（唯一差异是代码位置）。

use std::collections::HashMap;
use std::sync::Arc;

use tianyan::common::error::Result as TianyanResult;
use tianyan::config::ModelsConfig;
use tianyan::model::{
    ChatService, ConnectionProbe, EmbeddingService, EmbeddingUsageHandle, EmbeddingUsageSink,
    ProviderEmbedServices, ProviderLlmServices, SharedChatService, VlmService,
};

use crate::assembly::{ProviderEmbedFactory, ProviderLlmFactory};

/// 一组已构建的模型服务。
///
/// 将 chat / embedding / vision 三种服务打包在一起，每种能力可能来自不同的提供商。
/// （迁移前在 `tianyan::model::ModelServices`，ADR-049 期 2 序 6 起在装配层。）
#[derive(Clone)]
pub struct ModelServices {
    /// 聊天补全服务（含日志装饰）。
    pub chat: Arc<dyn ChatService>,
    /// 文本嵌入服务（含日志装饰 + 用量上报 + 查询缓存）。
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
    /// 连通性探针（配置页「测试连接」；`None` = 未装配 provider-llm 工厂）。
    ///
    /// 契约在 core（`tianyan::model::ConnectionProbe`），实现由 `tianyan-provider-llm`
    /// 随能力集合一起产出——探测要用**同一套**客户端构造语义造临时客户端。
    probe: Option<Arc<dyn ConnectionProbe>>,
}

impl ModelServices {
    /// 由两个 provider 插槽的工厂拼装（**唯一的模型实现选择点**）。
    ///
    /// `None`（未装配）时的降级语义与迁移前 `ModelServices::from_config` 在「能力未配置」
    /// 时一致：给 core 的「未配置」降级服务（`Unconfigured{Chat,Embedding,Vlm}Service`，
    /// 调用即报“应用未配置”），服务端照常启动（前端显示配置向导），并 `warn!` 一次指路。
    ///
    /// 为什么允许降级而不 `Err`：`create_app`（构造对象图，约 60 处库单测 + 嵌入式调用）
    /// 走 `AssemblyServices::default()`，硬失败等于删入口；而生产装配（`app` / `tauri` /
    /// 集成测试 `test_assembly()`）都已注入两个工厂——覆盖的是生产形态。
    pub(crate) fn from_factories(
        config: &ModelsConfig,
        llm_factory: Option<&ProviderLlmFactory>,
        embed_factory: Option<&ProviderEmbedFactory>,
    ) -> TianyanResult<Self> {
        let llm = match llm_factory {
            Some(factory) => factory(config)?,
            None => {
                tracing::warn!(
                    "未装配 provider-llm 工厂：聊天与视觉使用**降级服务**（调用即报“应用未配置”，\
                     会话、摘要、自审等全链路不可用）——生产装配请用 create_app_with_assembly / \
                     start_server_with_assembly"
                );
                ProviderLlmServices::unconfigured()
            }
        };
        let embed = match embed_factory {
            Some(factory) => factory(config)?,
            None => {
                tracing::warn!(
                    "未装配 provider-embed 工厂：嵌入使用**降级服务**（检索、向量写入、角色路由\
                     不可用）——生产装配请用 create_app_with_assembly / start_server_with_assembly"
                );
                ProviderEmbedServices::unconfigured()
            }
        };

        // 迁移前 `from_config` 的告警逐字保留（字段名与文案未改）：三条能力各自是否配置。
        let chat_configured = !llm.chat_model.is_empty();
        let embedding_configured = embed.embedding_model.is_some();
        let vision_configured = llm.vision_model.is_some();
        if !chat_configured || !embedding_configured || !vision_configured {
            tracing::warn!(
                chat_configured,
                embedding_configured,
                vision_configured,
                "部分模型能力未配置，使用降级服务（应用可启动，缺失能力调用将报'未配置'）"
            );
        }

        Ok(Self {
            chat: llm.chat,
            embedding: embed.embedding,
            vision: llm.vision,
            chat_model: llm.chat_model,
            embedding_model: embed.embedding_model,
            vision_model: llm.vision_model,
            embedding_usage_sink: embed.usage_handle,
            chat_clients: llm.chat_clients,
            probe: llm.probe,
        })
    }

    /// 注入嵌入用量上报（装配层拿到 `UsageLog` 后调用；未注入则不上报）。
    ///
    /// 后置注入而非构造参数：构造只依赖 `ModelsConfig`，而用量落库需要 `UsageLog`
    /// （数据库连接）——provider 插件不依赖 observability/db。
    pub fn set_embedding_usage_sink(&self, sink: Arc<dyn EmbeddingUsageSink>) {
        *self
            .embedding_usage_sink
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(sink);
    }

    /// 每 provider 的聊天客户端池（装配层注入 `AgentLoop` 的会话级路由面）。
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

    /// 连通性探针（`None` = 未装配 provider-llm 工厂）。
    pub(crate) fn probe(&self) -> Option<Arc<dyn ConnectionProbe>> {
        self.probe.clone()
    }
}

/// 根据配置创建模型服务（**装配层唯一的容器构造点**）。
///
/// 两个 provider 插槽的工厂由装配层注入（`app` / `tauri` / 集成测试 `test_assembly()`），
/// 本函数只负责「取工厂 → 拼容器」——不认识任何具体 provider 实现（D5）。
pub fn create_model_services(
    config: &tianyan::config::TianyanConfig,
    assembly: &crate::assembly::AssemblyServices,
) -> TianyanResult<ModelServices> {
    ModelServices::from_factories(
        &config.models,
        assembly.provider_llm_factory().as_ref(),
        assembly.provider_embed_factory().as_ref(),
    )
}
