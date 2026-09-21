//! repo_map 工具执行器：路径归属 + 安全门控 + 缓存扫描 + 骨架渲染。
//!
//! 路径归属与安全门控与 `grep` 同口径（显式 `path` 相对路径按会话工作目录解析，
//! 缺省会话工作目录；`check_path` 校验解析后的根）。

use std::path::{Path, PathBuf};

use crate::agent::tool_params::RepoMapParams;
use crate::common::error::TianyanError;
use crate::executor::repo_map::{self, RenderOptions, ScanOptions};

use super::{parse_params, safety_violation, wrap_tool_error, ToolRegistry};

impl ToolRegistry {
    /// 执行 repo_map 工具：生成跨文件符号骨架（按引用度排序）。
    ///
    /// 四步：参数解析 → 扫描根解析 → 路径安全检查 → 缓存扫描 + 按预算渲染。
    /// 缓存按仓库根 + 文件指纹（路径/mtime/大小）失效，指纹未变直接复用上次
    /// 扫描产物（第二次调用只需 walk + metadata）。
    pub(crate) async fn execute_repo_map(
        &self,
        arguments: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RepoMapParams = parse_params(arguments)?;
        let root = match &params.path {
            Some(dir) => self.resolve_tool_path(session_id, dir).await,
            None => self
                .resolve_base_dir(session_id)
                .await?
                .to_string_lossy()
                .into_owned(),
        };
        safety_violation(self.security_policy.check_path(Path::new(&root)))?;
        let root_path = PathBuf::from(&root);
        repo_map::ensure_root(&root_path).map_err(wrap_tool_error)?;

        let options = ScanOptions {
            root: root_path,
            include_tests: params.include_tests.unwrap_or(false),
        };
        let (outcome, cached) = self.repo_map_cache.get_or_scan(&options);
        let (map, entries, truncated) = repo_map::render(
            &outcome,
            &RenderOptions {
                focus: params.focus.clone(),
                max_bytes: repo_map::resolve_budget(params.max_tokens),
            },
        );
        Ok(serde_json::json!({
            "root": root,
            "files": outcome.files,
            "symbols": outcome.definitions.len(),
            "entries": entries,
            "truncated": truncated,
            "cached": cached,
            "map": map,
        }))
    }
}

#[cfg(test)]
#[path = "repo_map_ops_tests.rs"]
mod tests;
