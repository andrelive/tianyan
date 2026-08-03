//! 上下文管理模块。
//!
//! 本模块提供上下文工程能力，包括统一的上下文存储和检索系统。

pub mod assembler;
pub mod compression;
pub mod pipeline;
pub mod retrieval;

pub use pipeline::ContextPipeline;

pub use assembler::ContextAssembler;
pub use compression::{
    estimate_tokens, CompressionConfig, CompressionResult, CompressionStatus, CompressionStrategy,
    ContextCompressor, TokenEstimator,
};
pub use retrieval::{
    DualLayerRetriever, Intent, IntentAnalyzer, RetrievalResult, RetrievalStep, RetrievalStepType,
    RetrievalTrace,
};
