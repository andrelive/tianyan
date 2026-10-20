//! 代码搜索的**参数契约**（工具参数与执行器共用同一类型）。
//!
//! 为什么与实现分家（ADR-049 期 1）：`search` / `search_engine`（遍历 + 正则匹配）
//! 迁至 `plugins/tools-code`，但工具参数 schema（`SearchCodeParams`）与输出契约
//! 里的 `output_mode` 字段仍由 core 定义——参数类型是**契约**，不是实现。
//! 留在这里，插件反向引用 `tianyan::executor::search_types::*`，core 不依赖插件。

use serde::{Deserialize, Serialize};

/// 分页默认条数。
pub const DEFAULT_HEAD_LIMIT: usize = 100;

/// 分页硬上限（clamp 用）：模型请求更大值会被钳制——
/// 2026-09-29 输出治理（单次工具结果的体量与上下文成本直接挂钩）。
pub const MAX_HEAD_LIMIT: usize = 200;

/// 输出模式（工具参数与执行器共用同一类型；非法值在参数解析期即被拒绝）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// 仅列出命中的文件名（默认）。
    #[default]
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
}

/// 搜索选项（与工具参数一一对应；`None`/`false` 取默认行为）。
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// 搜索根目录（默认当前工作目录）。
    pub path: Option<String>,
    /// glob 过滤器（如 `*.rs`）。
    pub glob: Option<String>,
    /// 输出模式（`None` 取默认 [`OutputMode::FilesWithMatches`]）。
    pub output_mode: Option<OutputMode>,
    /// 文件类型过滤器（rg `--type`，如 "rust"）。
    pub type_: Option<String>,
    /// 忽略大小写（`-i`）。
    pub ignore_case: bool,
    /// 结果条数上限（默认 [`DEFAULT_HEAD_LIMIT`]）。
    pub head_limit: Option<usize>,
    /// 分页偏移（0 起始）。
    pub offset: usize,
    /// 多行匹配（`-U`）。
    pub multiline: bool,
}
