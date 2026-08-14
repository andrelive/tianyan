//! 工作区领域模块
//!
//! 本模块提供代码工作区的访问端点（design G9 后端）：
//! - `tree`：目录树浏览（单层条目，目录在前、文件在后）
//! - `read`：文件读取（委托 core executor 的锚点行读取）
//! - `diff`：差异对比（快照对比 / 文件间对比）
//! - `apply-patch`：补丁应用（编程工作台保存主通道）
//! - `apply-edit`：hashline 语义编辑（LLM/外部工作流）

/// 工作区请求处理函数
pub mod handlers;
/// 工作区路由定义
pub mod routes;
/// 工作区业务逻辑
pub mod services;
/// 工作区类型定义
pub mod types;

pub use routes::routes;
pub use types::{
    ApplyEditRequest, ApplyEditResponse, ApplyPatchFileDto, ApplyPatchRequest, ApplyPatchResponse,
    DiffQuery, DirEntry, DirsQuery, DirsResponse, FileDiffResponse, HunkDto, ReadQuery, TreeEntry,
    TreeQuery, TreeResponse,
};

#[cfg(test)]
mod tests;
