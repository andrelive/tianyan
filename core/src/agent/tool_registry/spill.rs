//! 工具输出 spill 后置监听器（ADR-047）。
//!
//! **问题**：工具结果一旦返回就永久留在会话历史里——从那一轮起每轮请求都会
//! 重复携带（直到被压缩）。一次 50KB 的 grep / repo_map / 子代理结果不只是
//! "一次输入"，而是持续推高后续每一轮的输入，并**提前触发有损且不可逆的压缩**。
//!
//! **做法**：在**工具输出的统一出口**做预算检查——超预算（行数或字节任一超限）
//! 的结果完整落盘，会话里只保留"预览 + 完整内容路径"，需要细节时由模型用
//! `read_file` 的 offset/limit 分页读取，或委托子代理处理。常态（绝大多数调用
//! 远低于预算）零改写、零成本。
//!
//! **与参照实现同构**（2026-09-30 核实）：opencode 的 `Truncate.output` 由
//! `tool/tool.ts` 在所有工具出口统一调用（超 2000 行 / 50KB → 落盘
//! `{data}/tool-output/` + 路径指引）；DSH 的 `spill-policy` 挂
//! `tools/post-execute` 全局钩子（`read` 除外）。天演落在既有管线上——
//! `pipeline.rs` 的 post-execute 文档早已写明其用途"如给大输出挂 spill locator"。
//!
//! **位置纪律**：注册为**第二个** post-execute 监听器（`ToolObservabilityListener`
//! 之后）。观测监听器要把**原始结果**写进 Trace / GEPA 数据层
//! （`ExecutionHistory.result = format!("{v}")`），spill 是改写者，必须排最后，
//! 否则数据层看到的是改写后的结果，统计与演化数据失真。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::pipeline::ToolPostExecuteListener;
use crate::common::error::TianyanError;
use crate::config::ToolOutputConfig;
use crate::executor::truncate::truncate_spill_with;
use crate::model::types::ToolCall;

/// 不落盘的工具（结果可随时重取，或已有落盘机制）。
///
/// - `read_file` / `vfs_read`：模型**主动选择**的精确内容（DSH 同款排除），
///   且文件 / VFS 本身是事实源，随时可重读；
/// - `list_dir` / `vfs_list` / `glob`：目录列举，可随时重取且通常很小；
/// - `execute_command`：已有 `log_file` 落盘机制（`executor/command.rs`），
///   避免双重落盘。
const SKIP_TOOLS: &[&str] = &[
    "read_file",
    "vfs_read",
    "vfs_list",
    "list_dir",
    "glob",
    "execute_command",
];

/// spill 监听器状态（std Mutex 短临界区：仅拷贝配置，跨 await 不持锁）。
#[derive(Debug, Clone)]
struct SpillState {
    enabled: bool,
    /// 落盘目录；`None` = 不落盘（未注入配置，与 `command_logs_dir` 同模式）。
    dir: Option<PathBuf>,
    max_lines: usize,
    max_bytes: usize,
}

/// 内置工具输出 spill 监听器（ADR-047）。
#[derive(Clone, Debug)]
pub(crate) struct ToolOutputSpillListener {
    inner: Arc<Mutex<SpillState>>,
}

impl Default for ToolOutputSpillListener {
    fn default() -> Self {
        let cfg = ToolOutputConfig::default();
        Self {
            inner: Arc::new(Mutex::new(SpillState {
                enabled: cfg.enabled,
                dir: None,
                max_lines: cfg.max_lines,
                max_bytes: cfg.max_bytes,
            })),
        }
    }
}

impl ToolOutputSpillListener {
    /// 配置落盘目录与预算（由 `ToolRegistry::with_tool_output` 委托）。
    ///
    /// 阈值下限钳到 1，避免 0 阈值把一切结果都送去落盘。
    pub(crate) fn configure(&self, dir: PathBuf, config: &ToolOutputConfig) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.enabled = config.enabled;
        state.dir = Some(dir);
        state.max_lines = config.max_lines.max(1);
        state.max_bytes = config.max_bytes.max(1);
    }

    /// 当前配置快照 `(enabled, dir, max_lines, max_bytes)`（诊断 / 测试用）。
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> (bool, Option<PathBuf>, usize, usize) {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (
            state.enabled,
            state.dir.clone(),
            state.max_lines,
            state.max_bytes,
        )
    }
}

#[async_trait]
impl ToolPostExecuteListener for ToolOutputSpillListener {
    async fn on_post_execute(
        &self,
        call: &ToolCall,
        _session_id: &str,
        result: &mut Result<Value, TianyanError>,
        _elapsed: std::time::Duration,
    ) {
        let (enabled, dir, max_lines, max_bytes) = {
            let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.enabled,
                state.dir.clone(),
                state.max_lines,
                state.max_bytes,
            )
        };
        // 旁路：未启用，或未注入目录（未配置 = 不改变任何行为）。
        if !enabled {
            return;
        }
        let Some(dir) = dir else { return };
        // 白名单：可重取 / 已有落盘机制的工具不改写。
        if SKIP_TOOLS.contains(&call.function.name.as_str()) {
            return;
        }
        // 失败结果不改写（错误信息通常很小，且改写会掩盖真实错误）。
        let Ok(value) = result.as_ref() else { return };

        // 预算检查：与 `truncate_spill_with` 同口径（序列化后行数 / 字节数）。
        // 常态（绝大多数调用）在此直接返回——零改写、零 IO。
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        let lines = text.lines().count();
        let bytes = text.len();
        if lines <= max_lines && bytes <= max_bytes {
            return;
        }

        match truncate_spill_with(&text, &dir, &call.function.name, max_lines, max_bytes).await {
            Ok(truncated) => {
                // 竞态兜底：判定超限但落盘未截断（配置并发变更）→ 不改写。
                let Some(spill_path) = truncated.spill_path else {
                    return;
                };
                *result = Ok(json!({
                    "spilled": true,
                    "spill_path": spill_path,
                    "spill_bytes": truncated.total_bytes,
                    "spill_lines": truncated.total_lines,
                    "note": format!(
                        "工具输出过大已落盘（完整内容 {} 行 / {} 字节）。用 read_file 的 offset/limit 分页读取该文件，或委托子代理（delegate_to_agent）处理——勿凭预览下结论。",
                        truncated.total_lines, truncated.total_bytes
                    ),
                    "preview": truncated.text,
                }));
                tracing::info!(
                    tool = %call.function.name,
                    path = %spill_path,
                    bytes = truncated.total_bytes,
                    "工具输出超预算：完整内容已落盘（会话仅保留预览 + 路径）"
                );
            }
            Err(e) => {
                // best effort：落盘失败保持原结果——不阻塞工具调用，不新增错误变体
                // （对齐 `executor/command.rs` 既有"落盘失败不影响结果"语义）。
                tracing::warn!(
                    tool = %call.function.name,
                    error = %e,
                    "工具输出落盘失败（保持原结果）"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "spill_tests.rs"]
mod tests;
