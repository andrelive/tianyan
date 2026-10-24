//! tools-fs 插件：文件系统工具（`read_file` / `write_file` / `apply_edit` /
//! `apply_patch` / `glob` / `list_dir` / `view_image`）。
//!
//! ## 为什么是独立 crate（ADR-049 期 1）
//!
//! 本 crate 是「改一行工具只重编插件 + bin」的**验证对象**（闸门 A）。工具的
//! schema、执行逻辑、实现本体全部在这里——core 只保留注册机制与 `ToolContext`
//! 契约。改本 crate 的任何一行，`tianyan-core` 与 `tianyan-server` 都不需要重编。
//!
//! ## 依赖面
//!
//! 只依赖 `tianyan-core`（kernel-api 抽取在期 3）：
//! - [`ToolContext`]——路径归属解析（会话工作目录）+ 安全策略
//! - [`DynamicToolExecutor`]——既有插件契约（ADR-049 D8：不新建 Plugin 四件套）
//! - [`GlobParams`] / [`ListDirParams`]（以及 `ViewImageParams`）——参数 schema
//!   单一事实源（core 侧定义）
//! - `parse_params` / `safety_violation` / `wrap_tool_error`——工具错误口径单点
//!
//! ## 实现归属（过渡形态，ADR-049 期 1 记录）
//!
//! 工具**定义 + 参数校验 + 安全门控 + 错误包装 + 结果序列化**全在本 crate
//! （改一行工具行为 = 只重编插件）；底层遍历算法暂留 core `executor::fs`
//! （`execute_glob` / `execute_list_dir`）——因为 core 的 `read_file` 目录模式
//! 反向依赖 `execute_list_dir`（"唯一目录列举实现"），fs 域必须整域迁移才能
//! 完全切断。期 1 收尾时随 read_file/write_file/edit/patch 一起搬。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::PathBuf;

use async_trait::async_trait;
use tianyan::agent::{
    parse_params, safety_violation, wrap_tool_error, ApplyEditParams, ApplyPatchParams,
    DynamicToolExecutor, GlobParams, ListDirParams, ReadFileParams, ToolContext, WriteFileParams,
};
use tianyan::common::error::TianyanError;
use tianyan::executor::fs::{execute_glob, execute_list_dir};
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

mod view_image;

pub use view_image::ViewImageTool;

/// `glob` 工具：按模式查找文件（遵循 `.gitignore`，按修改时间倒序）。
pub struct GlobTool {
    /// 工具上下文（装配层从 `ToolRegistry::context()` 取共享句柄）。
    ctx: ToolContext,
}

impl GlobTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for GlobTool {
    fn tool_name(&self) -> String {
        "glob".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<GlobParams>(
            "glob",
            "在目录下按 glob 模式找文件（如 **/*.rs），按修改时间倒序。遵循 .gitignore。",
        ))
    }

    /// 与迁移前内置表一致（`ToolPresentation::Search`）——展示意图丢失会让
    /// 前端把文件搜索渲染成通用卡片。
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Search
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: GlobParams = parse_params(arguments)?;
        // 安全校验基于解析后的实际搜索根（path 参数或会话工作目录）
        let base: PathBuf = match params.path {
            Some(p) => PathBuf::from(self.ctx.resolve_tool_path(session_id, &p).await),
            None => self.ctx.resolve_base_dir(session_id).await?,
        };
        safety_violation(self.ctx.security_policy.check_path(&base))?;
        let output = execute_glob(&params.pattern, Some(&base))
            .await
            .map_err(wrap_tool_error)?;
        serde_json::to_value(output)
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }
}

/// `list_dir` 工具：列出单层目录条目（目录在前，支持 offset/limit 分页）。
pub struct ListDirTool {
    /// 工具上下文（装配层从 `ToolRegistry::context()` 取共享句柄）。
    ctx: ToolContext,
}

impl ListDirTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for ListDirTool {
    fn tool_name(&self) -> String {
        "list_dir".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<ListDirParams>(
            "list_dir",
            "列出单层目录下的条目。目录带尾部 '/'。支持 offset/limit 分页。",
        ))
    }

    /// 与迁移前内置表一致（`ToolPresentation::Search`）。
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Search
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: ListDirParams = parse_params(arguments)?;
        let path = self.ctx.resolve_tool_path(session_id, &params.path).await;
        safety_violation(
            self.ctx
                .security_policy
                .check_path(std::path::Path::new(&path)),
        )?;
        let output = execute_list_dir(std::path::Path::new(&path), params.offset, params.limit)
            .await
            .map_err(wrap_tool_error)?;
        serde_json::to_value(output)
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }
}

