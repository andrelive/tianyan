//! 测试发现工具执行器（discover_tests）。

use crate::agent::tool_params::DiscoverTestsParams;
use crate::common::error::TianyanError;

use super::{parse_params, ToolRegistry};

impl ToolRegistry {
    /// 执行 discover_tests 工具：探测项目格式并列出测试（不执行测试）。
    ///
    /// 与 run_tests 一致不做路径安全检查：仅运行只读命令（`cargo test -- --list` /
    /// `pytest --collect-only -q` / `vitest --list`），无破坏性副作用；
    /// 项目根目录由 `probe_project` 从 `path` 向上探测。
    pub(crate) async fn execute_discover_tests(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: DiscoverTestsParams = parse_params(arguments)?;
        if params.path.trim().is_empty() {
            return Err(TianyanError::Custom(
                "tool: 参数无效：path 不能为空".to_string(),
            ));
        }
        crate::executor::test_discovery::discover_tests(&params.path)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }
}
