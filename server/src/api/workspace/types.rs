//! 工作区类型定义
//!
//! 定义工作区 API 的请求/响应 DTO（frozen 契约）：只读（tree/read/diff）与
//! 编辑（apply-patch / apply-edit，编程工作台 Phase 2）。

use serde::{Deserialize, Serialize};
use tianyan::executor::edit::EditSpec;

/// 目录树响应。
#[derive(Debug, Clone, Serialize)]
pub struct TreeResponse {
    /// 工作目录绝对路径。
    pub root: String,
    /// 请求的相对路径（空字符串表示根）。
    pub path: String,
    /// 单层条目列表。
    pub entries: Vec<TreeEntry>,
}

/// 目录树条目。
#[derive(Debug, Clone, Serialize)]
pub struct TreeEntry {
    /// 条目名称。
    pub name: String,
    /// 条目类型：`dir` | `file`。
    #[serde(rename = "type")]
    pub entry_type: String,
    /// 相对工作目录的路径。
    pub path: String,
    /// 文件大小（仅文件，目录不携带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// 修改时间（unix 毫秒，仅文件，目录不携带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
}

/// 单文件差异响应（快照单文件模式与文件间模式共用同一形状）。
#[derive(Debug, Clone, Serialize)]
pub struct FileDiffResponse {
    /// 相对路径。
    pub path: String,
    /// 差异状态：`modified` | `added` | `removed` | `binary` | `unchanged`。
    pub status: String,
    /// 变更块。
    pub hunks: Vec<HunkDto>,
    /// unified diff 文本（binary / unchanged 为空）。
    pub unified: String,
    /// 旧内容行数。
    pub old_lines: usize,
    /// 新内容行数。
    pub new_lines: usize,
}

/// 变更块坐标（1 起始，与 core snapshot 的 [`tianyan::snapshot::Hunk`] 形状一致）。
#[derive(Debug, Clone, Serialize)]
pub struct HunkDto {
    /// 旧内容起始行（1 起始）。
    pub old_start: usize,
    /// 旧内容行数。
    pub old_len: usize,
    /// 新内容起始行（1 起始）。
    pub new_start: usize,
    /// 新内容行数。
    pub new_len: usize,
}

/// tree 查询参数。
#[derive(Debug, Clone, Deserialize)]
pub struct TreeQuery {
    /// 相对路径（空 = 工作目录根）。
    pub path: Option<String>,
    /// 深度（当前固定单层列出，参数保留以兼容前端）。
    pub depth: Option<u32>,
}

/// read 查询参数。
#[derive(Debug, Clone, Deserialize)]
pub struct ReadQuery {
    /// 相对路径（空 = 工作目录根，返回目录模式）。
    pub path: Option<String>,
    /// 起始行（1 起始）。
    pub offset: Option<usize>,
    /// 窗口行数。
    pub limit: Option<usize>,
}

/// diff 查询参数（快照模式与文件间模式二选一）。
#[derive(Debug, Clone, Deserialize)]
pub struct DiffQuery {
    /// 相对路径（快照模式：单文件过滤；省略 = 返回整个差异列表）。
    pub path: Option<String>,
    /// 模式标记：`snapshot`。
    pub base: Option<String>,
    /// 会话 ID（快照模式）。
    pub session_id: Option<String>,
    /// 快照索引（快照模式）。
    pub index: Option<usize>,
    /// 文件间模式：左侧相对路径。
    pub path_a: Option<String>,
    /// 文件间模式：右侧相对路径。
    pub path_b: Option<String>,
}

/// apply-patch 请求体（前端保存文件的主通道）。
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyPatchRequest {
    /// unified diff 补丁文本（codex 风格 `*** Update File:` 信封格式）。
    pub patch: String,
}

/// apply-patch 响应中单个文件的变更摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchFileDto {
    /// 相对工作目录的路径（补丁头部声明的原样路径）。
    pub path: String,
    /// 应用的补丁块数。
    pub hunks_applied: usize,
    /// 变更行数（删除 + 新增行合计）。
    pub lines_changed: usize,
}

/// apply-patch 响应体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchResponse {
    /// 各文件变更摘要。
    pub files: Vec<ApplyPatchFileDto>,
    /// 变更文件总数。
    pub total_files: usize,
}

/// apply-edit 请求体（hashline 语义编辑，供 LLM/外部工作流）。
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyEditRequest {
    /// 相对工作目录的路径。
    pub path: String,
    /// 语义编辑列表（复用 core [`EditSpec`] 的 serde 契约）。
    pub edits: Vec<EditSpec>,
}

/// apply-edit 响应体。
#[derive(Debug, Clone, Serialize)]
pub struct ApplyEditResponse {
    /// 相对工作目录的路径（回显请求值）。
    pub path: String,
    /// 实际应用的编辑条数。
    pub edits_applied: usize,
}
