//! 代码类工具执行器：grep / run_tests / verify_build。

use crate::agent::tool_params::{
    RunProjectTestsParams, RunTestsParams, SearchCodeParams, VerifyBuildParams,
};
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

    /// 解析 run_tests / verify_build 的生效工作目录（T1-1）。
    ///
    /// 显式 `cwd` 相对路径按会话工作目录解析；**缺省即会话工作目录**——
    /// 与 grep/glob/read_file 同一套归属规则（旧实现缺省落进程 cwd：桌面
    /// 应用=安装目录，项目探测与执行都在错误目录，表现为"找不到项目"）。
    /// 无会话绑定时回退进程 cwd 语义（[`Self::resolve_base_dir`]）。
    pub(crate) async fn resolve_tool_cwd(
        &self,
        session_id: &str,
        cwd: Option<&str>,
    ) -> Result<String, TianyanError> {
        match cwd {
            Some(dir) => Ok(self.resolve_tool_path(session_id, dir).await),
            None => Ok(self
                .resolve_base_dir(session_id)
                .await?
                .to_string_lossy()
                .into_owned()),
        }
    }

    /// 执行 run_tests 工具：运行**显式测试命令**（设计 D6 增强结果解析）。
    ///
    /// cwd 归属（T1-1）：显式 `cwd` 相对路径按会话工作目录解析；**缺省即
    /// 会话工作目录**——与 grep/glob/read_file 同一套规则（旧实现缺省落
    /// 进程 cwd：桌面应用=安装目录，项目探测与执行都在错误目录）。
    ///
    /// 命令原样使用（LLM 完全控制权）；「按项目探测构造命令」是另一个工具
    /// [`Self::execute_run_project_tests`] 的职责——此前两者挤在同一工具里
    /// （`command` 优先、缺省才探测）= 同一意图两种写法，故按单一职责拆开。
    ///
    /// 安全门控与 execute_command 对齐：cwd 路径沙箱 + 命令 check_command
    /// （拦截注入元字符/黑名单命令）+ 审批工作流。
    pub(crate) async fn execute_run_tests(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RunTestsParams = parse_params(arguments)?;
        // cwd 归属（T1-1）：显式值按会话工作目录解析、缺省用会话工作目录；
        // 沙箱与审批/执行均用**解析后**的目录（门控与执行同一口径）
        let cwd = self
            .resolve_tool_cwd(session_id, params.cwd.as_deref())
            .await?;
        safety_violation(self.security_policy.check_path(std::path::Path::new(&cwd)))?;
        let command = params.command.trim().to_string();
        if command.is_empty() {
            return Err(TianyanError::invalid_input(
                "tool: run_tests 的 command 不能为空（要按项目探测跑测试，用 run_project_tests）",
            ));
        }
        safety_violation(self.security_policy.check_command(&command))?;
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::RunTests {
                command: command.clone(),
                cwd: Some(cwd.clone()),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // 取消感知（ADR-036 补盲区）：从会话取消标志取——长测试期间「停止」
        // 立即杀进程树返回（此前这条路径不可取消）。
        let cancel = self.session_cancel_flag(session_id).await;
        crate::executor::test_discovery::run_tests_action(
            &command,
            Some(&cwd),
            params.timeout_secs,
            cancel,
        )
        .await
        .map_err(wrap_tool_error)
    }

    /// 执行 run_project_tests 工具：探测项目类型 → 构造命令 → 运行。
    ///
    /// cwd 归属与 run_tests 一致；`framework` 覆盖探测结果、`suite`/`filter`
    /// 缩小范围。**先解析出完整命令再整体 check_command**——`filter`/`suite`
    /// 的拼接注入在此被拦截（门控与执行严格同一命令）。
    ///
    /// 安全门控与 execute_command 对齐。
    pub(crate) async fn execute_run_project_tests(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RunProjectTestsParams = parse_params(arguments)?;
        let cwd = self
            .resolve_tool_cwd(session_id, params.cwd.as_deref())
            .await?;
        safety_violation(self.security_policy.check_path(std::path::Path::new(&cwd)))?;
        // 先解析出实际将执行的完整命令（探测 + 模板拼接），
        // 再对其整体执行命令安全检查——filter/suite 拼接注入在此被拦截。
        let resolved = crate::executor::test_discovery::resolve_project_test_command(
            Some(&cwd),
            params.framework.as_deref(),
            params.filter.as_deref(),
            params.suite.as_deref(),
        )
        .map_err(wrap_tool_error)?;
        safety_violation(self.security_policy.check_command(&resolved))?;
        self.ensure_approved(
            session_id,
            subagent,
            &Action::RunTests {
                command: resolved.clone(),
                cwd: Some(cwd.clone()),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // 取消感知（ADR-036 补盲区）：与 run_tests 同一模式
        let cancel = self.session_cancel_flag(session_id).await;
        crate::executor::test_discovery::run_tests_action(
            &resolved,
            Some(&cwd),
            params.timeout_secs,
            cancel,
        )
        .await
        .map_err(wrap_tool_error)
    }

    /// 执行 verify_build 工具：构建验证（语义验证或退出码回退）。
    ///
    /// cwd 归属（T1-1）：与 run_tests 完全一致——显式值按会话工作目录解析、
    /// 缺省用会话工作目录；沙箱与审批/执行均用解析后的目录。
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
        // cwd 归属（T1-1）：与 run_tests 同一规则
        let cwd = self
            .resolve_tool_cwd(session_id, params.cwd.as_deref())
            .await?;
        safety_violation(self.security_policy.check_path(std::path::Path::new(&cwd)))?;
        safety_violation(self.security_policy.check_command(&params.command))?;
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::VerifyBuild {
                command: params.command.clone(),
                cwd: Some(cwd.clone()),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // Use semantic verification if available, otherwise fall back
        // to exit code + pattern matching.
        // 取消感知（ADR-036 补盲区）：与 run_tests 同一模式
        let cancel = self.session_cancel_flag(session_id).await;
        if let Some(ref gate) = self.verification_gate {
            let result = gate
                .verify_build(&params.command, Some(&cwd), params.timeout_secs, cancel)
                .await
                .map_err(wrap_tool_error)?;
            Ok(serde_json::Value::from(result))
        } else {
            crate::executor::execute_verify_build(
                &params.command,
                Some(&cwd),
                params.timeout_secs,
                cancel,
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
        output_mode: params.output_mode,
        type_: params.type_.clone(),
        ignore_case: params.ignore_case.unwrap_or(false),
        context: params.context,
        head_limit: params.head_limit,
        offset: params.offset.unwrap_or(0),
        multiline: params.multiline.unwrap_or(false),
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "code_ops_tests.rs"]
mod tests;
