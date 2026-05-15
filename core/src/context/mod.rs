//! 上下文管理模块。
//!
//! 本模块提供上下文工程能力，包括统一的上下文存储和检索系统。

pub mod assembly;
pub mod compression;
pub mod pipeline;
pub mod retrieval;
pub mod rule_recorder;
pub mod rule_suggester;
pub mod types;

pub use assembly::assemble_prompt;
pub use pipeline::ContextPipeline;
pub use rule_recorder::{FailureKind, RuleRecorder};
pub use rule_suggester::RuleSuggester;

pub use compression::{
    estimate_tokens, CompressionConfig, CompressionResult, CompressionStatus, CompressionStrategy,
    ContextCompressor, TokenEstimator,
};
pub use retrieval::{
    ContentLoadStrategy, ContentLoaderImpl, ContextRetriever, DualLayerRetriever,
    DualLayerRetrieverBuilder, Intent, IntentAnalyzer, LoadedContent, QueryType, RetrievalResult,
    RetrievalStep, RetrievalStepType, RetrievalTrace, RetrievalTraceBuilder, TokenBudget,
    TokenPercentages, TokenStats,
};
pub use types::*;
