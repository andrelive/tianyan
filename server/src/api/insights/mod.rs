//! 智能体内部状态视图：记忆浏览 + 使用统计。
//!
//! 提供 scheduler/observability 链路"只写不读"部分的读路径：
//! - `GET /api/v1/memory` — 浏览 `tianyan://memory/*` 提取的记忆
//! - `GET /api/v1/stats` — 技能调用/文档访问/搜索热度统计摘要

pub mod handlers;
pub mod routes;

pub use routes::routes;
