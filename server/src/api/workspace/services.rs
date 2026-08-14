//! 工作区业务逻辑
//!
//! 只读浏览工作目录（目录树单层列出、文件读取、差异对比），以及编程工作台
//! 编辑（apply-patch 补丁应用、apply-edit hashline 语义编辑，均委托 core
//! executor 并映射错误到 HTTP 语义）。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use tianyan::session::SessionManager;

use serde_json::{json, Value};
use tianyan::executor::edit::EditSpec;
use tianyan::executor::execute_read_file;
use tianyan::snapshot::SnapshotManager;
use tianyan::TianyanError;

use crate::api::shared::error::ApiError;
use crate::api::workspace::types::{
    ApplyEditResponse, ApplyPatchResponse, DirEntry, DirsResponse, FileDiffResponse, HunkDto,
    TreeEntry, TreeResponse,
};

/// 工作区服务：对会话生效的工作目录（会话级绑定优先，缺省全局配置）提供只读访问。
pub struct WorkspaceService {
    /// 会话管理器（按 session_id 解析工作区归属）。
    session_manager: Arc<dyn SessionManager>,
    /// 全局默认工作目录（[agent] working_directory；会话未绑定时使用）。
    default_working_dir: Option<PathBuf>,
    /// 快照管理器（配置未设置时为 None）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
}

impl WorkspaceService {
    /// 创建新的工作区服务。
    pub fn new(
        session_manager: Arc<dyn SessionManager>,
        default_working_dir: Option<PathBuf>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
    ) -> Self {
        Self {
            session_manager,
            default_working_dir,
            snapshot_manager,
        }
    }

    /// 解析请求生效的工作目录：会话级绑定（目录必须存在）优先，
    /// 缺省回退全局配置。会话不存在 → 404；未配置任何目录 → 400。
    async fn resolve_workdir(&self, session_id: Option<&str>) -> Result<PathBuf, ApiError> {
        if let Some(sid) = session_id.map(str::trim).filter(|s| !s.is_empty()) {
            let session = self
                .session_manager
                .get_session(sid)
                .await?
                .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {sid}")))?;
            if let Some(wd) = session.header.working_directory.as_deref() {
                let p = PathBuf::from(wd);
                if p.is_dir() {
                    return Ok(p);
                }
                return Err(ApiError::BadRequest(format!("会话工作目录不存在：{wd}")));
            }
        }
        self.default_working_dir
            .clone()
            .ok_or_else(|| ApiError::BadRequest("未配置 working_directory".to_string()))
    }

