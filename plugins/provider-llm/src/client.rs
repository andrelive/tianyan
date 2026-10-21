//! 聊天 / 视觉 / 模型目录能力的客户端（[`AsyncOpenAIClient`] 的 newtype 包装）。
//!
//! **为什么需要 newtype**（不是审美选择）：`impl ChatService for …` / `impl VlmService
//! for …` / `impl ServiceDiscovery for …` 必须写在「类型或 trait 属于本 crate」的位置
//! （Rust 孤儿规则）。三个 trait 在 core、`AsyncOpenAIClient` 在基底 crate
//! `tianyan-provider-client`——对本 crate 都是外部类型，直接 `impl` 会被编译器拒绝
//! （`E0117`/`E0116`）。
//!
//! **为什么不复制客户端构造**：`new` / `from_provider` 直接转发给
//! [`AsyncOpenAIClient::new`] / [`AsyncOpenAIClient::from_provider`]，UA 单点（ADR-046）、
//! 方言解析一次（ADR-038）、双预算超时（ADR-044）、动态会话头、`RetryPolicy` 全部仍是
//! 同一份实现（`provider-embed` 也复用同一份，见其 `EmbeddingClient`）。
//!
//! **为什么能 `impl Deref`**：包装只改「哪个 crate 拥有 impl」，不该改任何字段访问。
//! `Deref<Target = AsyncOpenAIClient>` 让 `self.base_url` / `self.http` / `self.dialect`
//! / `self.retry_policy` / `self.send_chat_request(...)` 等全部**逐字保持**——迁移前后
//! 的差异因此只剩类型名与本文件头，请求构造与流式解析代码一行未动。

use std::ops::Deref;

use tianyan::common::error::Result;
use tianyan::config::ProviderConfig;
use tianyan_provider_client::AsyncOpenAIClient;

/// 聊天 / 视觉客户端的包装（能力实现的载体，见模块文档）。
#[derive(Debug, Clone)]
pub struct ChatClient(AsyncOpenAIClient);

impl ChatClient {
    /// 从 provider 配置构造（构造语义完全复用 [`AsyncOpenAIClient::from_provider`]）。
    pub fn from_provider(config: &ProviderConfig) -> Result<Self> {
        Ok(Self(AsyncOpenAIClient::from_provider(config)?))
    }

    /// 从基本参数构造（构造语义完全复用 [`AsyncOpenAIClient::new`]）。
    ///
    /// 供连通性探针造**临时**客户端用（配置页「测试连接」：用户的 endpoint / key 尚未
    /// 落盘，不能用已配置的 provider 实例）。
    pub fn new(
        name: impl Into<String>,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
        first_token_timeout_secs: u64,
    ) -> Result<Self> {
        Ok(Self(AsyncOpenAIClient::new(
            name,
            endpoint,
            api_key,
            timeout_secs,
            first_token_timeout_secs,
        )?))
    }

    /// 包住一个已构造的客户端（装配期「每 provider 复用同一个 HTTP client 池」用）。
    pub fn from_client(inner: AsyncOpenAIClient) -> Self {
        Self(inner)
    }
}

impl Deref for ChatClient {
    type Target = AsyncOpenAIClient;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
