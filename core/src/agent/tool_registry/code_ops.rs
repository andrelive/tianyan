//! 代码类工具执行器：search_code / run_tests / verify_build。

use crate::agent::tool_params::{RunTestsParams, SearchCodeParams, VerifyBuildParams};
use crate::common::error::TianyanError;

use super::{parse_params, ToolRegistry};

impl ToolRegistry {
    /// 执行 search_code 工具：使用 ripgrep 搜索代码模式。
    pub(crate) async fn execute_search_code(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SearchCodeParams = parse_params(arguments)?;
        crate::executor::execute_search_code(&params.query, params.scope.as_deref())
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 run_tests 工具：运行测试命令。
    pub(crate) async fn execute_run_tests(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RunTestsParams = parse_params(arguments)?;
        crate::executor::execute_run_tests(
            &params.command,
            params.cwd.as_deref(),
            params.timeout_secs,
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 verify_build 工具：构建验证（语义验证或退出码回退）。
    pub(crate) async fn execute_verify_build(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VerifyBuildParams = parse_params(arguments)?;
        // Use semantic verification if available, otherwise fall back
        // to exit code + pattern matching.
        if let Some(ref gate) = self.verification_gate {
            let result = gate
                .verify_build(&params.command, params.cwd.as_deref(), params.timeout_secs)
                .await
                .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
            Ok(serde_json::Value::from(result))
        } else {
            crate::executor::execute_verify_build(
                &params.command,
                params.cwd.as_deref(),
                params.timeout_secs,
            )
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
        }
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "code_ops_tests.rs"]
mod tests;
