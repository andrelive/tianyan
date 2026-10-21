//! provider-llm 插件：OpenAI 兼容 provider 的**聊天与视觉实现**（ADR-049 期 2 序 6）。
//!
//! 迁入本 crate（含各自的内联测试）：
//!
//! | 文件 | 原位置 | 内容 |
//! |---|---|---|
//! | [`chat`] | `core/src/model/provider/chat.rs` | 非流式 / 流式补全、SSE 手写解析、思考参数、reasoning 回传、缓存命中提取、方言基线 |
//! | [`vision`] | `core/src/model/provider/vision.rs` | VLM 分析（宽容 usage 形状）、多模态消息转换 |
//! | [`discovery`] | `core/src/model/provider/discovery.rs` | 模型目录（`ServiceDiscovery`） |
//! | [`logging`] | `core/src/model/provider/middleware.rs`（上半） | 聊天 / 视觉的日志装饰 |
//!
//! ## 为什么搬（判据：变化原因，ADR-049 D7）
//!
//! 这里全是 **B 级**（ADR-049 D3）：思考参数形态与档位预算、`reasoning_content` 回传
//! 规则、SSE 容错口径（usage-only 块 / cost 块 / 非标准 `finish_reason`）、缓存命中字段
//! 探测链、视觉 side 的宽容反序列化、日志文案。改这些的理由与改内核机制毫无关系——
//! 此前调一个方言细节要重编 core（42.4K 生产行）+ server（13.4K），现在只重编本 crate +
//! 装配包。`async-openai` 依赖也随之离开 core。
//!
//! ## 哪些类型**必须**留 core（不是遗漏）
//!
//! | 留 core | 位置 | 为什么 |
//! |---|---|---|
//! | `ChatService` / `VlmService` / `ServiceDiscovery` | `tianyan::model`（`model/traits.rs`） | S 级契约：内核（`AgentLoop`）、通道层、其他插件都以 `Arc<dyn ChatService>` 消费。搬走 ⇒ 内核反向依赖插件（D2-1/D2-3） |
//! | 载荷类型（`ChatCompletionRequest` / `ChatCompletionChunk` / `VisionRequest` …） | `tianyan::model::types` | 出现在 S 级 trait 签名里，且被 `agent` / `server` / 其它插件共用 |
//! | `model::spec`（模型规格 + 上下文窗口） | `tianyan::model::spec` | 内核（动态 `max_tokens`）、`policy-context`（压缩窗口联动）、`server`（配置面板）共用 |
//! | `model::retry`（重试策略与谓词） | `tianyan::model::retry` | 请求层策略被本插件**与** `provider-embed` 共用；且「429/5xx/网络可重试」是跨 provider 的统一约定（ADR-014） |
//! | `config::model`（`ProviderConfig` / `ProviderDialect` / `ThinkingParam` / `HeaderSource` …） | `tianyan::config` | 它们是 `TianyanConfig` 的字段（配置文件的形状），随内核一起加载/校验/热重载 |
//! | `ConnectionProbe` / `ProviderLlmServices` / `ProviderEmbedServices` | `tianyan::model::assembly` | 装配原语，server 与两个 provider 插件三方共用（见 `ProviderLlmServices` 文档） |
//!
//! ## 为什么 `ModelServices` 搬去 `server`（而不是进本插件）
//!
//! `ModelServices`（原 `core/src/model/services.rs`，232 行）是「容器 + 构造器」：
//! 它按能力解析 provider/model、建客户端池、把三件服务打包。它**在 core 里零生产使用者**
//! （只有 `model/mod.rs` 的导出 + 一处注释），真正的消费者是 channel 层的
//! `server/src/agent_builder.rs` 与 `AppState::shared_model_services()`。
//!
//! 按 D5（装配器二分）「容器 + 构造器」属于装配职责：能力解析（哪个 provider 提供哪种能力）
//! 是**装配决策**，而三种能力现在分属两个插件——容器只能活在认识这两个插槽的地方。
//! 若留在 core，core 就得认识插槽（反向依赖装配层）；若进任一个插件，另一个插件与
//! channel 层就得依赖它。因此它落在 `server/src/model_services.rs`，由装配层注入的两个
//! 工厂的产出拼装。
//!
//! ## 连通性探测为什么挂在工厂产出上（`ProviderLlmServices::probe`）
//!
//! 配置页「测试连接」用用户**当场输入**的 endpoint / api_key 造临时客户端，因此不能复用
//! 已配置的 provider 实例；但它必须复用本 crate 的客户端构造语义（UA 单点、方言解析、
//! 双预算超时、动态会话头、重试策略）。所以探针是本插件的实现（[`ConnectionProbe`] 契约
//! 在 core），随 `services()` 一起产出，而不是让通道层自己造客户端（那要复制五条语义）
//! 或另开一个插槽（插槽数是每加一项能力都要付的装配成本）。
//!
//! ## 行为零变化
//!
//! 请求构造（ADR-038 方言 / ADR-046 动态会话头 `x-opencode-session` / UA 全局默认）、
//! 流式双预算（ADR-044：首 token 300s / 块间 `timeout`）、reasoning 回传、重试
//! （5 次、1s 起点、2x 退避 + 抖动、429/5xx/网络可重试）、错误映射（KIND 前缀 +
//! `is_*` 谓词）逐字未改——`git diff --no-index` 的差异只有 `crate::` → `tianyan::`
//! 与 `super::client::` → `tianyan_provider_client::` 的路径前缀。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// 模块私有：本 crate 的公开面只有 [`services`]（工厂实现体）、[`ChatClient`]（能力载体，
// `provider-embed` 的同款 newtype 见其 `EmbeddingClient`）与两个日志装饰（`LoggedService`
// / `LoggedVlmService`，装配层可选地在工厂外加一层）。保持公开面最小（ADR-049 反指标：
// 每插件 crate 公开符号 ≤ 15）。
mod chat;
mod client;
mod discovery;
mod logging;
mod vision;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::types::Message;
use tianyan::config::{find_provider, ModelCapability, ModelsConfig};
use tianyan::model::assembly::{ConnectionProbe, ProviderLlmServices};
use tianyan::model::types::ChatCompletionRequest;
use tianyan::model::{ChatService, VlmService};
use tianyan::model::{SharedChatService, UnconfiguredChatService, UnconfiguredVlmService};
use tianyan_provider_client::AsyncOpenAIClient;

