//! tools-fs 插件：文件系统浏览工具（`glob` / `list_dir`）。
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
//! - [`GlobParams`] / [`ListDirParams`]——参数 schema 单一事实源（core 侧定义）
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
    parse_params, safety_violation, wrap_tool_error, DynamicToolExecutor, GlobParams,
    ListDirParams, ToolContext,
};
use tianyan::common::error::TianyanError;
use tianyan::executor::fs::{execute_glob, execute_list_dir};
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

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
/// （前缀稳定性：工具定义是提示词前缀的一部分）。
pub fn tools(ctx: &ToolContext) -> Vec<std::sync::Arc<dyn DynamicToolExecutor>> {
    vec![
        std::sync::Arc::new(GlobTool::new(ctx.clone())),
        std::sync::Arc::new(ListDirTool::new(ctx.clone())),
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
