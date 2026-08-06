//! 工作区业务逻辑
//!
//! 只读浏览工作目录：目录树单层列出、文件读取（委托 core executor 锚点行读取）、
//! 差异对比（快照对比 / 文件间对比）。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use tianyan::executor::execute_read_file;
use tianyan::snapshot::SnapshotManager;
use tianyan::TianyanError;

use crate::api::shared::error::ApiError;
use crate::api::workspace::types::{FileDiffResponse, HunkDto, TreeEntry, TreeResponse};

/// 工作区服务：对配置的 working_directory 提供只读访问。
pub struct WorkspaceService {
    /// 工作目录（配置未设置时为 None）。
    working_dir: Option<PathBuf>,
    /// 快照管理器（配置未设置时为 None）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
}

impl WorkspaceService {
    /// 创建新的工作区服务。
    pub fn new(
        working_dir: Option<PathBuf>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
    ) -> Self {
        Self {
            working_dir,
            snapshot_manager,
        }
    }

    /// 列出 `<working_dir>/<rel>` 的单层条目（目录在前、文件在后，各自按名称升序）。
    ///
    /// `rel` 为空表示工作目录根。`depth` 参数当前固定为单层列出（与契约一致）。
    pub async fn tree(&self, rel: &str) -> Result<TreeResponse, ApiError> {
        let (base, target) = self.resolve(rel).await?;
        let mut rd = tokio::fs::read_dir(&target)
            .await
            .map_err(|e| ApiError::NotFound(format!("工作区目录不存在：{e}")))?;
        let mut entries = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ApiError::Internal(format!("工作区目录读取失败：{e}")))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            let entry_path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if is_dir {
                entries.push(TreeEntry {
                    name,
                    entry_type: "dir".to_string(),
                    path: entry_path,
                    size: None,
                    mtime: None,
                });
            } else {
                let meta = entry.metadata().await.ok();
                entries.push(TreeEntry {
                    name,
                    entry_type: "file".to_string(),
                    path: entry_path,
                    size: meta.as_ref().map(|m| m.len()),
                    mtime: meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64),
                });
            }
        }
        // 目录在前、文件在后，各自按名称升序（"dir" < "file"）
        entries.sort_by(|a, b| a.entry_type.cmp(&b.entry_type).then(a.name.cmp(&b.name)));
        Ok(TreeResponse {
            root: strip_verbatim(&base.to_string_lossy()),
            path: rel.to_string(),
            entries,
        })
    }

    /// 读取 `<working_dir>/<rel>` 文件内容（委托 core executor 的锚点行读取）。
    ///
    /// 返回 core executor 的结构化 `Value`（文本窗口 / 二进制 / 目录模式由 core 处理），
    /// 其中 `path` 为解析后的绝对路径。
    pub async fn read(
        &self,
        rel: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<Value, ApiError> {
        let (_base, target) = self.resolve(rel).await?;
        let value = execute_read_file(&strip_verbatim(&target.to_string_lossy()), offset, limit)
            .await
            .map_err(executor_error_to_api)?;
        Ok(value)
    }

    /// 快照模式差异：`SnapshotManager::diff(session_id, index)`。
    ///
    /// `path` 省略时返回整个 `{ "files": [...] }`；指定时返回单文件差异
    /// （快照中不存在该文件或内容未变 → `unchanged`）。
    pub async fn diff_snapshot(
        &self,
        session_id: &str,
        index: usize,
        path: Option<&str>,
    ) -> Result<Value, ApiError> {
        let manager = self
            .snapshot_manager
            .clone()
            .ok_or_else(|| ApiError::BadRequest("未配置 working_directory".to_string()))?;
        if let Some(rel) = path {
            validate_rel_path(rel)?;
        }
        let result = manager
            .diff(session_id, index)
            .await
            .map_err(snapshot_error_to_api)?;
        match path {
            Some(rel) => {
                let found = result.files.iter().find(|f| f.path == rel);
                let value = match found {
                    Some(f) => serde_json::to_value(f).map_err(ApiError::from)?,
                    None => json!({
                        "path": rel,
                        "status": "unchanged",
                        "hunks": [],
                        "unified": "",
                        "old_lines": 0,
                        "new_lines": 0,
                    }),
                };
                Ok(value)
            }
            None => Ok(json!({ "files": result.files })),
        }
    }

    /// 文件间模式差异：`<working_dir>/<path_a>` vs `<working_dir>/<path_b>`。
    ///
    /// 使用 `similar::TextDiff` 按行对比；任一文件为二进制 → `binary`。
    pub async fn diff_files(&self, path_a: &str, path_b: &str) -> Result<Value, ApiError> {
        let (_base, target_a) = self.resolve(path_a).await?;
        let (_, target_b) = self.resolve(path_b).await?;
        let bytes_a = tokio::fs::read(&target_a)
            .await
            .map_err(|e| ApiError::NotFound(format!("文件不存在：{e}")))?;
        let bytes_b = tokio::fs::read(&target_b)
            .await
            .map_err(|e| ApiError::NotFound(format!("文件不存在：{e}")))?;
        if is_binary(&bytes_a) || is_binary(&bytes_b) {
            return Ok(json!({
                "path": path_a,
                "status": "binary",
                "hunks": [],
                "unified": "",
                "old_lines": 0,
                "new_lines": 0,
            }));
        }
        let text_a = String::from_utf8_lossy(&bytes_a);
        let text_b = String::from_utf8_lossy(&bytes_b);
        let diff = similar::TextDiff::from_lines(text_a.as_ref(), text_b.as_ref());
        let ops: &[similar::DiffOp] = diff.ops();
        let status = if ops.iter().any(|op| op.tag() != similar::DiffTag::Equal) {
            "modified"
        } else {
            "unchanged"
        };
        let hunks: Vec<HunkDto> = ops
            .iter()
            .filter(|op| op.tag() != similar::DiffTag::Equal)
            .map(|op| HunkDto {
                old_start: op.old_range().start + 1,
                old_len: op.old_range().len(),
                new_start: op.new_range().start + 1,
                new_len: op.new_range().len(),
            })
            .collect();
        let unified = if status == "unchanged" {
            String::new()
        } else {
            diff.unified_diff()
                .header(&format!("a/{path_a}"), &format!("b/{path_b}"))
                .to_string()
        };
        let response = FileDiffResponse {
            path: path_a.to_string(),
            status: status.to_string(),
            hunks,
            unified,
            old_lines: text_a.lines().count(),
            new_lines: text_b.lines().count(),
        };
        serde_json::to_value(response).map_err(ApiError::from)
    }

    /// 校验相对路径并解析为绝对路径。
    ///
    /// 拒绝 `..` / 绝对路径（400）；目标必须存在（404）；canonicalize 后必须
    /// 仍位于工作目录内（沙箱，越界 → Forbidden）。
    async fn resolve(&self, rel: &str) -> Result<(PathBuf, PathBuf), ApiError> {
        let working_dir = self
            .working_dir
            .clone()
            .ok_or_else(|| ApiError::BadRequest("未配置 working_directory".to_string()))?;
        validate_rel_path(rel)?;
        let target = working_dir.join(rel);
        let base = tokio::fs::canonicalize(&working_dir)
            .await
            .map_err(|e| ApiError::NotFound(format!("工作目录不存在：{e}")))?;
        let target = tokio::fs::canonicalize(&target)
            .await
            .map_err(|e| ApiError::NotFound(format!("路径不存在：{e}")))?;
        if !target.starts_with(&base) {
            return Err(ApiError::Forbidden("路径超出工作目录".to_string()));
        }
        Ok((base, target))
    }
}

/// 拒绝包含 `..` 组件或以根/前缀（绝对路径、盘符）开头的相对路径。
fn validate_rel_path(rel: &str) -> Result<(), ApiError> {
    let path = Path::new(rel);
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ApiError::BadRequest(format!("无效路径：{rel}")));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// 去掉 Windows `\\?\` 逐字前缀（canonicalize 产物），返回前端可用的常规绝对路径。
fn strip_verbatim(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// core executor 错误 → API 错误：文件缺失 → 404，其余 → 500。
fn executor_error_to_api(err: TianyanError) -> ApiError {
    let msg = err.to_string();
    if msg.contains("文件不存在") {
        ApiError::NotFound(msg)
    } else {
        ApiError::Internal(msg)
    }
}

/// 快照错误 → API 错误：快照缺失 → 404，其余 → 500。
fn snapshot_error_to_api(err: TianyanError) -> ApiError {
    let msg = err.to_string();
    if msg.contains("快照") {
        ApiError::NotFound(msg)
    } else {
        ApiError::Internal(msg)
    }
}

/// 文件间对比的二进制判定（与 core `sniff_binary` 的主信号一致：NUL 字节）。
fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}
