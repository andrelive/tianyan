//! 代码类工具执行器：grep / run_tests / verify_build。

use crate::agent::tool_params::{RunTestsParams, SearchCodeParams, VerifyBuildParams};
use crate::common::error::TianyanError;
use crate::executor::search::{execute_search_code as execute_search, SearchOptions};
use crate::executor::Action;

use super::{parse_params, safety_violation, wrap_tool_error, ToolRegistry};

impl ToolRegistry {
    /// 执行 grep 工具：正则搜索文件内容（内嵌引擎，无外部 rg 依赖）。
    ///
    /// 四步：参数解析 → 搜索根解析（显式 `path` 相对路径按会话工作目录解析，
    /// 缺省为会话工作目录——与 glob/read_file 同一套归属规则；无会话时回退
    /// 进程 cwd 语义）→ 路径安全检查（校验解析后的搜索根是否在允许/禁止
    /// 目录范围内）→ 委托 [`execute_search`] 执行。
    pub(crate) async fn execute_search_code(
        &self,
        arguments: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SearchCodeParams = parse_params(arguments)?;
        let resolved_root = match &params.path {
            Some(dir) => self.resolve_tool_path(session_id, dir).await,
            None => self
                .resolve_base_dir(session_id)
                .await?
                .to_string_lossy()
                .into_owned(),
        };
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&resolved_root)),
        )?;
        let mut options = search_options(&params);
        options.path = Some(resolved_root);
        execute_search(&params.pattern, &options)
            .await
            .map_err(wrap_tool_error)
    }

    /// 执行 run_tests 工具：运行测试命令（设计 D6 增强结果解析）。
    ///
    /// 命令解析（见 [`crate::executor::test_discovery::run_tests_action`]）：
    /// 显式 `command` 原样使用（向后兼容，LLM 有完全控制权）；
    /// 缺省时按 `cwd` 项目探测（Cargo → `cargo test`、Python → `pytest`、
    /// TypeScript → `vitest run`）结合 `framework`/`suite`/`filter` 构建默认命令。
    ///
    /// 安全门控与 execute_command 对齐：cwd 路径沙箱 + 解析后的完整命令
    /// check_command（拦截注入元字符/黑名单命令）+ 审批工作流。
    pub(crate) async fn execute_run_tests(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RunTestsParams = parse_params(arguments)?;
        // Path sandbox for working directory（与 execute_command 一致）
        if let Some(ref cwd) = params.cwd {
            safety_violation(self.security_policy.check_path(std::path::Path::new(cwd)))?;
        }
        // 先解析出实际将执行的完整命令（显式 command 或 探测+模板拼接），
        // 再对其整体执行命令安全检查——filter/suite 拼接注入在此被拦截。
        let resolved = crate::executor::test_discovery::resolve_run_tests_command(
            params.command.as_deref(),
            params.cwd.as_deref(),
            params.framework.as_deref(),
            params.filter.as_deref(),
            params.suite.as_deref(),
        )
        .map_err(wrap_tool_error)?;
        safety_violation(self.security_policy.check_command(&resolved))?;
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::RunTests {
                command: resolved.clone(),
                cwd: params.cwd.clone(),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // 显式传入已解析命令（命令解析对同一输入幂等，门控与执行严格一致）
        crate::executor::test_discovery::run_tests_action(
            Some(&resolved),
            params.cwd.as_deref(),
            params.timeout_secs,
            None,
            None,
            None,
        )
        .await
        .map_err(wrap_tool_error)
    }

    /// 执行 verify_build 工具：构建验证（语义验证或退出码回退）。
    ///
    /// 安全门控与 execute_command 对齐：cwd 路径沙箱 + 命令 check_command +
    /// 审批工作流。
    pub(crate) async fn execute_verify_build(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VerifyBuildParams = parse_params(arguments)?;
        // Path sandbox for working directory（与 execute_command 一致）
        if let Some(ref cwd) = params.cwd {
            safety_violation(self.security_policy.check_path(std::path::Path::new(cwd)))?;
        }
        safety_violation(self.security_policy.check_command(&params.command))?;
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::VerifyBuild {
                command: params.command.clone(),
                cwd: params.cwd.clone(),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // Use semantic verification if available, otherwise fall back
        // to exit code + pattern matching.
        if let Some(ref gate) = self.verification_gate {
            let result = gate
                .verify_build(&params.command, params.cwd.as_deref(), params.timeout_secs)
                .await
                .map_err(wrap_tool_error)?;
            Ok(serde_json::Value::from(result))
        } else {
            crate::executor::execute_verify_build(
                &params.command,
                params.cwd.as_deref(),
                params.timeout_secs,
            )
            .await
            .map_err(wrap_tool_error)
        }
    }
}

/// 将工具参数转换为搜索选项（`None`/`false` 保持默认行为，由执行器解析）。
fn search_options(params: &SearchCodeParams) -> SearchOptions {
    SearchOptions {
        path: params.path.clone(),
        glob: params.glob.clone(),
        output_mode: params.output_mode.clone(),
        type_: params.type_.clone(),
        ignore_case: params.ignore_case.unwrap_or(false),
        line_number: params.line_number,
        context: params.context,
        before_context: params.before_context,
        after_context: params.after_context,
        head_limit: params.head_limit,
        offset: params.offset.unwrap_or(0),
        multiline: params.multiline.unwrap_or(false),
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "code_ops_tests.rs"]
mod tests;
