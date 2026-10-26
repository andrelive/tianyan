//! tools-code 插件：代码智能工具（`symbol_outline` / `repo_map` / `grep`）。
//!
//! ## 为什么独立成 crate（ADR-049 期 1）
//!
//! 代码智能的实现带 **tree-sitter + 6 个 grammar** 与 **ignore + regex** 两簇重依赖。
//! 搬进插件后 core 不再承担它们（全量构建的依赖面收窄），且改代码搜索 / 符号提取 /
//! 仓库地图的任何一行只重编本 crate（+ bin）。
//!
//! 与 `tools-fs` 的区别：本域 **server 侧零复用**（普查确认），所以实现可以整域搬走
//! ——不像 fs 域的 `read/edit/patch` 被 workspace API 直接调用。
//!
//! ## 参数契约仍在 core
//!
//! `SearchCodeParams` / `SymbolOutlineParams` / `RepoMapParams` 与
//! `OutputMode` / `SearchOptions`（`tianyan::executor::search_types`）都由 core 定义
//! ——参数类型是契约（工具 schema 与输出字段），不是实现。core 因此不反向依赖本 crate。
//!
//! ## 缓存归属
//!
//! `repo_map` 的扫描缓存（`RepoMapCache`）是**工具内部实现细节**，随实现搬进本 crate
//! 并由工具实例自持——此前它挂在 `ToolContext` 上，会让 core 反向依赖插件。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod repo_map;
pub mod search;
pub mod search_engine;
pub mod symbols;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tianyan::agent::{
    parse_params, safety_violation, wrap_tool_error, DynamicToolExecutor, RepoMapParams,
    SearchCodeParams, SymbolOutlineParams, ToolContext,
};
use tianyan::common::error::TianyanError;
use tianyan::executor::search_types::SearchOptions;
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

use crate::repo_map::{
    ensure_root, render, resolve_budget, RenderOptions, RepoMapCache, ScanOptions,
};
use crate::search::execute_search_code;
use crate::symbols::{language_from_extension, symbol_outline};

// ── symbol_outline ───────────────────────────────────────────────────────

/// `symbol_outline` 工具：用 tree-sitter 提取源码文件的结构化符号大纲。
pub struct SymbolOutlineTool {
    ctx: ToolContext,
}