pub use client::ChatClient;
pub use logging::{LoggedService, LoggedVlmService};

/// 缓存的 provider 客户端映射。
///
/// key = provider name，确保同一 provider 只创建一个 HTTP client 实例
/// （迁移前 `core/src/model/services.rs` 的同名别名，逻辑逐字保留）。
type ClientPool = HashMap<String, AsyncOpenAIClient>;

/// 按当前配置构造聊天 + 视觉能力集合（`provider_llm_factory` 的实现体）。
///
/// 流程与迁移前 `ModelServices::from_config` 的聊天 / 视觉部分**逐字一致**：
///
/// 1. 按能力 resolve 出 (provider, model) 对（缺失能力 → 降级服务，服务端可启动）；
/// 2. 为每个用到的 provider 创建（或复用）一个 [`AsyncOpenAIClient`]；
/// 3. 从 client 构造对应 trait object（`LoggedService` / `LoggedVlmService` 包装）；
/// 4. 对「已启用且至少一个模型声明了 Chat 能力」的 provider **预建**聊天客户端池
///    （会话级模型选择的路由面；单个失败跳过 + 告警，路由时回落默认聊天服务）。
///
/// 唯一未搬进来的部分是「部分模型能力未配置」的那条 `warn!`：它同时看三项能力，因而
/// 由 `server` 侧拼装完容器后统一打（文案与字段逐字保留）。
pub fn services(config: &ModelsConfig) -> Result<ProviderLlmServices> {
    let chat_ref = config.resolve(ModelCapability::Chat);
    let vision_ref = config.resolve(ModelCapability::Vision);

    let mut clients = ClientPool::new();

    let get_or_create_client =
        |clients: &mut ClientPool, provider_name: &str| -> Result<AsyncOpenAIClient> {
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
                Arc::new(LoggedService(ChatClient::from_client(client))) as Arc<dyn ChatService>,
                r.model.clone(),
            )
        }
        None => (
            Arc::new(UnconfiguredChatService) as Arc<dyn ChatService>,
            String::new(),
        ),
    };
    let (vision, vision_model) = match &vision_ref {
        Some(r) => {
            let client = get_or_create_client(&mut clients, &r.provider)?;
            (
                Arc::new(LoggedVlmService(ChatClient::from_client(client))) as Arc<dyn VlmService>,
                Some(r.model.clone()),
            )
        }
        None => (
            Arc::new(UnconfiguredVlmService) as Arc<dyn VlmService>,
            None,
        ),
    };

    // 每 provider 的聊天客户端池（会话级模型选择路由）：对全部「声明了 Chat
    // 能力模型」的已启用 provider 预建（数量少、构建便宜）；单个失败跳过并
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
                chat_clients.insert(
                    provider.name.clone(),
                    Arc::new(LoggedService(ChatClient::from_client(client))),
                );
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

    Ok(ProviderLlmServices {
        chat,
        chat_model,
        vision,
        vision_model,
        chat_clients,
        probe: Some(Arc::new(ProviderConnectionProbe)),
    })
}

/// 连通性探针的实现（[`ConnectionProbe`] 契约在 core）。
///
/// 迁移前该逻辑住 `server/src/api/config/handlers.rs::test_model_connection`：
/// 直接 `tianyan::model::AsyncOpenAIClient::new(...)` + 一次最小聊天请求。实现搬走后
/// 具体类型不再出现在通道层，于是这段逻辑随客户端一起进本 crate——**逐字保留**：
/// `timeout = 30`、`first_token_timeout = 300`、`system("You are a helpful assistant.")`、
/// `temperature = 0.7`、`max_tokens = 10`、`stream = false`，成功即返回 `vec![model]`
/// （探测不列目录）。
///
/// 错误分类（`is_permission` / `is_not_found` / `is_timeout` → 用户文案）**不在本 crate**：
/// 那是通道层的展示口径（ADR-014 的消费端），本实现只把上游错误原样上抛。
pub struct ProviderConnectionProbe;

