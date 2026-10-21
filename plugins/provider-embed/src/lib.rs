//! provider-embed 插件：OpenAI 兼容 provider 的**文本嵌入实现**（ADR-049 期 2 序 6）。
//!
//! 迁入本 crate（含内联测试）：
//!
//! | 文件 | 原位置 | 内容 |
//! |---|---|---|
//! | [`embedding`]（私有） | `core/src/model/provider/embedding.rs` | 手工构造请求体 + **宽容**解析响应（DashScope 只回 `usage.total_tokens`）、方言驱动的 usage 归一 |
//! | [`logging`]（私有） | `core/src/model/provider/middleware.rs`（下半） | 日志装饰 + 用量上报 + 查询缓存（`model\|dimensions\|text`，FIFO 256） |
//!
//! ## 为什么搬（判据：变化原因，ADR-049 D7）
//!
//! 这里全是 **B 级**（ADR-049 D3）：嵌入请求体的 wire 字段、非标准 `usage` 形状的归一
//! 规则（`EmbeddingUsageShape` 的两种语义）、查询缓存的容量与键、日志文案。改这些不该
//! 重编内核——此前一次嵌入字段的调整要重编 core（42.4K）+ server（13.4K）。
//!
//! ## 为什么客户端**不在**本 crate（三 crate 切法）
//!
//! `impl EmbeddingService for AsyncOpenAIClient` 只能写在定义 `AsyncOpenAIClient` 的 crate
//! 里（trait 在 core、类型在 `tianyan-provider-client`，孤儿规则不允许在第三方 crate 里
//! 实现）。而客户端被聊天 / 视觉 / 嵌入三种能力共用，构造语义（UA 单点、方言解析、
//! 双预算超时、动态会话头、重试策略）**必须只有一份**。因此：
//!
//! - `tianyan-provider-client`：客户端 + `RetryableFailure` / `classify_http_status`（两个能力插件共用）；
//! - `tianyan-provider-llm` / `tianyan-provider-embed`：各自的能力实现，**互不依赖**（D2-2）。
//!
//! 详见 `plugins/provider-client/src/lib.rs` 的 crate 文档。
//!
//! ## 哪些类型留 core
//!
//! `EmbeddingService`（S 级契约）、`EmbeddingRequest` / `EmbeddingResponse` / `EmbeddingData`
//! （载荷）、`model::retry`（重试策略）、`model::types::embedding_dimension`（模型维度表）、
//! `config::{EmbeddingUsageShape, ModelCapability, ModelsConfig}`（配置形状）。
//! 用量上报契约 [`tianyan::model::EmbeddingUsageSink`] 也留 core：实现方是通道层
//! （`UsageLogEmbeddingSink` 写 `usage_logs`），消费方是本插件——契约必须住在两者之上的 core。
//!
//! ## `ModelServices` 不在这里
//!
//! 原 `core/src/model/services.rs` 的 `ModelServices` 是「容器 + 构造器」，按 D5 属装配职责，
//! 已搬到 `server/src/model_services.rs`（详见 `plugins/provider-llm` 的 crate 文档与
//! ADR-049 期 2 序 6 实施记录）。本 crate 只产出 [`ProviderEmbedServices`]，由装配层填进容器。
//!
//! ## 行为零变化
//!
//! 请求体（`model` / `input` / `dimensions`）、`Authorization` 与静态头、重试（与 chat 同一
//! `RetryPolicy` 与状态码语义分类）、`usage` 归一的两种方言语义、查询缓存的容量与键、
//! 空文本短路、日志文案——逐字未改（`git diff --no-index` 差异只有路径前缀与本文件头）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod embedding;
mod logging;

use std::sync::{Arc, Mutex};

use tianyan::common::error::{Result, TianyanError};
use tianyan::config::{find_provider, ModelCapability, ModelsConfig};
use tianyan::model::assembly::ProviderEmbedServices;
use tianyan::model::EmbeddingService;
use tianyan::model::EmbeddingUsageHandle;
use tianyan::model::UnconfiguredEmbeddingService;

use crate::embedding::EmbeddingClient;
pub use logging::LoggedEmbeddingService;

