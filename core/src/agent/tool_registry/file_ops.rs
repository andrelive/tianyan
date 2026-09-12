//! 文件类工具执行器：read_file / write_file / vfs_read / vfs_list。
//!
//! 每个工具包含"安全检查 + 审批 + 执行"三步，顺序与行为与拆分前一致。

use crate::agent::tool_params::{
    ApplyEditParams, ApplyPatchParams, ReadFileParams, VfsListParams, VfsReadParams,
    WriteFileParams,
};
use crate::common::error::TianyanError;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::executor::Action;

use super::{parse_params, safety_violation, vfs_content_field, wrap_tool_error, ToolRegistry};

impl ToolRegistry {
    /// 执行 read_file 工具：读取文件内容。
    ///
    /// 相对路径按会话工作目录解析（[`Self::resolve_tool_path`]）。
    pub(crate) async fn execute_read_file(
        &self,
        arguments: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ReadFileParams = parse_params(arguments)?;
        let path = self.resolve_tool_path(session_id, &params.path).await;
        safety_violation(self.security_policy.check_path(std::path::Path::new(&path)))?;
        crate::executor::execute_read_file(&path, params.offset, params.limit)
            .await
            .map_err(wrap_tool_error)
    }

    /// 执行 write_file 工具：写入文件内容（含审批工作流门控）。
    pub(crate) async fn execute_write_file(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let mut params: WriteFileParams = parse_params(arguments)?;
        // 相对路径按会话工作目录解析（与 read_file 同一套归属规则）
        params.path = self.resolve_tool_path(session_id, &params.path).await;
        safety_violation(self.security_policy.check_file_write())?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        safety_violation(
            self.security_policy
                .check_file_size(params.content.len() as u64),
        )?;
        // 审批门控（自动放行安全路径/拒绝关键路径/请求人类确认，
        // 统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::WriteFile {
                path: params.path.clone(),
                content: params.content.clone(),
            },
        )
        .await?;
        crate::executor::execute_write_file(&params.path, &params.content)
            .await
            .map_err(wrap_tool_error)
    }

    /// 执行 apply_edit 工具：哈希锚定行编辑（含审批工作流门控）。
    pub(crate) async fn execute_apply_edit(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let mut params: ApplyEditParams = parse_params(arguments)?;
        params.path = self.resolve_tool_path(session_id, &params.path).await;
        safety_violation(self.security_policy.check_file_write())?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]；
        // edits 序列化为审批动作负载，与 execute_apply_edit 行为一致）
        let edits: Vec<serde_json::Value> = params
            .edits
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        self.ensure_approved(
            session_id,
            subagent,
            &Action::ApplyEdit {
                path: params.path.clone(),
                edits,
            },
        )
        .await?;
        crate::executor::edit::apply_edit_action(&params.path, params.edits)
            .await
            .map_err(wrap_tool_error)
            .map(|mut result| {
                self.attach_lsp_diagnostics(&params.path, &mut result);
                result
            })
    }

    /// 执行 apply_patch 工具：统一 diff 补丁应用（含审批工作流门控）。
    pub(crate) async fn execute_apply_patch(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ApplyPatchParams = parse_params(arguments)?;
        safety_violation(self.security_policy.check_file_write())?;
        // 补丁的**全部**目标文件逐一参与路径安全检查（旧实现只检查首个文件
        // 路径，多文件补丁的后续文件可借此绕过沙箱写入白名单外路径）。
        // 相对路径按测试/执行进程 CWD 解析，与 executor 层 base_dir 一致；
        // 绝对路径按字面检查（executor 层随后统一拒绝绝对路径）。
        let patch_paths = crate::executor::patch::collect_patch_paths(&params.patch);
        let first_path = patch_paths.first().cloned().unwrap_or_default();
        // 补丁基准目录：会话绑定的工作目录优先，缺省回退进程 cwd
        // （与 read_file/write_file 同一套归属规则；统一 resolve_base_dir）
        let base_dir = self.resolve_base_dir(session_id).await?;
        for path in &patch_paths {
            let resolved = if std::path::Path::new(path).is_absolute() {
                std::path::PathBuf::from(path)
            } else {
                base_dir.join(path)
            };
            safety_violation(self.security_policy.check_path(&resolved))?;
        }
        // 审批门控（统一序列见 [`ToolRegistry::ensure_approved`]）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::ApplyPatch {
                path: first_path.clone(),
                patch: params.patch.clone(),
            },
        )
        .await?;
        crate::executor::patch::apply_patch_action(&params.patch, &base_dir)
            .await
            .map_err(wrap_tool_error)
            .map(|mut result| {
                if !first_path.is_empty() {
                    self.attach_lsp_diagnostics(&first_path, &mut result);
                }
                result
            })
    }

    /// 编辑成功后附加 LSP 推送诊断（"LSP errors detected, please fix" 模式）。
    ///
    /// 仅在注入 [`LspManager`] 时生效且尽力而为：诊断来自服务器
    /// `textDocument/publishDiagnostics` 推送（需先有服务器被启动并打开该文档）；
    /// 无记录时附加空数组，绝不阻断编辑结果。
    fn attach_lsp_diagnostics(&self, path: &str, result: &mut serde_json::Value) {
        let Some(manager) = &self.lsp_manager else {
            return;
        };
        let diagnostics = manager.diagnostics_for(path);
        if let serde_json::Value::Object(map) = result {
            map.insert(
                "diagnostics".to_string(),
                serde_json::to_value(diagnostics)
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
            );
        }
    }

    /// 执行 vfs_read 工具：读取 VFS 条目的三层内容。
    pub(crate) async fn execute_vfs_read(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VfsReadParams = parse_params(arguments)?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "VFS not configured"))
        })?;
        let parsed = TianyanUri::parse(&params.uri)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：无效 URI: {}", e)))?;
        // ADR-018 兼容层：会话内容已迁出 VFS（SQLite 权威存储）——
        // 对 tianyan://session/{id} 特判，经 SessionStore 导出 JSONL 返回，
        // 保持 LLM「读取压缩前原始记录」的能力与提示语不变。
        if parsed.namespace() == ContextNamespace::Session {
            let session_id = parsed.path().last().cloned().unwrap_or_default();
            let Some(store) = &self.session_store else {
                return Err(TianyanError::Custom(
                    "tool: 执行失败：会话存储未配置".to_string(),
                ));
            };
            let detail = match store.export_jsonl(&session_id).await {
                Ok(Some(jsonl)) => jsonl,
                Ok(None) => String::new(),
                Err(e) => return Err(wrap_tool_error(e)),
            };
            // 体量治理：会话 JSONL 可达数 MB，整读会压垮上下文——头部截断
            //（50KB / 2000 行）+ 提示（本路径无 offset 续读参数，指引改用检索）。
            let truncated = crate::executor::truncate::truncate_head_noted(
                &detail,
                "；如需特定内容，可用 session_recall 按关键词检索",
            );
            return Ok(serde_json::json!({
                "uri": params.uri,
                "abstract": "",
                "overview": "",
                "detail": truncated.text,
                "truncated": truncated.truncated,
                "total_bytes": truncated.total_bytes,
            }));
        }
        // 加载三层内容，让 LLM 按需使用
        let (l0, l1, l2) = tokio::join!(
            vfs.read_content(&parsed, ContentLevel::Abstract),
            vfs.read_content(&parsed, ContentLevel::Overview),
            vfs.read_content(&parsed, ContentLevel::Detail),
        );
        Ok(serde_json::json!({
            "uri": params.uri,
            "abstract": vfs_content_field(l0),
            "overview": vfs_content_field(l1),
            "detail": vfs_content_field(l2),
        }))
    }

    /// 执行 vfs_list 工具：列出 VFS 目录条目。
    pub(crate) async fn execute_vfs_list(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VfsListParams = parse_params(arguments)?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "VFS not configured"))
        })?;
        let target_uri = match params.uri.as_deref() {
            Some(s) => TianyanUri::parse(s)
                .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：无效 URI: {}", e)))?,
            None => TianyanUri::new(ContextNamespace::Knowledge, vec![]),
        };
        let entries = vfs.list(&target_uri).await.map_err(wrap_tool_error)?;
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "uri": e.uri().to_string(),
                    "is_directory": e.is_directory(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "uri": target_uri.to_string(),
            "count": items.len(),
            "entries": items,
        }))
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
