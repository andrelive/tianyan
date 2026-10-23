//! `providers` 组（ADR-049 期 4b）：**模型 provider 实现**插槽。
//!
//! 两个插槽（聊天/视觉 + 嵌入）由**同一套客户端构造语义**产出（UA / 方言 / 双预算
//! 超时 / 动态会话头 / 重试策略都在 `plugins/provider-client`），产出的两份能力集合由
//! 本库拼成同一个 [`crate::ModelServices`] 容器、在同一次热重载里一起重建，因此同组：
//! 换模型 provider 时两个插槽一起动。未装配时两份能力集合都落到 core 的「未配置」降级
//! 服务（调用即报“应用未配置”，前端据此显示配置向导）。

use super::{ProviderEmbedFactory, ProviderLlmFactory};

/// 模型 provider 工厂（`None` = 使用 core 的「未配置」降级服务 + 装配期告警）。
#[derive(Clone, Default)]
pub struct ProviderAssemblies {
    provider_llm_factory: Option<ProviderLlmFactory>,
    provider_embed_factory: Option<ProviderEmbedFactory>,
}

impl ProviderAssemblies {
    /// 装配聊天 / 视觉 provider 工厂（`app` / `tauri` 用：`tianyan-provider-llm`）。
    pub fn with_provider_llm_factory(mut self, factory: ProviderLlmFactory) -> Self {
        self.provider_llm_factory = Some(factory);
        self
    }

    /// 装配嵌入 provider 工厂（`app` / `tauri` 用：`tianyan-provider-embed`）。
    pub fn with_provider_embed_factory(mut self, factory: ProviderEmbedFactory) -> Self {
        self.provider_embed_factory = Some(factory);
        self
    }

    /// 聊天 / 视觉 provider 工厂（`None` = 使用 core 的「未配置」降级服务 + 装配期告警）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`ModelServices::
    /// from_factories`，经 `AppState::new` / `reload_agent` 调用）取用——装配层负责
    /// **注入**，不负责调用。
    pub(crate) fn llm_factory(&self) -> Option<ProviderLlmFactory> {
        self.provider_llm_factory.clone()
    }

    /// 嵌入 provider 工厂（`None` = 使用 core 的「未配置」降级服务 + 装配期告警）。
    pub(crate) fn embed_factory(&self) -> Option<ProviderEmbedFactory> {
        self.provider_embed_factory.clone()
    }
}
