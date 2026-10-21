//! provider 插件的**装配原语**（ADR-049 期 2 序 6）。
//!
//! 本模块只放三样东西，全部是「装配期形状」而非实现：
//!
//! | 类型 | 角色 |
//! |---|---|
//! | [`ProviderLlmServices`] | `tianyan-provider-llm` 工厂的产出：聊天 / 视觉服务 + 模型名 + 每 provider 客户端池 + 连通性探针 |
//! | [`ProviderEmbedServices`] | `tianyan-provider-embed` 工厂的产出：嵌入服务 + 模型名 + 用量上报槽位 |
//! | [`ConnectionProbe`] | 配置页「测试连接」的**契约**（实现在 provider 插件） |
//!
//! ## 为什么这两个结构住在 core（不是 `server/src/assembly.rs`）
//!
//! 与 [`tianyan::scheduler::TaskServices`] / [`tianyan::context::ContextServices`] 同一条
//! 纪律：它们被**三方**同时引用——server 消费、装配包（`app` / `tauri` / 集成测试）生产、
//! 插件填充。住在 server 会让两个 provider 插件反向依赖 `tianyan-server`，破 D2-1
//! 「插件只依赖 kernel-api」。
//!
//! ## 为什么能力集合里带「探针」而不是给探针单开一个插槽
//!
//! 探测用的是用户在配置页**当场输入**的 endpoint / api_key（不是已配置的 provider
//! 实例），但它必须复用 provider 插件里那**一套**客户端构造语义：UA 单点、方言解析、
//! 双预算超时（ADR-044）、动态会话头（ADR-046）、重试策略（ADR-014 语义分类）。
//!
//! 因此 `probe` 是 `tianyan-provider-llm` 工厂**顺手产出**的一项能力（插件内部用同一个
//! `AsyncOpenAIClient::new` 造临时客户端），而不是装配层另造一个客户端（那要复制上面
//! 五条语义，两份口径同时漂移），也不是第三个插槽（插槽数是每加一项能力都要付的装配
//! 成本，而探针与聊天服务同源、同 crate、同构造点）。
//!
//! 迁移前该函数在 `server/src/api/config/handlers.rs` 直接
//! `tianyan::model::AsyncOpenAIClient::new(...)`（具体类型）——实现搬走后那个类型不存在，
//! 于是契约化到这里；错误分类（`is_permission` / `is_not_found` / `is_timeout`，ADR-014）
//! 仍留在通道层，本契约只承诺「返回 `TianyanError`」。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::common::error::Result;
use crate::model::traits::{ChatService, EmbeddingService, VlmService};
use crate::model::unconfigured::{
    UnconfiguredChatService, UnconfiguredEmbeddingService, UnconfiguredVlmService,
};
use crate::model::usage::EmbeddingUsageHandle;
use crate::model::SharedChatService;

/// `tianyan-provider-llm` 工厂的产出（聊天 + 视觉 + 路由池 + 探针）。
///
/// 字段语义与迁移前 `ModelServices` 的同名部分逐字一致（`ModelServices` 已搬进
/// `server`，见 ADR-049 期 2 序 6 实施记录）。
#[derive(Clone)]
pub struct ProviderLlmServices {
    /// 默认聊天服务（`preferences.chat` 解析出的 provider/model；已包日志装饰）。
    pub chat: Arc<dyn ChatService>,
    /// 聊天模型名（空串 = 未配置，与迁移前一致）。
    pub chat_model: String,
    /// 视觉分析服务（未配置时为降级服务）。
    pub vision: Arc<dyn VlmService>,
    /// 视觉模型名（`None` = 未配置）。
    pub vision_model: Option<String>,
    /// 每 provider 的聊天客户端池（**会话级模型选择的路由面**；key = provider name）。
    ///
    /// 只收「已启用且至少一个模型声明了 Chat 能力」的 provider；未命中的 provider
    /// 在路由时回落默认聊天服务。
    pub chat_clients: HashMap<String, SharedChatService>,
    /// 连通性探针（配置页「测试连接」；`None` = 该装配未提供探针）。
    pub probe: Option<Arc<dyn ConnectionProbe>>,
}

impl ProviderLlmServices {
    /// 未装配 provider 插件时的能力集合：两项服务都是「调用即报未配置」的降级实现，
    /// 路由池为空、无探针。
    ///
    /// 由 `server` 的装配插槽在**未注入工厂**时使用（`create_app` / 嵌入式 / 库单测
    /// 路径）。降级语义与迁移前 `ModelServices::from_config` 在「能力未配置」时的行为
    /// 一致：服务对象存在、调用才报错，服务端照常启动。
    pub fn unconfigured() -> Self {
        Self {
            chat: Arc::new(UnconfiguredChatService),
            chat_model: String::new(),
            vision: Arc::new(UnconfiguredVlmService),
            vision_model: None,
            chat_clients: HashMap::new(),
            probe: None,
        }
    }
}

/// `tianyan-provider-embed` 工厂的产出（嵌入服务 + 模型名 + 用量槽位）。
#[derive(Clone)]
pub struct ProviderEmbedServices {
    /// 文本嵌入服务（已包日志 / 查询缓存 / 用量上报装饰）。
    pub embedding: Arc<dyn EmbeddingService>,
    /// 嵌入模型名（`None` = 未配置）。
    pub embedding_model: Option<String>,
    /// 用量上报槽位（装配层拿到 `UsageLog` 后经
    /// [`tianyan::model::EmbeddingUsageHandle`] 后置注入）。
    pub usage_handle: EmbeddingUsageHandle,
}

impl ProviderEmbedServices {
    /// 未装配 provider 插件时的能力集合：嵌入服务调用即报未配置，槽位为空句柄。
    pub fn unconfigured() -> Self {
        Self {
            embedding: Arc::new(UnconfiguredEmbeddingService),
            embedding_model: None,
            usage_handle: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// 连通性探测契约（配置页「测试连接」，ADR-049 期 2 序 6）。
///
/// 实现方：`tianyan-provider-llm`（用插件的客户端构造语义造**临时**客户端）。
/// 消费方：`server/src/api/config/handlers.rs`（`POST /api/config/test-connection`）。
///
/// 契约只承诺「成功 = 该 endpoint + key + model 可用」，错误分类（ADR-014 的
/// `is_permission` / `is_not_found` / `is_timeout` 语义谓词）仍在消费端——上游错误已由
/// provider 实现映射为带 KIND 前缀的 `TianyanError`，消费端不做消息文本匹配。
#[async_trait]
pub trait ConnectionProbe: Send + Sync {
    /// 用给定凭据发一次最小聊天请求验证连通性。
    ///
    /// 成功返回该 endpoint 上可用的模型名列表。今天恒为 `vec![model]`（迁移前
    /// `test_model_connection` 的行为逐字保留）：探测只回答「这三个参数能不能用」，
    /// **不列**目录——列目录是 [`crate::model::ServiceDiscovery`] 的事，两者语义不同
    /// （探测允许 endpoint 没有任何模型目录接口）。
    async fn probe(&self, endpoint: &str, api_key: &str, model: &str) -> Result<Vec<String>>;
}
