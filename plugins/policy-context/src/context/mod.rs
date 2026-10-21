//! 上下文工程模块（ADR-049 期 3-T2：**策略面已整体搬出 core**）。
//!
//! 本模块原先住在 `core/src/context/`。按 D7「按变化原因切」复查后发现它不是
//! 一个整体（见 ADR-049「期 3 普查结论 · ★ 推翻前提 1」）：
//!
//! - **留 core**：`assembler.rs`（`ContextAssembler`）是 K1 的**组装机制**——纯静态
//!   函数、零 I/O，承载 ADR-043「工具对连续性规范化」与「前缀零漂移」，`loop.rs` 与
//!   `agent_core.rs` 直接持有它；
//! - **搬到这里**：管线（`pipeline`）、压缩（`compression`）、检索（`retrieval`）——
//!   调 prompt 模板 / 压缩阈值 / RRF 参数 / 意图分类关键词都不该重编内核。
//!
//! 模块树的**路径形状刻意与 core 一致**（`context::{pipeline, compression, retrieval}`）：
//! core 的单元测试在 `#[cfg(test)]` 下用 `#[path]` **同源包含**这些文件
//! （`core/src/context/mod.rs`），而文件内部的跨模块引用写作 `crate::context::…`
//! ——在插件里指向本 crate 的同名模块，在 core 测试构建里指向 core 的同名模块，
//! **一处源码、两边成立、零复制**。
//!
//! 注：`ContextAssembler` 不在本模块（它留在 core），因此 core 的
//! `tianyan::context::ContextAssembler` 路径保持不变，`loop.rs` / `agent_core.rs` 零改动。

pub mod compression;
pub mod pipeline;
pub mod retrieval;

pub use pipeline::ContextPipeline;

pub use compression::{
    estimate_tokens, CompressionConfig, CompressionResult, CompressionStatus, CompressionStrategy,
    ContextCompressor, TokenEstimator,
};
pub use retrieval::{DualLayerRetriever, Intent, IntentAnalyzer, RetrievalResult};
