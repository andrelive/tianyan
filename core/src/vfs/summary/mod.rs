//! 摘要管理 —— 分层摘要的生成与服务。

mod engine;

#[cfg(test)]
pub use engine::MockSummaryEngine;
pub use engine::{
    parse_doc_index, render_doc_index, DocIndex, IndexSection, SummaryEngine, SummaryLevel,
    SummaryService, ABSTRACT_TOKEN_LIMIT, OVERVIEW_TOKEN_LIMIT,
};
