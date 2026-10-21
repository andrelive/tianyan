//! 嵌入用量上报契约（ADR-049 期 2 序 6：从 `model::provider::middleware` 提到 `model` 根）。
//!
//! ## 为什么这条契约留 **core**（而不是随实现搬进 `plugins/provider-embed`）
//!
//! 三个角色分别落在三个 crate 里：
//!
//! | 角色 | 位置 | 说明 |
//! |---|---|---|
//! | 契约 | **core**（本模块） | `EmbeddingUsageSink` + 后置注入句柄，公开路径不变 |
//! | 实现 | 通道层（`server/src/embedding_usage.rs`） | `UsageLogEmbeddingSink` 写 `usage_logs`（依赖 SQLite，属通道层业务） |
//! | 消费 | 插件（`plugins/provider-embed`） | `LoggedEmbeddingService` 每次嵌入后 `record` |
//!
//! 契约若跟着实现住进插件，通道层就得 `use tianyan_provider_embed::…`——插件与通道层
//! 互相认识，破 D2-1/D2-3。这与 `TaskReviewer`（期 3-T3）、`ApprovalGate`（期 3-T1）是
//! 同一条边界判据：**实现方在通道层、消费方在插件 → 契约必须住在两者之上的 core**。
//!
//! ## 背景（迁移前就有的设计，逐字保留）
//!
//! 嵌入调用**不走 AgentLoop**（检索与摘要写入直接调 `EmbeddingService`），其 token
//! 用量曾完全不入账——账单与应用内用量统计都看不到嵌入消耗。本条契约与
//! [`EmbeddingUsageHandle`] 的后置注入就是为此而加：`ModelServices` 构造之后再注入
//! `UsageLog` 实现，避免 model 层（今天的 provider 插件）反向依赖 observability/db。

use crate::common::types::TokenUsage;

/// 嵌入用量上报槽位类型（装配层后置注入的可选句柄）。
///
/// 用共享 `Arc<Mutex<Option<...>>>` 而不是构造期参数：装配层（server）在创建模型服务
/// 之后才拿到 `UsageLog`，后置注入不必改「从配置构造服务」的签名。
pub type EmbeddingUsageHandle =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<dyn EmbeddingUsageSink>>>>;

/// 嵌入调用用量上报契约（model 层定义契约，实现由装配层注入）。
///
/// 实现必须**非阻塞**（例如 `tokio::spawn` 异步落库）：本方法在检索热路径上被调用，
/// 不得等待 I/O。
pub trait EmbeddingUsageSink: Send + Sync {
    /// 记录一次嵌入调用；`calls` 为本次文本条数（批量 > 1）。
    fn record(&self, model: &str, usage: &TokenUsage, calls: usize);
}
