//! LSP 工具执行器（execute_lsp）。
//!
//! 参数形态收敛：`operation` 是类型化枚举（未知操作在解析期即被拒绝）、
//! `file_path` 必填；"操作—参数"匹配校验集中在 [`to_lsp_query`] 一处，
//! 缺失即报错——不再有静默退化（旧形态对缺 line/character 的请求按 0:0
//! 查询，让"漏参"伪装成"空结果"）。

use std::sync::Arc;

use crate::agent::tool_params::LspParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::common::error::{Result, TianyanError};
use crate::lsp::diagnostics::{LspManager, LspOperation, LspQuery};

impl ToolRegistry {
    /// 执行 lsp 工具：查询语言服务器（跳转/悬停/符号等）。
    ///
    /// 参数校验：位置类操作缺少 line/character、workspaceSymbol 缺少 query
    /// 返回 `tool: 参数无效`；执行失败返回 `tool: 执行失败：{detail}`。
    /// 未注入 [`LspManager`] 时惰性新建（每次调用独立实例；注入后共享
    /// 服务池与诊断存储，编辑类工具的 diagnostics 附加才能生效）。
    pub(crate) async fn execute_lsp(&self, arguments: &str) -> Result<serde_json::Value> {
        let params: LspParams = super::parse_params(arguments)?;
        let request = to_lsp_query(&params)?;
        let manager = self
            .lsp_manager
            .clone()
            .unwrap_or_else(|| Arc::new(LspManager::new()));
        let results = manager
            .query(&params.file_path, request)
            .await
            .map_err(super::wrap_tool_error)?;
        Ok(serde_json::json!({
            "operation": params.operation.as_str(),
            "file_path": params.file_path,
            "results": results,
        }))
    }
}

/// 工具参数 → 类型化查询请求（"操作—参数"匹配校验的**单点**）。
///
/// 位置类操作必须给 line 与 character；workspaceSymbol 必须给 query。
/// 返回 [`TianyanError::invalid_input`]（ADR-014 语义分类），消息与
/// `parse_params` 同口径（`tool: 参数无效：…`）。
fn to_lsp_query(params: &LspParams) -> Result<LspQuery> {
    let position = |operation: LspOperation| -> Result<(usize, usize)> {
        match (params.line, params.character) {
            (Some(line), Some(character)) => Ok((line, character)),
            _ => Err(TianyanError::invalid_input(format!(
                "tool: 参数无效：{} 需要 line 与 character",
                operation.as_str()
            ))),
        }
    };
    match params.operation {
        LspOperation::GoToDefinition => {
            let (line, character) = position(params.operation)?;
            Ok(LspQuery::GoToDefinition { line, character })
        }
        LspOperation::FindReferences => {
            let (line, character) = position(params.operation)?;
            Ok(LspQuery::FindReferences { line, character })
        }
        LspOperation::Hover => {
            let (line, character) = position(params.operation)?;
            Ok(LspQuery::Hover { line, character })
        }
        LspOperation::GoToImplementation => {
            let (line, character) = position(params.operation)?;
            Ok(LspQuery::GoToImplementation { line, character })
        }
        LspOperation::DocumentSymbol => Ok(LspQuery::DocumentSymbol),
        LspOperation::WorkspaceSymbol => {
            let query = params.query.clone().ok_or_else(|| {
                TianyanError::invalid_input("tool: 参数无效：workspaceSymbol 需要 query")
            })?;
            Ok(LspQuery::WorkspaceSymbol { query })
        }
    }
}

#[cfg(test)]
#[path = "lsp_ops_tests.rs"]
mod tests;
