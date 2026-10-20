//! 文件类工具执行器：vfs_read / vfs_list / apply_edit / apply_patch。
//!
//! `read_file` / `write_file` 已迁至 `plugins/tools-fs`（ADR-049 期 1）。
//! 每个工具包含"安全检查 + 审批 + 执行"三步，顺序与行为与拆分前一致。

use crate::agent::tool_params::{VfsListParams, VfsReadParams};
use crate::common::error::TianyanError;
use crate::common::truncate::truncate_utf8_boundary;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::executor::truncate::truncate_head;

use super::{parse_params, vfs_content_field, wrap_tool_error};
use crate::agent::tool_context::ToolContext;

/// vfs_list 条目中 L0 简介的截断上限（字节，UTF-8 边界安全）——目录的"含义"
/// 不需要超长简介（L0 契约约 100 token；此为防御生成失败回退路径的长文本）。
const LIST_ABSTRACT_MAX_BYTES: usize = 480;

impl ToolContext {
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
        // 正文（detail）窗口：ADR-001 修订后正文按章节取段——未指定 offset/limit
        // 时头部截断至 2000 行/50KB（附"使用 offset 继续"，正文不再无上限倾泻）；
        // 指定时按行取段（对齐目录 L1 中的章节行号）。
        let window = detail_window(&l2.unwrap_or_default(), params.offset, params.limit)?;
        Ok(serde_json::json!({
            "uri": params.uri,
            "abstract": vfs_content_field(l0),
            "overview": vfs_content_field(l1),
            "detail": window.text,
            "offset": window.start_line,
            "total_lines": window.total_lines,
            "truncated": window.truncated,
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
        // 目录条目（"书的目录"）= uri + 类型 + **子条目 L0 简介**：含义即简介，
        // 随 list 一并返回（本地无新增查询）；目录节点/未生成时省略该字段。
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                let mut item = serde_json::json!({
                    "uri": e.uri().to_string(),
                    "is_directory": e.is_directory(),
                });
                let abstract_text = e.abstract_content.as_deref().unwrap_or("").trim();
                if !abstract_text.is_empty() {
                    item["abstract"] = serde_json::json!(truncate_utf8_boundary(
                        abstract_text,
                        LIST_ABSTRACT_MAX_BYTES
                    ));
                }
                item
            })
            .collect();
        Ok(serde_json::json!({
            "uri": target_uri.to_string(),
            "count": items.len(),
            "entries": items,
        }))
    }
}

/// 正文行窗口结果。
struct DetailWindow {
    text: String,
    start_line: usize,
    total_lines: usize,
    truncated: bool,
}

/// 正文（detail）行窗口（与 `read_file` 的 offset/limit 语义对齐：offset 1 起始、
/// limit 默认 2000、单窗口钳制 2000）：
/// - 未指定 offset/limit：头部截断至 2000 行 / 50KB（[`truncate_head`]，附
///   "使用 offset 继续"标记）——正文不再无上限倾泻；
/// - 指定 offset/limit：取窗口行并附续读提示；offset 超出总行数报错（对齐 read_file）。
fn detail_window(
    detail: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<DetailWindow, TianyanError> {
    if offset.is_none() && limit.is_none() {
        let t = truncate_head(detail);
        return Ok(DetailWindow {
            text: t.text,
            start_line: 1,
            total_lines: t.total_lines,
            truncated: t.truncated,
        });
    }
    let lines: Vec<&str> = detail.lines().collect();
    let total_lines = lines.len();
    let start = offset.unwrap_or(1).max(1) - 1; // 1 起始；offset 0 视为 1
    if total_lines > 0 && start >= total_lines {
        return Err(TianyanError::Custom(format!(
            "tool: 参数无效：offset {} 超出正文总行数 {}",
            start + 1,
            total_lines
        )));
    }
    let limit = limit.unwrap_or(2000).clamp(1, 2000);
    let end = (start + limit).min(total_lines);
    let truncated = end < total_lines;
    let mut text = lines[start.min(total_lines)..end].join("\n");
    if truncated {
        text.push_str(&format!(
            "\n... (输出已截断，共 {total_lines} 行，使用 offset {} 继续)",
            end + 1
        ));
    }
    Ok(DetailWindow {
        text,
        start_line: start + 1,
        total_lines,
        truncated,
    })
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
