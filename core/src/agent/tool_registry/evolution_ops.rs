//! 演化数据查询工具执行器（ADR-017 GEPA 数据层）。
//!
//! execution_stats / execution_detail / delegation_stats —— 供演化智能体
//! 查询执行统计数据（只读，不修改任何内容）。统计查询经共享 SqliteDb
//! （派生数据，可重建）。

use std::sync::Arc;

use crate::agent::tool_params::{
    DelegationStatsParams, ExecutionDetailParams, ExecutionStatsParams, SessionRecallParams,
};
use crate::common::error::TianyanError;
use crate::common::llm_judge::truncate_output;
use crate::observability::execution_log::ExecutionLog;

use super::{parse_params, wrap_tool_error, ToolRegistry};

/// 回忆窗口单条消息文本上限（控制工具输出体积——超长对话单条可达数十 KB）。
const RECALL_WINDOW_TEXT_CHARS: usize = 800;
/// 回忆窗口单条工具文本上限。
const RECALL_WINDOW_TOOL_CHARS: usize = 400;

/// 解析 RFC3339 since 参数为 epoch 秒。
///
/// # Errors
/// * 格式非法时返回 TianyanError::invalid_input（ADR-014 语义分类）。
fn parse_since(since: Option<&str>) -> Result<Option<i64>, TianyanError> {
    match since {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.timestamp()))
            .map_err(|e| {
                TianyanError::invalid_input(format!(
                    "tool: 参数无效：since 需为 RFC3339 时间（如 2026-08-18T00:00:00Z）：{e}"
                ))
            }),
    }
}

impl ToolRegistry {
    /// 获取执行记录日志（GEPA 数据层；未装配时工具不可用）。
    fn execution_log(&self) -> Result<Arc<ExecutionLog>, TianyanError> {
        self.execution_log.clone().ok_or_else(|| {
            TianyanError::Custom(
                "tool: 执行失败：执行记录日志未装配（GEPA 数据层不可用）".to_string(),
            )
        })
    }

    /// 执行 execution_stats 工具：按类别统计工具执行情况。
    pub(crate) async fn execute_execution_stats(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ExecutionStatsParams = parse_params(arguments)?;
        let log = self.execution_log()?;
        let since = parse_since(params.since.as_deref())?;
        let stats = log
            .stats(since, params.category.as_deref())
            .await
            .map_err(wrap_tool_error)?;
        Ok(serde_json::json!({
            "count": stats.len(),
            "stats": stats,
        }))
    }

    /// 执行 execution_detail 工具：查询原始执行记录。
    pub(crate) async fn execute_execution_detail(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ExecutionDetailParams = parse_params(arguments)?;
        let log = self.execution_log()?;
        let since = parse_since(params.since.as_deref())?;
        let limit = params.limit.unwrap_or(20).min(50);
        let items = log
            .detail(since, params.category.as_deref(), limit)
            .await
            .map_err(wrap_tool_error)?;
        Ok(serde_json::json!({
            "count": items.len(),
            "executions": items,
        }))
    }

    /// 执行 delegation_stats 工具：按角色统计委托使用情况。
    pub(crate) async fn execute_delegation_stats(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: DelegationStatsParams = parse_params(arguments)?;
        let log = self.execution_log()?;
        let since = parse_since(params.since.as_deref())?;
        let stats = log.delegation_stats(since).await.map_err(wrap_tool_error)?;
        Ok(serde_json::json!({
            "count": stats.len(),
            "stats": stats,
        }))
    }

    /// 执行 session_recall 工具：FTS5 关键词回忆（ADR-017 决策 6）。
    ///
    /// 返回命中列表，每个命中附带附近窗口（user/assistant 文本；
    /// 工具调用/结果被过滤——只进 tool_text 列，不进 FTS 索引）。
    pub(crate) async fn execute_session_recall(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SessionRecallParams = parse_params(arguments)?;
        let recall = self.session_recall.clone().ok_or_else(|| {
            TianyanError::Custom(
                "tool: 执行失败：会话回忆服务未装配（FTS5 索引不可用）".to_string(),
            )
        })?;
        let limit = params.limit.unwrap_or(5).min(20);
        let radius = params
            .radius
            .unwrap_or(crate::session::types::DEFAULT_WINDOW_RADIUS)
            .max(0);
        let hits = recall
            .search(&params.query, limit, params.since_days)
            .await
            .map_err(wrap_tool_error)?;
        // 每个命中附带附近窗口；窗口消息截断（text/tool_text 上限）
        let mut results = Vec::new();
        for hit in &hits {
            let mut window = recall
                .window(&hit.session_id, hit.seq, radius)
                .await
                .map_err(wrap_tool_error)?;
            for msg in &mut window {
                msg.text = truncate_output(&msg.text, RECALL_WINDOW_TEXT_CHARS);
                msg.tool_text = truncate_output(&msg.tool_text, RECALL_WINDOW_TOOL_CHARS);
            }
            results.push(serde_json::json!({
                "hit": hit,
                "window": window,
            }));
        }
        Ok(serde_json::json!({
            "query": params.query,
            "count": results.len(),
            "results": results,
        }))
    }
}
