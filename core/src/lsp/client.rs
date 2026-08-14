//! 自实现的 LSP JSON-RPC 2.0 客户端（tokio 进程管道 / 内存流）。
//!
//! 传输层为 `Content-Length: N\r\n\r\n<json>` 分帧（LSP 标准）。
//! 设计要点：
//! - 所有请求带 10s 超时；服务器不可用时返回
//!   "executor: lsp: 服务器 {name} 不可用：{detail}，安装提示：{hint}" 错误，
//!   优雅降级、绝不阻塞其他工具；
//! - 响应按 id 分发到挂起的 oneshot；服务器主动推送的
//!   `textDocument/publishDiagnostics` 通知转发给注册的回调；
//! - 通过 [`LspClient::from_streams`] 支持任意读写流（测试用内存 duplex）。

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use lsp_types::notification::{
    DidOpenTextDocument, Initialized, Notification as LspNotification,
    PublishDiagnostics,
};
use lsp_types::request::{GotoImplementationParams, Request as LspRequest};
use lsp_types::{
    DidOpenTextDocumentParams, DocumentSymbolParams,
    GotoDefinitionParams, HoverParams, InitializeParams, InitializedParams, Position,
    PublishDiagnosticsParams, ReferenceContext, ReferenceParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
    WorkspaceFolder, WorkspaceSymbolParams,
};
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    BufWriter,
};
use tokio::sync::{oneshot, Mutex};

use crate::common::error::{Result, TianyanError};

use super::registry::ServerSpec;

/// 单个 LSP 请求的超时上限（10s）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 单条消息体大小上限（64 MiB，防御异常服务器）。
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// 单个头部行长度上限（1 KiB）。
const MAX_HEADER_LINE: usize = 1024;

/// 诊断推送回调：服务器主动推送 `textDocument/publishDiagnostics` 时触发。
pub type PublishDiagnosticsCallback = Arc<dyn Fn(PublishDiagnosticsParams) + Send + Sync>;

/// 挂起请求的响应通道（携带最终结果）。
type PendingSender = oneshot::Sender<Result<Value>>;

/// LSP 客户端：管理一个语言服务器子进程的读写管道。
pub struct LspClient {
    /// 写侧（请求/通知），互斥保证分帧不交错。
    writer: Mutex<BufWriter<Box<dyn AsyncWrite + Unpin + Send>>>,
    /// 请求 id 分配器。
    next_id: AtomicU64,
    /// 挂起请求表：id → 响应通道。
    pending: Arc<DashMap<u64, PendingSender>>,
    /// 服务器名称（错误消息展示）。
    name: String,
    /// 安装提示（错误消息展示）。
    auto_install_hint: String,
    /// 读循环任务句柄（Drop 时中止）。
    reader_handle: tokio::task::JoinHandle<()>,
    /// 服务器子进程（进程模式持有；Drop 时终止）。
    process: Mutex<Option<tokio::process::Child>>,
    /// 读侧已关闭标记（快速失败，避免等满超时）。
    dead: Arc<AtomicBool>,
}

