//! LSP 推送诊断存储与客户端池（LspManager）。
//!
//! `LspManager` 是 LSP 能力的统一入口：
//! - `servers`：按项目根缓存已初始化的 [`LspClient`]（服务池）；
//! - `store`：`textDocument/publishDiagnostics` 推送的诊断，按绝对路径索引；
//! - [`LspManager::ensure_server`] 按扩展名查注册表 → 探测项目根 → 启动并初始化服务器 → 打开文档；
//! - [`LspManager::query`] 将工具操作映射到客户端方法。
//!
//! 诊断复用 [`crate::executor::verification::StructuredDiagnostic`]（单一来源），
//! LSP 0 起始行列 → 结构化诊断 1 起始行列。

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dashmap::DashMap;
use lsp_types::{
    Diagnostic, DiagnosticSeverity, NumberOrString, Position, PublishDiagnosticsParams,
};
use serde_json::Value;

use crate::common::error::{Result, TianyanError};
use crate::executor::verification::StructuredDiagnostic;

use super::client::{LspClient, PublishDiagnosticsCallback};
use super::registry::{probe_project_root, spec_for_extension};

/// LSP 管理器：服务池 + 推送诊断存储。
pub struct LspManager {
    /// 推送诊断存储：绝对路径 → 结构化诊断列表。
    pub store: Arc<DashMap<String, Vec<StructuredDiagnostic>>>,
    /// 客户端池：项目根 → 已初始化客户端。
    pub servers: DashMap<String, Arc<LspClient>>,
}

impl LspManager {
    /// 创建空管理器（服务器按需惰性启动）。
    pub fn new() -> Self {
        Self {
            store: Arc::new(DashMap::new()),
            servers: DashMap::new(),
        }
    }

    /// 处理服务器推送的 `textDocument/publishDiagnostics` 通知。
    ///
    /// LSP 行列是 0 起始，写入时转为 1 起始（与 [`StructuredDiagnostic`] 一致）。
    pub fn handle_publish(&self, params: PublishDiagnosticsParams) {
        let path = uri_to_path(params.uri.as_str());
        self.store
            .insert(path, convert_diagnostics(params.diagnostics));
    }

    /// 读取指定文件的诊断（按行、列排序；无记录返回空列表）。
    pub fn diagnostics_for(&self, path: &str) -> Vec<StructuredDiagnostic> {
        let mut diagnostics = self
            .store
            .get(path)
            .map(|entry| entry.clone())
            .unwrap_or_default();
        diagnostics.sort_by_key(|d| (d.line, d.column));
        diagnostics
    }

