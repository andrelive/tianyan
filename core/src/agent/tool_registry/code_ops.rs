//! 代码类工具执行器：grep（内嵌搜索引擎）。

use crate::agent::tool_params::SearchCodeParams;
use crate::common::error::TianyanError;
use crate::executor::search::{execute_search_code as execute_search, SearchOptions};

use super::{parse_params, safety_violation, wrap_tool_error};
use crate::agent::tool_context::ToolContext;

impl ToolContext {
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
}

/// 将工具参数转换为搜索选项（`None`/`false` 保持默认行为，由执行器解析）。
fn search_options(params: &SearchCodeParams) -> SearchOptions {
    SearchOptions {
        path: params.path.clone(),
        glob: params.glob.clone(),
        output_mode: params.output_mode,
        type_: params.type_.clone(),
        ignore_case: params.ignore_case.unwrap_or(false),
        head_limit: params.head_limit,
        offset: params.offset.unwrap_or(0),
        multiline: params.multiline.unwrap_or(false),
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "code_ops_tests.rs"]
mod tests;