impl SymbolOutlineTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for SymbolOutlineTool {
    fn tool_name(&self) -> String {
        "symbol_outline".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<SymbolOutlineParams>(
            "symbol_outline",
            "用 tree-sitter 提取源文件的结构大纲（函数/结构体/类/impl/接口/枚举）。支持 Rust、TypeScript/JavaScript、Python、Go。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Code
    }

    async fn execute(
        &self,
        _session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: SymbolOutlineParams = parse_params(arguments)?;
        safety_violation(self.ctx.security_policy.check_path(Path::new(&params.path)))?;
        let language = language_from_extension(&params.path).map_err(wrap_tool_error)?;
        // 异步读取：工具执行路径不得阻塞运行时线程
        let source = tokio::fs::read_to_string(&params.path)
            .await
            .map_err(|e| wrap_tool_error(e.into()))?;
        let outline = symbol_outline(&source, &language).map_err(wrap_tool_error)?;
        serde_json::to_value(serde_json::json!({
            "path": params.path,
            "language": outline.language,
            "symbols": outline.symbols,
            "count": outline.symbols.len(),
            "truncated": outline.truncated,
            "errors": outline.errors,
        }))
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：序列化失败：{e}")))
    }
}

// ── repo_map ─────────────────────────────────────────────────────────────

/// `repo_map` 工具：跨文件符号骨架（按引用度排序）。
pub struct RepoMapTool {
    ctx: ToolContext,
    /// 扫描缓存（按仓库根 + 文件指纹失效）：工具内部实现细节，插件自持。
    cache: Arc<RepoMapCache>,
}

impl RepoMapTool {
    /// 以工具上下文构造（每个工具实例持自己的缓存）。
    pub fn new(ctx: ToolContext) -> Self {
        Self {
            ctx,
            cache: Arc::new(RepoMapCache::new()),
        }
    }
}

#[async_trait]
impl DynamicToolExecutor for RepoMapTool {
    fn tool_name(&self) -> String {
        "repo_map".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<RepoMapParams>(
            "repo_map",
            "生成仓库结构地图：跨文件符号骨架，按「被引用次数」排序——一次调用替代多轮 grep 试探。用途：摸清代码布局与模块划分、定位某功能实现位置、重构前侦察影响面与耦合热点。每行 `路径:行 种类 名称(外部引用数)`；用 focus 聚焦关键字相关子图，用 path 限定子树。**引用度是文本级近似**（不做名称解析：宏展开/重导出/动态分发不可见）——要精确影响面，改完跑 `cargo check`（编译器给精确清单）。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Code
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: RepoMapParams = parse_params(arguments)?;
        let root = match &params.path {
            Some(dir) => self.ctx.resolve_tool_path(session_id, dir).await?,
            None => self
                .ctx
                .resolve_base_dir(session_id)
                .await?
                .to_string_lossy()
                .into_owned(),
        };
        safety_violation(self.ctx.security_policy.check_path(Path::new(&root)))?;
        let root_path = PathBuf::from(&root);
        ensure_root(&root_path).map_err(wrap_tool_error)?;

        let options = ScanOptions {
            root: root_path,
            include_tests: params.include_tests.unwrap_or(false),
        };
        let (outcome, cached) = self.cache.get_or_scan(&options);
        let (map, entries, truncated) = render(
            &outcome,
            &RenderOptions {
                focus: params.focus.clone(),
                max_bytes: resolve_budget(params.max_tokens),
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

// ── grep ─────────────────────────────────────────────────────────────────

/// `grep` 工具：正则搜索文件内容（内嵌引擎，无外部 rg 依赖）。
pub struct GrepTool {
    ctx: ToolContext,
}

impl GrepTool {
    /// 以工具上下文构造。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl DynamicToolExecutor for GrepTool {
    fn tool_name(&self) -> String {
        "grep".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::from_schema::<SearchCodeParams>(
            "grep",
            "用正则搜索文件内容（类似 ripgrep），用于定位。默认只列命中文件名（output_mode=files_with_matches）；要看匹配行传 output_mode=content（返回**匹配行单行预览**：行号 + 匹配行、超长截断；**不含上下文**——需要上下文时用 read_file 读匹配处附近窗口）；统计各文件命中数用 output_mode=count。结果条数默认 100、上限 200，超出即截断并附收窄/续查指引；支持 glob/type 过滤、ignore_case、multiline、offset 分页续查。搜索文件内容统一用本工具（勿用 execute_command 跑 shell 的 grep/rg/findstr）；开放式探索（摸代码布局、重构前侦察）优先用 repo_map 或子代理。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::Search
    }

    async fn execute(
        &self,
        session_id: &str,
        arguments: &str,
    ) -> tianyan::common::error::Result<serde_json::Value> {
        let params: SearchCodeParams = parse_params(arguments)?;
        let resolved_root = match &params.path {
            Some(dir) => self.ctx.resolve_tool_path(session_id, dir).await?,
            None => self
                .ctx
                .resolve_base_dir(session_id)
                .await?
                .to_string_lossy()
                .into_owned(),
        };
        safety_violation(
            self.ctx
                .security_policy
                .check_path(Path::new(&resolved_root)),
        )?;
        let mut options = search_options(&params);
        options.path = Some(resolved_root);
        execute_search_code(&params.pattern, &options)
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

/// 本插件提供的全部工具（装配层一行注册用）。
///
/// 顺序即注册顺序：`symbol_outline` → `repo_map` → `grep`（与迁移前内置表顺序一致，
/// 前缀稳定性）。
pub fn tools(ctx: &ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>> {
    vec![
        Arc::new(SymbolOutlineTool::new(ctx.clone())),
        Arc::new(RepoMapTool::new(ctx.clone())),
        Arc::new(GrepTool::new(ctx.clone())),
    ]
}

/// 插件清单工厂（各 bin / 桌面端共用；清单单一事实源在本 crate）。
pub fn plugin_factory() -> impl FnOnce(&ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>> {
    tools
}