    /// 列出 `<working_dir>/<rel>` 的单层条目（目录在前、文件在后，各自按名称升序）。
    ///
    /// `rel` 为空表示工作目录根。`depth` 参数当前固定为单层列出（与契约一致）。
    pub async fn tree(
        &self,
        rel: &str,
        session_id: Option<&str>,
    ) -> Result<TreeResponse, ApiError> {
        let base = self.resolve_workdir(session_id).await?;
        let (base, target) = resolve_rel(&base, rel).await?;
        let mut rd = tokio::fs::read_dir(&target)
            .await
            .map_err(|e| ApiError::NotFound(format!("工作区目录不存在：{e}")))?;
        let mut entries = Vec::new();
        while let Some(entry) = rd.next_entry().await? {
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
        session_id: Option<&str>,
    ) -> Result<Value, ApiError> {
        let base = self.resolve_workdir(session_id).await?;
        let (_base, target) = resolve_rel(&base, rel).await?;
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
        // 快照根取该会话生效的工作目录（会话级绑定优先，缺省全局配置）
        let workdir = self.resolve_workdir(Some(session_id)).await?;
        let result = manager
            .with_workdir(workdir)
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
    pub async fn diff_files(
        &self,
        path_a: &str,
        path_b: &str,
        session_id: Option<&str>,
    ) -> Result<Value, ApiError> {
        let base = self.resolve_workdir(session_id).await?;
        let (_base, target_a) = resolve_rel(&base, path_a).await?;
        let (_, target_b) = resolve_rel(&base, path_b).await?;
        let bytes_a = tokio::fs::read(&target_a)
            .await
            .map_err(|e| ApiError::NotFound(format!("文件不存在：{e}")))?;
        let bytes_b = tokio::fs::read(&target_b)
            .await
            .map_err(|e| ApiError::NotFound(format!("文件不存在：{e}")))?;
        if tianyan::common::binary::sniff_binary(&bytes_a)
            || tianyan::common::binary::sniff_binary(&bytes_b)
        {
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

    /// 应用 unified diff 补丁（委托 core executor 原子多文件落盘）。
    ///
    /// 接受 git 风格（jsdiff `createTwoFilesPatch` 产物）与 codex 风格两种补丁，
    /// git 风格先归一化为 codex 信封（见 [`normalize_patch`]）。补丁内路径由
    /// core 内部沙箱校验（拒绝 `..` 组件与绝对路径，相对 `working_directory`
    /// 解析），此处不重复校验。未配置工作目录 → 400。
    pub async fn apply_patch(
        &self,
        patch_text: &str,
        session_id: Option<&str>,
    ) -> Result<ApplyPatchResponse, ApiError> {
        let working_dir = self.resolve_workdir(session_id).await?;
        let normalized = normalize_patch(patch_text)?;
        let value = tianyan::executor::patch::apply_patch_action(&normalized, &working_dir)
            .await
            .map_err(executor_error_to_api)?;
        serde_json::from_value(value).map_err(ApiError::from)
    }

    /// 应用 hashline 语义编辑（委托 core executor 锚点校验 + 原子落盘）。
    ///
    /// 先经 [`Self::resolve`] 沙箱解析绝对路径（文件不存在 → 404），再交给
    /// core；响应路径回显请求的相对路径（前端契约）。
    pub async fn apply_edit(
        &self,
        rel: &str,
        edits: Vec<EditSpec>,
        session_id: Option<&str>,
    ) -> Result<ApplyEditResponse, ApiError> {
        let base = self.resolve_workdir(session_id).await?;
        let (_base, target) = resolve_rel(&base, rel).await?;
        let abs = strip_verbatim(&target.to_string_lossy());
        let value = tianyan::executor::edit::apply_edit_action(&abs, edits)
            .await
            .map_err(executor_error_to_api)?;
        let edits_applied = value
            .get("edits_applied")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ApiError::Internal("executor: apply_edit: 响应缺少 edits_applied".to_string())
            })? as usize;
        Ok(ApplyEditResponse {
            path: rel.to_string(),
            edits_applied,
        })
    }

    /// 目录选择器：列出 `path` 下的子目录（供前端逐级浏览选择工作目录）。
    ///
    /// - `path` 缺省/空 → 浏览根：Windows 返回盘符列表（`C:\`、`D:\`...），
    ///   其余平台返回家目录下的子目录（parent = None）。
    /// - 指定路径 → 列出该目录的子目录（parent = 上级目录，根目录时 None）。
    /// - 与 [`Self::tree`] 不同：这是**任意路径浏览**（工作区选择器用），
    ///   不限于当前 working_directory；只列目录不列文件；目录不存在 → 404。
    pub async fn list_dirs(&self, path: Option<&str>) -> Result<DirsResponse, ApiError> {
        let path = path.map(str::trim).filter(|p| !p.is_empty());
        let Some(path) = path else {
            // 浏览根：Windows 返回盘符列表；其余平台返回家目录下的子目录
            return self.browse_root().await;
        };

        let target = PathBuf::from(path);
        if !target.is_dir() {
            return Err(ApiError::NotFound(format!("目录不存在：{path}")));
        }
        let canonical = tokio::fs::canonicalize(&target)
            .await
            .map_err(|e| ApiError::NotFound(format!("目录不存在：{e}")))?;
        let current = strip_verbatim(&canonical.to_string_lossy());
        let parent = canonical
            .parent()
            .map(|p| strip_verbatim(&p.to_string_lossy()));
        let entries = read_subdirs(&canonical, false)?;
        Ok(DirsResponse {
            current,
            parent,
            entries,
        })
    }

    /// 浏览根：Windows 盘符列表（A:-Z: 存在即列出）；其余平台家目录子目录。
    async fn browse_root(&self) -> Result<DirsResponse, ApiError> {
        #[cfg(target_os = "windows")]
        {
            let mut entries = Vec::new();
            for letter in b'A'..=b'Z' {
                let drive = format!("{}:\\", letter as char);
                if Path::new(&drive).exists() {
                    entries.push(DirEntry {
                        name: drive.clone(),
                        path: drive,
                        is_root: Some(true),
                    });
                }
            }
            Ok(DirsResponse {
                current: "浏览根".to_string(),
                parent: None,
                entries,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let home =
                dirs::home_dir().ok_or_else(|| ApiError::Internal("无法解析家目录".to_string()))?;
            let entries = read_subdirs(&home, true)?;
            Ok(DirsResponse {
                current: home.to_string_lossy().into_owned(),
                parent: None,
                entries,
            })
        }
    }
}

/// 校验相对路径并解析为绝对路径（基于给定的工作目录根）。
///
/// 拒绝 `..` / 绝对路径（400）；目标必须存在（404）；canonicalize 后必须
/// 仍位于工作目录内（沙箱，越界 → Forbidden）。
async fn resolve_rel(base: &Path, rel: &str) -> Result<(PathBuf, PathBuf), ApiError> {
    validate_rel_path(rel)?;
    let target = base.join(rel);
    let base = tokio::fs::canonicalize(base)
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

/// 列出目录的直接子目录（只列目录，不列文件；目录名按升序）。
///
/// `is_root_entries`：浏览根模式下条目本身即盘符/家目录（不再展开）。
fn read_subdirs(dir: &Path, is_root_entries: bool) -> Result<Vec<DirEntry>, ApiError> {
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| ApiError::NotFound(format!("目录不存在：{e}")))?;
    for entry in rd.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_path = entry.path();
        entries.push(DirEntry {
            name: name.clone(),
            path: strip_verbatim(&entry_path.to_string_lossy()),
            is_root: is_root_entries.then_some(true),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// 去掉 Windows `\\?\` 逐字前缀（canonicalize 产物），返回前端可用的常规绝对路径。
fn strip_verbatim(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// core executor 错误 → API 错误（语义谓词优先，与 core `TianyanError` 单一来源对齐）：
/// - 目标不存在（条目/目录未找到）→ 404；
/// - 冲突（锚点/旧内容/补丁定位不匹配）→ 409（前端据此提示"文件已被修改，请重新加载"）；
/// - 无效输入（非法路径、绝对路径）→ 400；
/// - 其余 → 500。
fn executor_error_to_api(err: TianyanError) -> ApiError {
    if err.is_not_found() {
        ApiError::NotFound(err.to_string())
    } else if err.is_conflict() {
        ApiError::Conflict(err.to_string())
    } else if err.is_invalid_input() {
        ApiError::BadRequest(err.to_string())
    } else {
        ApiError::Internal(err.to_string())
    }
}

/// 将 git 风格 unified diff（jsdiff `createTwoFilesPatch` 产物）归一化为
/// core `parse_patch` 接受的 codex `*** Update File:` 信封；输入已是 codex
/// 风格时原样透传。
///
/// git 风格（jsdiff 输出）：
/// ```text
/// --- a/src/main.rs
/// +++ b/src/main.rs
/// @@ -1,3 +1,4 @@
///  context
/// -old
/// +new
/// ```
/// 转换规则：`--- a/<path>` 头 → `*** Update File: <path>`（去 `a/`/`b/`
/// 前缀与 jsdiff 的引号转义）；`+++ b/<path>` 头删除；其余行（`@@` 块头、
/// 块体行、`\ No newline` 标记）原样保留。`--- ` / `+++ ` 只在进入 `@@`
/// 块之前被识别为文件头——块体中的删除行是 `-` 加原文（原文再以 `---`
/// 开头时呈现为 `---- `，四连 `-`），不会误判。
fn normalize_patch(patch_text: &str) -> Result<String, ApiError> {
    if patch_text.trim_start().starts_with("*** Update File:") {
        return Ok(patch_text.to_string());
    }
    let mut out = String::with_capacity(patch_text.len());
    let mut in_hunk = false;
    let mut saw_header = false;
    for line in patch_text.split('\n') {
        if !in_hunk {
            if let Some(rest) = line.strip_prefix("--- ") {
                saw_header = true;
                out.push_str("*** Update File: ");
                out.push_str(&strip_ab_prefix(rest.trim()));
                out.push('\n');
                continue;
            }
            if line.starts_with("+++ ") {
                continue;
            }
        }
        if line.starts_with("@@") {
            in_hunk = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !saw_header {
        return Err(ApiError::BadRequest(
            "补丁格式无效：缺少 --- a/ 文件头或 *** Update File 信封".to_string(),
        ));
    }
    Ok(out)
}

/// 去掉 jsdiff 路径的 `a/` / `b/` 前缀与引号转义（路径含空格时 jsdiff 用
/// 引号包裹）。前缀缺失时返回原样。
fn strip_ab_prefix(path: &str) -> String {
    let p = path.trim().trim_matches('"');
    match p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => p.to_string(),
    }
}

/// 快照错误 → API 错误：快照缺失（`is_not_found` 语义）→ 404，其余 → 500。
fn snapshot_error_to_api(err: TianyanError) -> ApiError {
    if err.is_not_found() {
        ApiError::NotFound(err.to_string())
    } else {
        ApiError::Internal(err.to_string())
    }
}
