//! 天演智能体系统的检索模块。
//!
//! 本模块实现了目录递归检索系统，包括。
//! - 意图分析
//! - 双层向量检索（L0/L1。
//! - 带有 Token 预算管理的内容加。
//! - 检索追踪记。

mod intent;
mod loader;
mod retriever;
mod trace;

pub use crate::context::types::RetrievalResult;
pub use intent::{Intent, IntentAnalyzer, QueryType, TargetScope};
pub use loader::{
    ContentLoadStrategy, ContentLoaderImpl, LoadedContent, TokenBudget, TokenCounter,
};
pub use retriever::{ContextRetriever, DualLayerRetriever, DualLayerRetrieverBuilder};
pub use trace::{
    RetrievalStep, RetrievalStepType, RetrievalTrace, RetrievalTraceBuilder, TokenPercentages,
    TokenStats,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // 验证所有公共类型可访问
        let _ = QueryType::Search("test".to_string());
        let _ = TokenBudget::new(1000);
        let _ = ContentLoadStrategy::from_score(0.8);
    }
}