#[async_trait]
impl ConnectionProbe for ProviderConnectionProbe {
    async fn probe(&self, endpoint: &str, api_key: &str, model: &str) -> Result<Vec<String>> {
        let client = ChatClient::new("test-connection", endpoint, api_key, 30, 300)
            .map_err(|e| TianyanError::Custom(format!("创建客户端失败: {}", e)))?;

        let request = ChatCompletionRequest::new(
            model.to_string(),
            vec![Message::system("You are a helpful assistant.")],
        )
        .with_temperature(0.7)
        .with_max_tokens(10)
        .with_stream(false);

        client.chat_completion(request).await?;
        Ok(vec![model.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tianyan::config::{ProviderConfig, ThinkingField};

    /// 构造一个声明了 Chat / TextEmbedding / Vision 三种能力的 provider。
    fn provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            endpoint: format!("https://{name}.example.com/v1"),
            api_key: Some("k".to_string()),
            models: vec![
                tianyan::config::ModelEntry {
                    name: "chat-model".to_string(),
                    capabilities: vec![ModelCapability::Chat],
                    ..Default::default()
                },
                tianyan::config::ModelEntry {
                    name: "vision-model".to_string(),
                    capabilities: vec![ModelCapability::Vision],
                    ..Default::default()
                },
            ],
            timeout: 30,
            first_token_timeout: 300,
            enabled: true,
            headers: HashMap::new(),
            thinking_field: None,
            dialect: None,
            ..Default::default()
        }
    }

    fn models_config(providers: Vec<ProviderConfig>) -> ModelsConfig {
        ModelsConfig {
            providers,
            ..Default::default()
        }
    }

    /// 能力集合必须带模型名、路由池与探针（装配层据此填 `ModelServices`）。
    ///
    /// 判别力：把 `probe` 改成 `None`（或漏建路由池）→ 断言红：探测能力会从生产装配里
    /// 静默消失，配置页「测试连接」变成「未装配」。
    #[test]
    fn test_services_exposes_models_routing_pool_and_probe() {
        let config = models_config(vec![provider("p1")]);
        let services = services(&config).expect("构造能力集合");
        assert_eq!(services.chat_model, "chat-model");
        assert_eq!(services.vision_model.as_deref(), Some("vision-model"));
        assert!(
            services.chat_clients.contains_key("p1"),
            "声明了 Chat 能力的已启用 provider 必须进路由池"
        );
        assert!(services.probe.is_some(), "必须产出连通性探针");
    }

    /// 未配置任何 provider 时给出**降级服务**而不是报错：服务端仍能启动，调用才报
    /// “应用未配置”（与迁移前 `ModelServices::from_config` 的降级语义逐字一致）。
    #[test]
    fn test_services_degrades_without_providers() {
        let services = services(&models_config(vec![])).expect("无 provider 不应报错");
        assert_eq!(services.chat_model, "", "未配置时模型名为空串");
        assert!(services.vision_model.is_none());
        assert!(services.chat_clients.is_empty());
    }

    /// 禁用的 provider 不进路由池（迁移前 `from_config` 的 `enabled` 过滤）。
    #[test]
    fn test_services_skips_disabled_provider_in_routing_pool() {
        let mut p = provider("off");
        p.enabled = false;
        let services = services(&models_config(vec![p])).expect("构造能力集合");
        assert!(
            services.chat_clients.is_empty(),
            "已禁用的 provider 不得进路由池"
        );
    }

    /// 探针的请求形状与错误语义（**不发起真实请求**：endpoint 非法即在构造阶段失败）。
    ///
    /// 判别力：若探针改用别的构造路径（如绕过 `AsyncOpenAIClient::new` 的校验），
    /// 非法 endpoint 就不再在构造期失败，本断言转红。
    #[tokio::test]
    async fn test_probe_rejects_invalid_endpoint_at_construction() {
        let probe = ProviderConnectionProbe;
        let err = probe
            .probe("api.example.com/v1", "k", "m")
            .await
            .expect_err("非 http(s) endpoint 必须在构造阶段失败");
        assert!(
            err.to_string().contains("创建客户端失败"),
            "错误文案须与迁移前一致（`创建客户端失败: …`）：{err}"
        );
    }

    /// 方言解析仍在客户端构造期完成（本插件的日志装饰不改变它）。
    #[test]
    fn test_client_dialect_flows_through_services() {
        let mut ollama = provider("ollama");
        ollama.endpoint = "https://ollama.com/v1".to_string();
        let client = ChatClient::from_provider(&ollama).expect("构造客户端");
        assert_eq!(client.dialect.thinking_field, ThinkingField::Ollama);
    }
}