    /// 确保目标文件所属项目有可用客户端：注册表查规格 → 探测项目根 →
    /// 启动/初始化/打开文档 → 缓存进池。已缓存时直接复用。
    pub async fn ensure_server(&self, file_path: &str) -> Result<Arc<LspClient>> {
        let path = Path::new(file_path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let spec = spec_for_extension(ext).ok_or_else(|| {
            TianyanError::Custom(format!("executor: lsp: 不支持的文件类型：{ext}"))
        })?;
        let root = probe_project_root(path, spec)
            .unwrap_or_else(|| path.parent().unwrap_or(path).to_path_buf());
        let key = root.to_string_lossy().to_string();
        if let Some(client) = self.servers.get(&key) {
            return Ok(client.clone());
        }
        // 诊断回调直接写入共享 store（Arc 捕获，避免自引用）
        let store = self.store.clone();
        let on_publish: PublishDiagnosticsCallback = Arc::new(move |params| {
            store.insert(
                uri_to_path(params.uri.as_str()),
                convert_diagnostics(params.diagnostics),
            );
        });
        let client = Arc::new(LspClient::spawn_process(spec, &root, Some(on_publish)).await?);
        client.initialize(file_uri(&root)?).await?;
        client.initialized().await?;
        // 双检（double-checked）：并发 ensure_server（不同文件、同一项目根）
        // 可能同时错过上方缓存并各自 spawn+initialize；先完成者入池，后完成者
        // 复用已入池客户端，并丢弃自己的客户端（LspClient drop 时终止其子进程）。
        // 残余竞态：两个调用都走到 insert 时后写者覆盖先写者——两个客户端等价
        // （同规格、同项目根），良性；被覆盖客户端打开过的文档未在幸存客户端上
        // did_open，其后续查询结果可能为空，由 LSP 调用方的尽力而为语义兜底。
        if let Some(existing) = self.servers.get(&key) {
            return Ok(existing.clone());
        }
        // 打开文档：服务器只对已打开文档返回诊断/符号
        let text = tokio::fs::read_to_string(file_path)
            .await
            .unwrap_or_default();
        client
            .did_open(file_uri(path)?, spec.language_id, 1, &text)
            .await?;
        self.servers.insert(key, client.clone());
        Ok(client)
    }

    /// 执行一次 LSP 查询（按操作分发到客户端方法）。
    ///
    /// 未知操作返回 `tool: 参数无效`（与工具层错误前缀一致）。
    pub async fn query(
        &self,
        file_path: &str,
        operation: &str,
        line: Option<usize>,
        character: Option<usize>,
        workspace_query: Option<&str>,
    ) -> Result<Value> {
        let client = self.ensure_server(file_path).await?;
        let uri = file_uri(Path::new(file_path))?;
        let pos = position(line.unwrap_or(0), character.unwrap_or(0))?;
        match operation {
            "hover" => client.hover(&uri, pos.line, pos.character).await,
            "goToDefinition" => client.goto_definition(&uri, pos.line, pos.character).await,
            "findReferences" => client.references(&uri, pos.line, pos.character, true).await,
            "documentSymbol" => client.document_symbols(&uri).await,
            "workspaceSymbol" => {
                client
                    .workspace_symbols(workspace_query.unwrap_or_default())
                    .await
            }
            "goToImplementation" => {
                client
                    .goto_implementation(&uri, pos.line, pos.character)
                    .await
            }
            other => Err(TianyanError::Custom(format!(
                "tool: 参数无效：未知操作：{other}"
            ))),
        }
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 将 usize 行列转为 LSP `Position`（u32，0 起始）。
fn position(line: usize, character: usize) -> std::result::Result<Position, TianyanError> {
    Ok(Position {
        line: u32::try_from(line)
            .map_err(|_| TianyanError::Custom("executor: lsp: 行号超出范围".to_string()))?,
        character: u32::try_from(character)
            .map_err(|_| TianyanError::Custom("executor: lsp: 列号超出范围".to_string()))?,
    })
}

/// 将 LSP 诊断（0 起始行列）转为结构化诊断（1 起始行列）。
fn convert_diagnostics(diagnostics: Vec<Diagnostic>) -> Vec<StructuredDiagnostic> {
    diagnostics
        .into_iter()
        .map(|d| StructuredDiagnostic {
            file: String::new(), // 由调用方按路径填充
            line: usize::try_from(d.range.start.line)
                .unwrap_or(0)
                .saturating_add(1),
            column: usize::try_from(d.range.start.character)
                .unwrap_or(0)
                .saturating_add(1),
            level: match d.severity {
                Some(DiagnosticSeverity::ERROR) => "error".to_string(),
                Some(DiagnosticSeverity::WARNING) => "warning".to_string(),
                Some(DiagnosticSeverity::INFORMATION) => "info".to_string(),
                Some(DiagnosticSeverity::HINT) => "hint".to_string(),
                _ => "note".to_string(),
            },
            code: d.code.map(|c| match c {
                NumberOrString::Number(n) => n.to_string(),
                NumberOrString::String(s) => s,
            }),
            message: d.message,
            suggestion: None,
        })
        .collect()
}

/// file:// URI → 本地绝对路径（解析失败时原样返回）。
fn uri_to_path(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| uri.to_string())
}

/// 本地绝对路径 → LSP file:// URI。
fn file_uri(path: &Path) -> Result<lsp_types::Uri> {
    let url = url::Url::from_file_path(path).map_err(|_| {
        TianyanError::Custom(format!(
            "executor: lsp: 路径无法转换为 file URI：{}",
            path.display()
        ))
    })?;
    lsp_types::Uri::from_str(url.as_str())
        .map_err(|e| TianyanError::Custom(format!("executor: lsp: URI 解析失败：{e}")))
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
