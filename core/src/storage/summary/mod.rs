//! 摘要管理 —— 分层摘要的生成与服务。

mod engine;

pub use engine::{
    MockSummaryEngine, SummaryEngine, SummaryLevel, ABSTRACT_TOKEN_LIMIT, OVERVIEW_TOKEN_LIMIT,
};
