//! symbol_outline 工具执行器：安全检查 + 委托 + 错误包装。
//!
//! 与 fs_ops 相同的三步模式：`parse_params` → `check_path` 安全校验 →
//! 委托 `crate::executor::symbols` → 统一 `tool: 执行失败` 错误包装。
//!
//! 输出 JSON：
//! `{ "path", "language", "symbols": [...], "count", "truncated", "errors" }`

use crate::agent::tool_params::SymbolOutlineParams;
use crate::common::error::TianyanError;

use super::{parse_params, safety_violation, wrap_tool_error, ToolRegistry};

impl ToolRegistry {
    /// 执行 symbol_outline 工具：提取源码文件的结构化符号大纲。
    pub(crate) async fn execute_symbol_outline(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SymbolOutlineParams = parse_params(arguments)?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        let language = crate::executor::symbols::language_from_extension(&params.path)
            .map_err(wrap_tool_error)?;
        // 异步读取：工具执行路径不得阻塞运行时线程（与 file_ops 约定一致）
        let source = tokio::fs::read_to_string(&params.path)
            .await
            .map_err(|e| wrap_tool_error(e.into()))?;
        let outline = crate::executor::symbols::symbol_outline(&source, &language)
            .map_err(wrap_tool_error)?;
        serde_json::to_value(serde_json::json!({
            "path": params.path,
            "language": outline.language,
            "symbols": outline.symbols,
            "count": outline.symbols.len(),
            "truncated": outline.truncated,
            "errors": outline.errors,
        }))
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "symbol_ops_tests.rs"]
mod tests;