/// 本插件提供的全部工具（装配层一行注册用）。
///
/// 顺序即注册顺序：`glob` 在前、`list_dir` 在后，与迁移前内置表顺序一致
/// （前缀稳定性：工具定义是提示词前缀的一部分）。**新工具一律追加末尾**
/// （重排既有前缀会让工具 schema 前缀变化 → prompt 缓存失效），例如
/// `view_image`（ADR-050）排在最后。
pub fn tools(ctx: &ToolContext) -> Vec<std::sync::Arc<dyn DynamicToolExecutor>> {
    vec![
        std::sync::Arc::new(ReadFileTool::new(ctx.clone())),
        std::sync::Arc::new(WriteFileTool::new(ctx.clone())),
        std::sync::Arc::new(ApplyEditTool::new(ctx.clone())),
        std::sync::Arc::new(ApplyPatchTool::new(ctx.clone())),
        std::sync::Arc::new(GlobTool::new(ctx.clone())),
        std::sync::Arc::new(ListDirTool::new(ctx.clone())),
        std::sync::Arc::new(ViewImageTool::new(ctx.clone())),
    ]
}

/// 插件清单工厂（`ToolPluginFactory` 的等价物；各 bin / 桌面端共用）。
///
/// **单一事实源**：装配端只写 `Box::new(tianyan_tools_fs::plugin_factory())`，
/// 不各自列举工具——新增工具只改本 crate，装配端零改动。
pub fn plugin_factory() -> impl FnOnce(&ToolContext) -> Vec<std::sync::Arc<dyn DynamicToolExecutor>>
{
    tools
}

/// `read_file` 工具：读取文件内容（大文件按 offset/limit 取窗口）。
pub struct ReadFileTool {
    ctx: ToolContext,
}

impl ReadFileTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for ReadFileTool {
    fn tool_name(&self) -> String {
        "read_file".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<ReadFileParams>(
            "read_file",
            "读取文本文件内容（纯内容，无行号/哈希前缀）。默认返回前 2000 行（单次输出上限约 50KB，超出部分截断并附「使用 offset 继续」提示）。大文件请**按需读取**：用 offset/limit 指定行范围（1 起始行号），建议先用 grep/symbol_outline 定位目标区域再按范围精读；结果含 total_lines/total_bytes 与实际窗口（showing）。内容匹配编辑（apply_edit）直接按内容定位，无需行号。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Read
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: ReadFileParams = parse_params(arguments)?;
        let path = self.ctx.resolve_tool_path(session_id, &params.path).await;
        safety_violation(
            self.ctx
                .security_policy
                .check_path(std::path::Path::new(&path)),
        )?;
        tianyan::executor::execute_read_file(&path, params.offset, params.limit)
            .await
            .map_err(wrap_tool_error)
    }
}

/// `write_file` 工具：写入文件内容（自动创建父目录 + 审批门控）。
pub struct WriteFileTool {
    ctx: ToolContext,
}

impl WriteFileTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }

    /// 收集「写入时将要新建」的父目录链（缺失的最深层 → 已存在的祖先之下）。
    fn missing_parent_dirs(path: &str) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        let Some(parent) = std::path::Path::new(path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        else {
            return missing;
        };
        let mut cur = Some(parent);
        while let Some(p) = cur {
            if p.exists() {
                break;
            }
            missing.push(p.display().to_string());
            cur = p.parent().filter(|q| !q.as_os_str().is_empty());
        }
        missing.reverse(); // 自外层向内，便于阅读
        missing
    }

    /// 写入实现（`subagent` 决定审批语义：ADR-011 子任务非交互审批）。
    async fn write(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let mut params: WriteFileParams = parse_params(arguments)?;
        params.path = self.ctx.resolve_tool_path(session_id, &params.path).await;
        safety_violation(self.ctx.security_policy.check_file_write())?;
        safety_violation(
            self.ctx
                .security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        safety_violation(
            self.ctx
                .security_policy
                .check_file_size(params.content.len() as u64),
        )?;
        // 目录语义（2026-10 修订）：自动创建缺失父目录 + 结果回显新建目录
        let created_dirs = Self::missing_parent_dirs(&params.path);
        self.ctx
            .ensure_approved(
                session_id,
                subagent,
                &tianyan::executor::Action::WriteFile {
                    path: params.path.clone(),
                    content: params.content.clone(),
                },
            )
            .await?;
        tianyan::executor::execute_write_file(&params.path, &params.content, true)
            .await
            .map_err(wrap_tool_error)?;
        Ok(serde_json::json!({
            "path": params.path,
            "message": "写入成功",
            "bytes": params.content.len(),
            "created_dirs": created_dirs,
        }))
    }
}