impl LspClient {
    /// 从任意读写流构建客户端（进程管道或测试用内存流）。
    ///
    /// `on_publish` 注册 `textDocument/publishDiagnostics` 回调；
    /// 读循环在后台任务中运行，流关闭时自动失败所有挂起请求。
    pub fn from_streams(
        read: Box<dyn AsyncRead + Unpin + Send>,
        write: Box<dyn AsyncWrite + Unpin + Send>,
        name: &str,
        auto_install_hint: &str,
        on_publish: Option<PublishDiagnosticsCallback>,
    ) -> Self {
        let pending = Arc::new(DashMap::new());
        let reader_pending = pending.clone();
        let name_owned = name.to_string();
        let hint_owned = auto_install_hint.to_string();
        let reader_name = name_owned.clone();
        let reader_hint = hint_owned.clone();
        let dead = Arc::new(AtomicBool::new(false));
        let reader_dead = dead.clone();
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(read);
            loop {
                match read_frame(&mut reader).await {
                    Ok(Some(payload)) => {
                        if let Ok(message) = serde_json::from_slice::<Value>(&payload) {
                            dispatch_message(message, &reader_pending, &on_publish);
                        } else {
                            tracing::debug!(server = %reader_name, "LSP 消息 JSON 解析失败");
                        }
                    }
                    Ok(None) => break, // EOF：服务器关闭了 stdout
                    Err(e) => {
                        tracing::debug!(server = %reader_name, error = %e, "LSP 读循环错误");
                        break;
                    }
                }
            }
            reader_dead.store(true, Ordering::SeqCst);
            fail_all_pending(&reader_pending, &reader_name, &reader_hint);
        });
        Self {
            writer: Mutex::new(BufWriter::new(write)),
            next_id: AtomicU64::new(1),
            pending,
            name: name_owned,
            auto_install_hint: hint_owned,
            reader_handle,
            process: Mutex::new(None),
            dead,
        }
    }

    /// 按规格启动语言服务器子进程并构建客户端（stdin/stdout 管道）。
    ///
    /// 进程以 `spec.args` 在 `root` 目录下启动；失败返回带安装提示的错误。
    pub async fn spawn_process(
        spec: &ServerSpec,
        root: &Path,
        on_publish: Option<PublishDiagnosticsCallback>,
    ) -> Result<Self> {
        use std::process::Stdio;

        let mut child = tokio::process::Command::new(spec.spawn_command)
            .args(spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                unavailable(
                    spec.spawn_command,
                    &format!("进程启动失败：{e}"),
                    spec.auto_install_hint,
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            unavailable(
                spec.spawn_command,
                "无法获取 stdin 管道",
                spec.auto_install_hint,
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            unavailable(
                spec.spawn_command,
                "无法获取 stdout 管道",
                spec.auto_install_hint,
            )
        })?;
        let mut client = Self::from_streams(
            Box::new(stdout),
            Box::new(stdin),
            spec.spawn_command,
            spec.auto_install_hint,
            on_publish,
        );
        client.process = Mutex::new(Some(child));
        Ok(client)
    }

    /// 发送 JSON-RPC 请求并等待响应（10s 超时）。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(unavailable(
                &self.name,
                "连接已关闭",
                &self.auto_install_hint,
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        if let Err(e) = self.write_payload(&payload).await {
            self.pending.remove(&id);
            return Err(unavailable(
                &self.name,
                &format!("写入失败：{e}"),
                &self.auto_install_hint,
            ));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => {
                self.pending.remove(&id);
                Err(unavailable(
                    &self.name,
                    "响应通道关闭",
                    &self.auto_install_hint,
                ))
            }
            Err(_) => {
                self.pending.remove(&id);
                Err(unavailable(&self.name, "请求超时", &self.auto_install_hint))
            }
        }
    }

    /// 发送 JSON-RPC 通知（无需响应）。
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))?;
        self.write_payload(&payload).await.map_err(|e| {
            unavailable(
                &self.name,
                &format!("写入失败：{e}"),
                &self.auto_install_hint,
            )
        })
    }

    /// 加锁写出一个完整分帧（保证并发请求不交错）。
    async fn write_payload(&self, payload: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().await;
        write_frame(&mut *writer, payload).await
    }

    // ── 生命周期 ─────────────────────────────────────────────────────────

    /// LSP 生命周期第一步：initialize 请求（返回服务器能力原始 result）。
    pub async fn initialize(&self, root_uri: lsp_types::Uri) -> Result<Value> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            capabilities: Default::default(),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri.clone(),
                name: root_uri.to_string(),
            }]),
            ..Default::default()
        };
        self.request(
            <lsp_types::request::Initialize as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// 发送 initialized 通知（initialize 成功后的第二步）。
    pub async fn initialized(&self) -> Result<()> {
        let params = InitializedParams {};
        self.notify(
            <Initialized as LspNotification>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// 打开文档（服务器只对已打开文档提供诊断/符号等能力，LSP 关键前置）。
    pub async fn did_open(
        &self,
        uri: lsp_types::Uri,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<()> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.to_string(),
                version,
                text: text.to_string(),
            },
        };
        self.notify(
            <DidOpenTextDocument as LspNotification>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    // ── 查询方法（返回响应原始 result 字段）─────────────────────────────

    /// `textDocument/hover`：悬停信息。
    pub async fn hover(&self, uri: &lsp_types::Uri, line: u32, character: u32) -> Result<Value> {
        let params = HoverParams {
            text_document_position_params: position_params(uri, line, character),
            work_done_progress_params: Default::default(),
        };
        self.request(
            <lsp_types::request::HoverRequest as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// `textDocument/definition`：跳转到定义。
    pub async fn goto_definition(
        &self,
        uri: &lsp_types::Uri,
        line: u32,
        character: u32,
    ) -> Result<Value> {
        let params = GotoDefinitionParams {
            text_document_position_params: position_params(uri, line, character),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        self.request(
            <lsp_types::request::GotoDefinition as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// `textDocument/references`：查找引用。
    pub async fn references(
        &self,
        uri: &lsp_types::Uri,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Value> {
        let params = ReferenceParams {
            context: ReferenceContext {
                include_declaration,
            },
            text_document_position: position_params(uri, line, character),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        self.request(
            <lsp_types::request::References as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// `textDocument/documentSymbol`：文档符号大纲。
    pub async fn document_symbols(&self, uri: &lsp_types::Uri) -> Result<Value> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        self.request(
            <lsp_types::request::DocumentSymbolRequest as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// `workspace/symbol`：工作区符号搜索。
    pub async fn workspace_symbols(&self, query: &str) -> Result<Value> {
        let params = WorkspaceSymbolParams {
            query: query.to_string(),
            ..Default::default()
        };
        self.request(
            <lsp_types::request::WorkspaceSymbolRequest as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }

    /// `textDocument/implementation`：跳转到实现。
    pub async fn goto_implementation(
        &self,
        uri: &lsp_types::Uri,
        line: u32,
        character: u32,
    ) -> Result<Value> {
        let params = GotoImplementationParams {
            text_document_position_params: position_params(uri, line, character),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        self.request(
            <lsp_types::request::GotoImplementation as LspRequest>::METHOD,
            serde_json::to_value(params)?,
        )
        .await
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.reader_handle.abort();
        // 尽力终止子进程（同步 start_kill，Drop 内不能 await；
        // kill_on_drop 作为第二道保险在 Child 释放时触发）。
        if let Ok(mut guard) = self.process.try_lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.start_kill();
            }
        }
    }
}

/// 读取一个 Content-Length 分帧消息；EOF 返回 `Ok(None)`。
async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        let n = reader.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        if line.len() > MAX_HEADER_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "LSP 头部行过长"));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            break; // 头部结束
        }
        if let Some(rest) = text.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "缺少 Content-Length 头"))?;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "LSP 消息体过大"));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// 写出一个 Content-Length 分帧消息。
async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

/// 分发一条 JSON-RPC 消息：响应 → 挂起请求；通知 → publishDiagnostics 回调。
fn dispatch_message(
    message: Value,
    pending: &DashMap<u64, PendingSender>,
    on_publish: &Option<PublishDiagnosticsCallback>,
) {
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        // 服务器 → 客户端通知
        if message.get("method").and_then(Value::as_str) == Some(PublishDiagnostics::METHOD) {
            if let Some(params) = message.get("params") {
                match serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                    Ok(p) => {
                        if let Some(callback) = on_publish {
                            // 回调 panic 不得杀死读循环（读循环是唯一响应分发者）
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    callback(p);
                                }));
                            if let Err(payload) = result {
                                let detail = payload
                                    .downcast_ref::<&str>()
                                    .copied()
                                    .or_else(|| {
                                        payload.downcast_ref::<String>().map(String::as_str)
                                    })
                                    .unwrap_or("未知 panic");
                                tracing::error!(detail, "LSP publishDiagnostics 回调 panic");
                            }
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "publishDiagnostics 参数解析失败"),
                }
            }
        }
        return;
    };
    let Some((_, tx)) = pending.remove(&id) else {
        return; // 未知 id（如已超时的请求），忽略
    };
    if let Some(error) = message.get("error") {
        let _ = tx.send(Err(TianyanError::Custom(format!(
            "executor: lsp: JSON-RPC 错误：{error}"
        ))));
    } else {
        let result = message.get("result").cloned().unwrap_or(Value::Null);
        let _ = tx.send(Ok(result));
    }
}

/// 流关闭时失败所有挂起请求（避免挂起方等待至超时）。
fn fail_all_pending(pending: &DashMap<u64, PendingSender>, name: &str, hint: &str) {
    let ids: Vec<u64> = pending.iter().map(|entry| *entry.key()).collect();
    for id in ids {
        if let Some((_, tx)) = pending.remove(&id) {
            let _ = tx.send(Err(unavailable(name, "连接已关闭", hint)));
        }
    }
}

/// 构造"服务器不可用"错误（含安装提示，优雅降级）。
fn unavailable(name: &str, detail: &str, hint: &str) -> TianyanError {
    TianyanError::Custom(format!(
        "executor: lsp: 服务器 {name} 不可用：{detail}，安装提示：{hint}"
    ))
}

/// 构造 `TextDocumentPositionParams`。
fn position_params(uri: &lsp_types::Uri, line: u32, character: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position { line, character },
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
