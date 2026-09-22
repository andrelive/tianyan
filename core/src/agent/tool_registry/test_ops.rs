//! 测试发现工具执行器（discover_tests）。

use crate::agent::tool_params::DiscoverTestsParams;
use crate::common::error::TianyanError;

use super::{parse_params, wrap_tool_error, ToolRegistry};

impl ToolRegistry {
    /// 执行 discover_tests 工具：探测项目格式并列出测试（不执行测试）。
    ///
    /// 与 run_tests 一致不做路径安全检查：仅运行只读命令（`cargo test -- --list` /
    /// `pytest --collect-only -q` / `vitest --list`），无破坏性副作用；
    /// 项目根目录由 `probe_project` 从 `path` 向上探测。
    pub(crate) async fn execute_discover_tests(
        &self,
        arguments: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: DiscoverTestsParams = parse_params(arguments)?;
        if params.path.trim().is_empty() {
            return Err(TianyanError::Custom(
                "tool: 参数无效：path 不能为空".to_string(),
            ));
        }
        // 取消感知（ADR-036 补盲区）：`cargo test -- --list` 首次可能触发全量
        // 编译（分钟级）——「停止」应能中断等待。
        let cancel = self.session_cancel_flag(session_id).await;
        crate::executor::test_discovery::discover_tests(&params.path, cancel)
            .await
            .map_err(wrap_tool_error)
    }
}
