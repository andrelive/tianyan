//! 剪贴板服务：配置门控的捕获处理、记忆/知识沉淀、pending 与 outbox 状态。

use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;

use tianyan::common::types::{ContentSource, ContextNamespace, TianyanUri};
use tianyan::knowledge::IngestionRequest;
use tianyan::vfs::VirtualFileSystem;

use crate::api::clipboard::types::{
    CaptureResponse, PendingCapture, RespondAction, RespondRequest,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 记忆命名空间下的剪贴板子目录。
const MEMORY_CLIPBOARD_DIR: &str = "clipboard";
/// 记忆条目 Abstract（L0）截断长度（与 knowledge 摘要回退截断同量级）。
const MEMORY_ABSTRACT_MAX_CHARS: usize = 300;
/// 知识库导入使用的文件名（内容为剪贴板文本，无真实磁盘文件）。
const KNOWLEDGE_CLIPBOARD_FILENAME: &str = "clipboard.txt";

/// 剪贴板服务。
///
/// 职责边界：配置门控（`clipboard.enabled` / `auto_capture`）、
/// 沉淀落盘（记忆/知识库）、pending 与 outbox 的存取。
/// pending 与 outbox 句柄由调用方（`AppState`）注入——跨请求保持，
/// 配置热更新不重建。
pub struct ClipboardService {
    vfs: Arc<dyn VirtualFileSystem>,
    /// outbox：agent `clipboard_write` 工具写入 → tauri 轮询消费。
    outbox: Arc<Mutex<Vec<String>>>,
    /// pending：capture 存 → 前端确认（respond）消费。
    pending: Arc<RwLock<Option<PendingCapture>>>,
}

impl ClipboardService {
    /// 创建服务（持有 VFS 用于沉淀写入 + AppState 注入的共享状态句柄）。
    pub fn new(
        vfs: Arc<dyn VirtualFileSystem>,
        outbox: Arc<Mutex<Vec<String>>>,
        pending: Arc<RwLock<Option<PendingCapture>>>,
    ) -> Self {
        Self {
            vfs,
            outbox,
            pending,
        }
    }

    /// 处理捕获：配置门控 → auto_capture 直接沉淀，否则存 pending。
    ///
    /// 返回 `None` 表示剪贴板监听未启用（调用方映射为 204，不做任何存储）；
    /// `Some` 为处理结果（`stored` 或 `pending`）。
    pub async fn capture(
        &self,
        state: &AppState,
        text: &str,
    ) -> Result<Option<CaptureResponse>, ApiError> {
        if text.trim().is_empty() {
            return Err(ApiError::BadRequest("剪贴板文本不能为空".to_string()));
        }

        let config_handle = state.config();
        let config = config_handle.read().await;
        let (enabled, auto_capture) = (config.clipboard.enabled, config.clipboard.auto_capture);

        if !enabled {
            // 隐私敏感功能：未启用时行为零变化（不沉淀、不存 pending）
            return Ok(None);
        }

        if auto_capture {
            // 自动沉淀：跳过确认条，直接写入记忆
            let uri = store_memory(&self.vfs, text).await?;
            return Ok(Some(CaptureResponse {
                status: "stored".to_string(),
                pending: None,
                uri: Some(uri.to_string()),
            }));
        }

        // 需要确认：存 pending，由前端确认条决定去向
        let pending = PendingCapture {
            id: uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("capture")
                .to_string(),
            text: text.to_string(),
            captured_at: chrono::Utc::now().to_rfc3339(),
        };
        *self.pending.write().await = Some(pending.clone());
        Ok(Some(CaptureResponse {
            status: "pending".to_string(),
            pending: Some(pending),
            uri: None,
        }))
    }

    /// 处理确认：remember → 记忆；knowledge → 知识库；ignore → 清 pending。
    ///
    /// 文本优先级：请求体 `text` 覆盖 > pending 原文；两者皆无返回 400。
    /// 响应含最终状态与沉淀目标 URI。
    pub async fn respond(
        &self,
        state: &AppState,
        request: RespondRequest,
    ) -> Result<serde_json::Value, ApiError> {
        let text = match request.text {
            Some(t) if !t.trim().is_empty() => t,
            _ => match self.pending.read().await.as_ref() {
                Some(p) => p.text.clone(),
                None => {
                    return Err(ApiError::BadRequest(
                        "没有待确认的剪贴板内容，且未提供 text".to_string(),
                    ))
                }
            },
        };

        let (status, uri) = match request.action {
            RespondAction::Remember => {
                let uri = store_memory(&self.vfs, &text).await?;
                ("stored", Some(uri.to_string()))
            }
            RespondAction::Knowledge => {
                let ingestor = state.create_knowledge_ingestor().await?;
                let result = ingestor
                    .ingest(
                        IngestionRequest::new(
                            text.as_bytes().to_vec(),
                            KNOWLEDGE_CLIPBOARD_FILENAME,
                        )
                        .with_source(ContentSource::ExternalImport)
                        .with_tags(vec!["clipboard".to_string()]),
                    )
                    .await?;
                ("ingested", Some(result.uri.to_string()))
            }
            RespondAction::Ignore => ("ignored", None),
        };

        // 无论何种动作，pending 一次性消费
        *self.pending.write().await = None;
        Ok(serde_json::json!({ "status": status, "uri": uri }))
    }

    /// 读取当前待确认捕获（无则 `None`）。
    pub async fn get_pending(&self) -> Option<PendingCapture> {
        self.pending.read().await.clone()
    }

    /// 把文本推入 outbox（agent `clipboard_write` 工具调用）。
    pub fn outbox_push(&self, content: String) {
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(content);
    }

    /// 取出 outbox 全部内容（tauri 轮询消费；取后清空，防重复写出）。
    pub fn outbox_drain(&self) -> Vec<String> {
        let mut guard = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *guard)
    }
}

/// 把剪贴板文本沉淀为记忆条目（`tianyan://memory/clipboard/<ts>`）。
///
/// 与 `MemoryTask::store_memory` 同一写入约定：Detail 存全文、
/// Abstract 存检索摘要（截断），父目录由 `ContentStore::write` 自动创建。
/// 简洁优先：不引入 LLM 摘要依赖，保证离线可用。
async fn store_memory(
    vfs: &Arc<dyn VirtualFileSystem>,
    text: &str,
) -> Result<TianyanUri, ApiError> {
    let uri = TianyanUri::new(
        ContextNamespace::Memory,
        vec![
            MEMORY_CLIPBOARD_DIR.to_string(),
            chrono::Utc::now().timestamp_millis().to_string(),
        ],
    );

    vfs.write_content(&uri, text).await?;

    let abstract_content: String = text.chars().take(MEMORY_ABSTRACT_MAX_CHARS).collect();
    vfs.write_abstract(&uri, &abstract_content).await?;

    tracing::info!(uri = %uri, bytes = text.len(), "剪贴板内容已沉淀到记忆");
    Ok(uri)
}
