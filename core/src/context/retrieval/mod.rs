//! 天演智能体系统的检索模块。
//!
//! 本模块实现基于意图分析与双层向量检索的检索系统：
//! - 意图分析
//! - 双层向量检索（L0/L1）
//! - 带 Token 预算管理的内容加载

mod intent;
mod loader;
mod retriever;
mod types;

pub use intent::{Intent, IntentAnalyzer};
pub use retriever::DualLayerRetriever;
pub use types::RetrievalResult;

#[cfg(test)]
mod tests {
    use crate::context::retrieval::intent::QueryType;
    use crate::context::retrieval::loader::ContentLoadStrategy;

    #[test]
    fn test_module_exports() {
        // 验证所有公共类型可访问
        let _ = QueryType::Search("test".to_string());
        let _ = ContentLoadStrategy::from_score(0.8);
    }
}
