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
mod types;

pub use intent::{Intent, IntentAnalyzer};
pub use retriever::DualLayerRetriever;
pub use types::{RetrievalResult, RetrievalStep, RetrievalStepType, RetrievalTrace};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::retrieval::intent::QueryType;
    use crate::context::retrieval::loader::ContentLoadStrategy;

    #[test]
    fn test_module_exports() {
        // 验证所有公共类型可访问
        let _ = QueryType::Search("test".to_string());
        let _ = ContentLoadStrategy::from_score(0.8);
    }
}
