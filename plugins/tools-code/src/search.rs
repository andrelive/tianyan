//! 代码搜索门面（内嵌引擎，无外部 rg 依赖）——实现见 [`search_engine`]。
//!
//! 输出契约（JSON，LLM 可见）：
//! ```json
//! { "pattern": "...", "path": "...", "results": [...], "count": N, "total": N,
//!   "truncated": bool, "exhausted": bool, "output_mode": "content",
//!   "message": "..." }
//! ```
//!
//! **参数契约留在 core**（`tianyan::executor::search_types`）：`OutputMode` /
//! `SearchOptions` / 分页常量是工具 schema 与输出字段的类型，不是实现细节——
//! core 的 `SearchCodeParams` 与插件实现共用同一类型，core 不反向依赖插件
//! （ADR-049 期 1）。

use serde_json::Value;

use tianyan::common::error::TianyanError;
pub use tianyan::executor::search_types::{
    OutputMode, SearchOptions, DEFAULT_HEAD_LIMIT, MAX_HEAD_LIMIT,
};

/// 执行代码搜索（内嵌引擎，无外部 rg 依赖）。
///
/// - `total` = 收集到的记录总数（含守卫丢弃前的上限 5000）。
/// - `truncated` = `offset + head_limit < total`（还有更多结果可翻页）。
/// - `exhausted` = `offset >= total`（已无更多结果）。
pub async fn execute_search_code(
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, TianyanError> {
    crate::search_engine::execute_embedded_search(pattern, options)
}
