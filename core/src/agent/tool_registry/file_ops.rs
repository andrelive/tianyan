//! 文件类工具执行器：read_file / write_file / vfs_read / vfs_list。
//!
//! 每个工具包含"安全检查 + 审批 + 执行"三步，顺序与行为与拆分前一致。

use crate::agent::tool_params::{ReadFileParams, VfsListParams, VfsReadParams, WriteFileParams};
use crate::common::error::TianyanError;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::executor::approval::ApprovalDecision;
use crate::executor::Action;

use super::{parse_params, safety_violation, vfs_content_field, ToolRegistry};

impl ToolRegistry {
    /// 执行 read_file 工具：读取文件内容。
    pub(crate) async fn execute_read_file(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ReadFileParams = parse_params(arguments)?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        crate::executor::execute_read_file(&params.path)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 write_file 工具：写入文件内容（含审批工作流门控）。
    pub(crate) async fn execute_write_file(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: WriteFileParams = parse_params(arguments)?;
        safety_violation(self.security_policy.check_file_write())?;
        safety_violation(
            self.security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        safety_violation(
            self.security_policy
                .check_file_size(params.content.len() as u64),
        )?;
        // Approval workflow check — auto-approve safe paths,
        // deny critical paths (e.g. /etc/, .env), request human
        // approval for Medium/High risk paths.
        if let Some(ref approval) = self.approval_workflow {
            let action = Action::WriteFile {
                path: params.path.clone(),
                content: params.content.clone(),
            };
            let resp = approval
                .request_approval("tool-execution", &action)
                .await
                .map_err(|e| {
                    TianyanError::Custom(format!("tool: 执行失败：审批工作流错误: {}", e))
                })?;
            if resp.decision != ApprovalDecision::Approve {
                // 记录待确认操作：用户通过"询问用户"链路批准后放行
                self.remember_pending_approval(&action).await;
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规：操作需要用户确认：{}",
                    resp.reason.unwrap_or_default()
                )));
            }
        }
        crate::executor::execute_write_file(&params.path, &params.content)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
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
        let entries = vfs
            .list(&target_uri)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
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
