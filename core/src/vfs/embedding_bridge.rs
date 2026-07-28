//! EmbeddingService 到 EmbeddingProvider 的适配桥接。
//!
//! 将 `crate::model::EmbeddingService` 包装为 VFS 模块的 `EmbeddingProvider` trait，
//! 消除 VFS 对 model 模块的直接依赖。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::Embedding;
use crate::model::EmbeddingService;
use crate::vfs::traits::EmbeddingProvider;

/// 将 `Arc<dyn EmbeddingService>` 适配为 `dyn EmbeddingProvider`。
///
/// 这是 VFS 与 model 模块间的唯一依赖桥接点 —— 外部代码通过此桥传
/// 入嵌入能力，VFS 内部仅通过 `EmbeddingProvider` trait 访问。
pub struct EmbeddingServiceBridge {
    service: Arc<dyn EmbeddingService>,
}

impl EmbeddingServiceBridge {
    /// 使用嵌入服务实例创建桥接。
    pub fn new(service: Arc<dyn EmbeddingService>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for EmbeddingServiceBridge {
    async fn embed_single(&self, model: &str, text: &str) -> Result<Embedding> {
        self.service.embed_single(model, text).await
    }
}
