//! LSP 工具执行器（execute_lsp）。

use std::sync::Arc;

use crate::agent::tool_params::LspParams;
use crate::agent::tool_registry::ToolRegistry;
use crate::common::error::{Result, TianyanError};
use crate::lsp::diagnostics::LspManager;

/// lsp 工具支持的操作集合。
const LSP_OPERATIONS: &[&str] = &[
    "goToDefinition",
    "findReferences",
    "hover",
    "documentSymbol",
    "workspaceSymbol",
    "goToImplementation",
];

impl ToolRegistry {
    /// 执行 lsp 工具：查询语言服务器（跳转/悬停/符号等）。
    ///
    /// 参数校验：未知操作或缺少 file_path（workspaceSymbol 缺少 query）返回
    /// `tool: 参数无效`；执行失败返回 `tool: 执行失败：{detail}`。
    /// 未注入 [`LspManager`] 时惰性新建（每次调用独立实例；注入后共享
    /// 服务池与诊断存储，编辑类工具的 diagnostics 附加才能生效）。
    pub(crate) async fn execute_lsp(&self, arguments: &str) -> Result<serde_json::Value> {
        let params: LspParams = super::parse_params(arguments)?;
        let operation = params.operation.as_str();
        if !LSP_OPERATIONS.contains(&operation) {
            return Err(TianyanError::Custom(format!(
                "tool: 参数无效：未知操作：{operation}"
            )));
        }
        if params.file_path.is_none() || (operation == "workspaceSymbol" && params.query.is_none())
        {
            return Err(TianyanError::Custom(
                "tool: 参数无效：需要 file_path（workspaceSymbol 需要 query）".to_string(),
            ));
        }
        let manager = self
            .lsp_manager
            .clone()
            .unwrap_or_else(|| Arc::new(LspManager::new()));
        let file_path = params.file_path.clone().unwrap_or_default();
        let results = manager
            .query(
                &file_path,
                &params.operation,
                params.line,
                params.character,
                params.query.as_deref(),
            )
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
        Ok(serde_json::json!({
            "operation": params.operation,
            "file_path": file_path,
            "results": results,
        }))
    }
}

#[cfg(test)]
#[path = "lsp_ops_tests.rs"]
mod tests;
