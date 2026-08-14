//! 摘要管理 —— 分层摘要的生成与服务。

mod engine;

#[cfg(test)]
pub use engine::MockSummaryEngine;
pub use engine::{
    SummaryEngine, SummaryLevel, SummaryService, ABSTRACT_TOKEN_LIMIT, OVERVIEW_TOKEN_LIMIT,
};
