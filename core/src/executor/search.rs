//! 代码搜索执行器（内嵌引擎，无外部 rg 依赖）。
//!
//! 输出契约（JSON，LLM 可见）：
//! ```json
//! { "pattern": "...", "path": "...", "results": [...], "count": N, "total": N,
//!   "truncated": bool, "exhausted": bool, "output_mode": "content" }
//! ```
//!
//! 守卫：单条记录文本 > 64KB 丢弃；单条记录 submatch 上限 100；单行显示截断
//! 2000 字符并追加 `…<truncated>`；`.git` 目录始终排除；总记录硬上限 5000。
//!
//! 执行委托给 [`super::search_engine`]（`ignore` 遍历 + `regex` 匹配，
//! 语义与 ripgrep 对齐）；本模块保留参数模型与输出契约定义。

use serde_json::Value;

use crate::common::error::TianyanError;

/// 分页默认条数（search_engine 引用）。
pub const DEFAULT_HEAD_LIMIT: usize = 200;

/// 输出模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// 仅列出命中的文件名（默认）。
    FilesWithMatches,
    /// 匹配行 + 行号 + submatch 偏移。
    Content,
    /// 每个文件的匹配行数（`text` 字段为数字字符串）。
    Count,
}

impl OutputMode {
    /// 模式名（出现在输出 JSON 的 `output_mode` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            OutputMode::FilesWithMatches => "files_with_matches",
            OutputMode::Content => "content",
            OutputMode::Count => "count",
        }
    }

    /// 解析输出模式字符串；`None` 取默认 [`OutputMode::FilesWithMatches`]。
    pub(crate) fn parse(mode: Option<&str>) -> Result<Self, TianyanError> {
        match mode {
            None | Some("files_with_matches") => Ok(OutputMode::FilesWithMatches),
            Some("content") => Ok(OutputMode::Content),
            Some("count") => Ok(OutputMode::Count),
            Some(other) => Err(TianyanError::Custom(format!(
                "executor: 搜索失败：输出模式无效：{other}"
            ))),
        }
    }
}

/// 搜索选项（与工具参数一一对应；`None`/`false` 取默认行为）。
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// 搜索根目录（默认当前工作目录）。
    pub path: Option<String>,
    /// glob 过滤器（如 `*.rs`）。
    pub glob: Option<String>,
    /// 输出模式字符串（见 [`OutputMode::parse`]）。
    pub output_mode: Option<String>,
    /// 文件类型过滤器（rg `--type`，如 "rust"）。
    pub type_: Option<String>,
    /// 忽略大小写（`-i`）。
    pub ignore_case: bool,
    /// 是否显示行号（content 模式默认 true）。
    pub line_number: Option<bool>,
    /// 上下文行数（`-C`）。
    pub context: Option<usize>,
    /// 前文行数（`-B`）。
    pub before_context: Option<usize>,
    /// 后文行数（`-A`）。
    pub after_context: Option<usize>,
    /// 结果条数上限（默认 [`DEFAULT_HEAD_LIMIT`]）。
    pub head_limit: Option<usize>,
    /// 分页偏移（0 起始）。
    pub offset: usize,
    /// 多行匹配（`-U`）。
    pub multiline: bool,
}

/// 执行代码搜索（内嵌引擎，无外部 rg 依赖）。
///
/// 委托给 [`super::search_engine::execute_embedded_search`]（纯 Rust：
/// `ignore` 遍历 + `regex` 匹配，语义与 ripgrep 对齐）。
///
/// - `total` = 收集到的记录总数（含守卫丢弃前的上限 5000）。
/// - `truncated` = `offset + head_limit < total`（还有更多结果可翻页）。
/// - `exhausted` = `offset >= total`（已无更多结果）。
pub async fn execute_search_code(
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, TianyanError> {
    super::search_engine::execute_embedded_search(pattern, options)
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