/// 按当前配置构造嵌入能力集合（`provider_embed_factory` 的实现体）。
///
/// 能力解析与迁移前 `ModelServices::from_config` 的嵌入部分**逐字一致**：
/// 先取 `TextEmbedding`，缺失时回落 `MultimodalEmbedding`（视觉相似性检索用的多模态
/// 嵌入模型可直接承担文本嵌入）；两者都没有 → 降级服务（服务端可启动，调用才报
/// “应用未配置”）。
///
/// 返回的 [`ProviderEmbedServices::usage_handle`] 是**空槽位**：装配层拿到 `UsageLog`
/// 之后经 [`tianyan::model::EmbeddingUsageHandle`] 后置注入（嵌入用量不走 AgentLoop，
/// 必须单独入账）。
pub fn services(config: &ModelsConfig) -> Result<ProviderEmbedServices> {
    let embedding_ref = config
        .resolve(ModelCapability::TextEmbedding)
        .or_else(|| config.resolve(ModelCapability::MultimodalEmbedding));

    // 嵌入用量上报槽位（与 LoggedEmbeddingService 共享；装配层后置注入）
    let embedding_usage_sink: EmbeddingUsageHandle = Arc::new(Mutex::new(None));

    let (embedding, embedding_model) = match &embedding_ref {
        Some(r) => {
            let provider = find_provider(&config.providers, &r.provider).ok_or_else(|| {
                TianyanError::config(format!("提供商 '{}' 未找到或未启用", r.provider))
            })?;
            // 客户端构造语义仍只有一份（`AsyncOpenAIClient::from_provider`），
            // 这里只是套上本 crate 的 newtype 以满足孤儿规则（见 `EmbeddingClient`）。
            let client = EmbeddingClient::new(provider)?;
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

    Ok(ProviderEmbedServices {
        embedding,
        embedding_model,
        usage_handle: embedding_usage_sink,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tianyan::config::{ModelEntry, ProviderConfig};

    fn provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            endpoint: format!("https://{name}.example.com/v1"),
            api_key: Some("k".to_string()),
            models: vec![ModelEntry {
                name: "text-embedding-v4".to_string(),
                capabilities: vec![ModelCapability::TextEmbedding],
                ..Default::default()
            }],
            timeout: 30,
            first_token_timeout: 300,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
            ..Default::default()
        }
    }

    /// 能力集合必须带模型名与**可后置注入**的用量槽位。
    ///
    /// 判别力：若槽位是构造期独占的（不给装配层句柄），嵌入用量就再也入不了账——
    /// 迁移前 `set_embedding_usage_sink` 的全部收益会静默消失。
    #[test]
    fn test_services_exposes_model_and_injectable_usage_handle() {
        let config = ModelsConfig {
            providers: vec![provider("dashscope")],
            ..Default::default()
        };
        let services = services(&config).expect("构造能力集合");
        assert_eq!(
            services.embedding_model.as_deref(),
            Some("text-embedding-v4")
        );
        assert!(
            services
                .usage_handle
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none(),
            "槽位初始为空，由装配层后置注入"
        );
    }

    /// 未配置任何 provider → 降级服务（调用才报“应用未配置”），不阻断启动。
    #[test]
    fn test_services_degrades_without_providers() {
        let services = services(&ModelsConfig::default()).expect("无 provider 不应报错");
        assert!(services.embedding_model.is_none());
    }

    /// 多模态嵌入能力可作为文本嵌入的回落（迁移前 `or_else` 分支，逐字保留）。
    #[tokio::test]
    async fn test_services_falls_back_to_multimodal_embedding_capability() {
        let mut p = provider("mm");
        p.models = vec![ModelEntry {
            name: "multimodal-embedding-v1".to_string(),
            capabilities: vec![ModelCapability::MultimodalEmbedding],
            ..Default::default()
        }];
        let config = ModelsConfig {
            providers: vec![p],
            ..Default::default()
        };
        let services = services(&config).expect("构造能力集合");
        assert_eq!(
            services.embedding_model.as_deref(),
            Some("multimodal-embedding-v1"),
            "TextEmbedding 缺失时应回落 MultimodalEmbedding"
        );
        // 降级服务在调用时才报错，本用例只锁「构造期不失败」
        assert!(
            services
                .embedding
                .embedding_dimension("multimodal-embedding-v1")
                > 0
        );
    }
}
