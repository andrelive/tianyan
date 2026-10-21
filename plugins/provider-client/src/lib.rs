//! provider-client：provider 域的**共享基底** crate（ADR-049 期 2 序 6）。
//!
//! 它只装两样东西，二者都被 `provider-llm` 与 `provider-embed` **同时**需要：
//!
//! | 内容 | 来源（迁移前） | 为什么两个能力插件都要 |
//! |---|---|---|
//! | [`AsyncOpenAIClient`]（OpenAI 兼容客户端，一次构造） | `core/src/model/provider/client.rs` | 聊天 / 视觉 / 嵌入三种能力共用一个 HTTP 客户端与一套构造语义 |
//! | [`RetryableFailure`] / [`classify_http_status`] | `core/src/model/provider/chat.rs` | 聊天与嵌入的请求发送都按同一套 HTTP 状态码语义分类（ADR-014） |
//!
//! ## 为什么是**三个** crate（`provider-client` + `provider-llm` + `provider-embed`）而不是两个
//!
//! 这是本序唯一的结构性裁决，理由是硬的、可复核的：
//!
//! 1. **孤儿规则**：`impl EmbeddingService for AsyncOpenAIClient` 只能写在**定义该类型**的
//!    crate 里（trait 在 core、类型若在 `provider-llm`，则 `provider-embed` 两样都是外部
//!    类型，`impl` 直接被编译器拒绝）。所以嵌入能力必须与客户端同 crate，而聊天/视觉也要
//!    用同一个客户端——两者不可能各自拥有它。
//! 2. **一次构造、全路径生效**（迁移前 `client.rs` 的注释逐字保留）：UA 单点（ADR-046）、
//!    方言解析一次（ADR-038）、双预算超时（ADR-044）、动态会话头解析一次、`RetryPolicy`。
//!    复制一份构造逻辑＝两份口径同时漂移，是本仓库反复记录过的坑（`RepoMapCache`、
//!    `action_fingerprint`、`extract_marker_summary` 都是同一条判据的产物）。
//! 3. **两个能力插件之间零互赖**（D2-2）：若让 `provider-embed` 依赖 `provider-llm`，
//!    改一行嵌入实现就要重编聊天插件，且插件间出现直接 import——两者都是本 ADR 明令禁止的。
//!
//! D7 的判据也支持这个切法：**客户端构造**（UA / 超时 / 方言 / 动态头 / 重试）是第三种
//! 「变化原因」，与「聊天怎么发请求」「嵌入怎么解析 usage」都不同，因此值得独立成 crate。
//! 反指标（每 crate ≥ 300 行且 ≥ 2 个真实消费者）：本 crate 约 410 行、恰有 2 个消费者。
//!
//! ## 为什么这些类型**不能**留在 core
//!
//! 它们是 provider wire 方言的**实现**（B 级，ADR-049 D3）：改思考字段名、改 UA 默认值、
//! 调首 token 预算、加一种动态请求头，都不该重编内核。`async-openai` 依赖也随本 crate
//! 离开 core（迁移后 `cargo tree -p tianyan-core` 无 `async-openai`）。
//!
//! 留在 core 的是**契约与配置**：`ChatService` / `EmbeddingService` / `VlmService`（S 级）、
//! 载荷类型（`model::types`）、模型规格（`model::spec`）、重试策略（`model::retry`）、
//! 与 `TianyanConfig` 字段绑定的 `config::model`（方言数据 + provider 配置）。
//!
//! ## 装配形状
//!
//! 本 crate **不实现任何 factory**（它没有装配插槽）：装配层只把它间接带进
//! `AssemblyServices::provider_llm_factory` / `provider_embed_factory` 的产出里。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod client;
mod failure;

pub use client::AsyncOpenAIClient;
pub use failure::{classify_http_status, RetryableFailure};
