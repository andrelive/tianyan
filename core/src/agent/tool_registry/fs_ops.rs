//! glob / list_dir 工具执行器：安全检查 + 委托 + 错误包装。
//!
//! 与 file_ops 相同的三步模式：`parse_params` → `check_path` 安全校验 →
//! 委托 `crate::executor::fs` → 统一 `tool: 执行失败` 错误包装。

use std::path::PathBuf;

use crate::agent::tool_params::{GlobParams, ListDirParams};
use crate::common::error::TianyanError;

use super::{parse_params, safety_violation, ToolRegistry};

impl ToolRegistry {
    /// 执行 glob 工具：按 glob 模式查找文件（默认搜索当前工作目录）。
    pub(crate) async fn execute_glob(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: GlobParams = parse_params(arguments)?;
        // 安全校验基于解析后的实际搜索根（path 参数或当前目录）
        let base: PathBuf = match params.path {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir()
                .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{e}")))?,
        };
        safety_violation(self.security_policy.check_path(&base))?;
        let output = crate::executor::fs::execute_glob(&params.pattern, Some(&base))
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
        serde_json::to_value(output)
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }

    /// 执行 list_dir 工具：列出单个目录层级的条目（目录在前，支持分页）。
    pub(crate) async fn execute_list_dir(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ListDirParams = parse_params(arguments)?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        let output = crate::executor::fs::execute_list_dir(
            std::path::Path::new(&params.path),
            params.offset,
            params.limit,
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
        serde_json::to_value(output)
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "fs_ops_tests.rs"]
mod tests;
