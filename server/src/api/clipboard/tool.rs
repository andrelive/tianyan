//! `clipboard_write` 动态工具：把内容写入 outbox，由 tauri 轮询写到系统剪贴板。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::DynamicToolExecutor;
use tianyan::common::error::Result;
use tianyan::model::types::{FunctionDefinition, ToolDefinition};
use tianyan::TianyanError;

/// `clipboard_write` 工具参数。
#[derive(Debug, Deserialize)]
pub struct ClipboardWriteArgs {
    /// 要写入系统剪贴板的文本内容。
    pub content: String,
}

/// 剪贴板写入动态工具（ADR-003 组件工具化：server 层适配 core 动态工具 trait）。
///
/// 行为：把内容推入 outbox（`Arc<Mutex<Vec<String>>>`），tauri 侧轮询
/// `GET /api/v1/clipboard/outbox` 取走并写入系统剪贴板。工具本身不直接
/// 触碰系统剪贴板——写剪贴板发生在桌面进程，避免跨进程/平台差异。
///
/// 注册方式（集成者）：outbox 取 `AppState::clipboard_outbox()` 注入——
/// `tool_registry.register_dynamic_tool(Arc::new(ClipboardWriteTool::new(state.clipboard_outbox()))).await`
pub struct ClipboardWriteTool {
    outbox: Arc<Mutex<Vec<String>>>,
}

impl ClipboardWriteTool {
    /// 绑定到指定 outbox（与 `GET /api/v1/clipboard/outbox` 同一实例）。
    pub fn new(outbox: Arc<Mutex<Vec<String>>>) -> Self {
        Self { outbox }
    }
}

#[async_trait]
impl DynamicToolExecutor for ClipboardWriteTool {
    fn tool_name(&self) -> String {
        "clipboard_write".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(FunctionDefinition::new(
            "clipboard_write",
            "将文本写入系统剪贴板，供用户在其他应用中粘贴使用（如代码片段、格式化答案）。返回排队结果。",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "要写入系统剪贴板的文本内容"
                    }
                },
                "required": ["content"]
            }),
        ))
    }

    async fn execute(&self, arguments: &str) -> Result<serde_json::Value> {
        let args: ClipboardWriteArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: clipboard_write 参数无效：{e}")))?;
        let content_length = args.content.len();
        self.outbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(args.content);
        Ok(serde_json::json!({
            "status": "queued",
            "content_length": content_length,
        }))
    }
}