#[async_trait]
impl DynamicToolExecutor for WriteFileTool {
    fn tool_name(&self) -> String {
        "write_file".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<WriteFileParams>(
            "write_file",
            "将内容写入指定路径的文件（覆盖写入；文件不存在则创建；父目录不存在时**自动创建**——返回结果的 `created_dirs` 会列出本次新建的目录，若与预期不符请及时清理）。**临时/中间产物**（提交信息文件、一次性脚本、审计产物等）请写系统临时目录的会话区（如 `$env:TEMP\\\\tianyan-scratch\\\\`），**不要**写应用数据目录根或安装目录。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Write
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.write(session_id, arguments, false).await
    }

    async fn execute_with_scope(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.write(session_id, arguments, subagent).await
    }
}

/// `apply_edit` 工具：内容匹配编辑（`old_string`/`new_string` 唯一匹配，原子批量）。
pub struct ApplyEditTool {
    ctx: ToolContext,
}

impl ApplyEditTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }

    /// 编辑实现（`subagent` 决定审批语义：ADR-011 子任务非交互审批）。
    async fn edit(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let mut params: ApplyEditParams = parse_params(arguments)?;
        params.path = self.ctx.resolve_tool_path(session_id, &params.path).await;
        safety_violation(self.ctx.security_policy.check_file_write())?;
        safety_violation(
            self.ctx
                .security_policy
                .check_path(std::path::Path::new(&params.path)),
        )?;
        // 审批门控：edits 序列化为审批动作负载（与内置实现一致）
        let edits: Vec<serde_json::Value> = params
            .edits
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        self.ctx
            .ensure_approved(
                session_id,
                subagent,
                &tianyan::executor::Action::ApplyEdit {
                    path: params.path.clone(),
                    edits,
                },
            )
            .await?;
        tianyan::executor::edit::apply_edit_action(&params.path, params.edits)
            .await
            .map_err(wrap_tool_error)
    }
}

#[async_trait]
impl DynamicToolExecutor for ApplyEditTool {
    fn tool_name(&self) -> String {
        "apply_edit".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<ApplyEditParams>(
            "apply_edit",
            "**仅用于能精确复现原文的极小改动**（单个唯一片段，如改一行）。在文件内容中查找 old_string（须唯一，多处出现需 replace_all）替换为 new_string。注意：old_string 必须**逐字节精确匹配**（含空行与缩进）——漏空行、抄错缩进都会失败；若修改区域含空行/上下文、或改动较大，请改用 apply_patch（上下文锚定，更稳）。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Diff
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.edit(session_id, arguments, false).await
    }

    async fn execute_with_scope(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.edit(session_id, arguments, subagent).await
    }
}

/// `apply_patch` 工具：统一 diff 补丁应用（多文件原子 + 全目标路径安全检查）。
pub struct ApplyPatchTool {
    ctx: ToolContext,
}

impl ApplyPatchTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }

    /// 补丁应用实现（`subagent` 决定审批语义：ADR-011 子任务非交互审批）。
    async fn apply(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: ApplyPatchParams = parse_params(arguments)?;
        safety_violation(self.ctx.security_policy.check_file_write())?;
        // 补丁基准目录：会话绑定工作目录优先，缺省回退进程 cwd
        let base_dir = self.ctx.resolve_base_dir(session_id).await?;
        // 补丁的**全部**目标文件逐一参与安全检查（与落盘对象同源：都经 patch_targets）
        let targets = tianyan::executor::patch::patch_targets(&params.patch, &base_dir)
            .map_err(wrap_tool_error)?;
        let first_path = targets.first().map(|t| t.raw.clone()).unwrap_or_default();
        for target in &targets {
            safety_violation(self.ctx.security_policy.check_path(&target.resolved))?;
        }
        self.ctx
            .ensure_approved(
                session_id,
                subagent,
                &tianyan::executor::Action::ApplyPatch {
                    path: first_path.clone(),
                    patch: params.patch.clone(),
                },
            )
            .await?;
        tianyan::executor::patch::apply_patch_action(&params.patch, &base_dir)
            .await
            .map_err(wrap_tool_error)
    }
}

#[async_trait]
impl DynamicToolExecutor for ApplyPatchTool {
    fn tool_name(&self) -> String {
        "apply_patch".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<ApplyPatchParams>(
            "apply_patch",
            "对一个或多个文件做修改的**主力编辑工具**。格式：`*** Update File: <路径>` 后直接跟 `-`（删除）/ `+`（新增）/ 空格（上下文）行，只写要改的行 + 少量上下文，不要复现整个文件。`<路径>` 可为相对会话工作目录的相对路径，或绝对路径（与 read_file / write_file 同一口径）。`@@` 块头**可省略**（工具按内容定位，无需行号）。`-`/上下文行须与当前文件内容一致。**块内空行**：整行无字符的空行按格式噪声忽略；若原文该行为空、需作为上下文行，须写成「一个空格」的单独一行（上下文行以空格前缀书写，内容可为空）——写成裸空行会让期望匹配窗口少一行。小改、大改、单文件、多文件均适用；编辑前建议先 read_file 目标区确认当前内容。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Diff
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.apply(session_id, arguments, false).await
    }

    async fn execute_with_scope(
        &self,
        session_id: &str,
        arguments: &str,
        subagent: bool,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        self.apply(session_id, arguments, subagent).await
    }
}
