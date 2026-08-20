//! 内部状态视图处理器。
//!
//! 记忆浏览直接读取 VFS memory 命名空间（不经 SummaryEngine——内容已由
//! MemoryExtractor 写入，此处仅提供读路径）；统计摘要读取 UsageStats。

use std::sync::Arc;

use axum::{extract::Query, extract::State, Json};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use tianyan::vfs::{ContentStore, VfsCore};

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 列出记忆命名空间全部条目（含 L0/L1/L2 内容）。
///
/// 记忆由 MemoryTask（每 10 分钟 cron）经 MemoryExtractor 写入
/// `tianyan://memory/*`；此前无任何读路径，本端点补齐浏览能力。
pub async fn list_memories_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let vfs = state.vfs();
    let memory_root = TianyanUri::new(ContextNamespace::Memory, vec![]);

    let entries = vfs.list(&memory_root).await?;

    let mut memories = Vec::with_capacity(entries.len());
    for entry in entries {
        let uri = &entry.metadata.uri;
        // 内容读取失败不阻塞整体浏览：缺失层级返回 null
        let abstract_content = vfs.read(uri, ContentLevel::Abstract).await.ok();
        let overview_content = vfs.read(uri, ContentLevel::Overview).await.ok();
        let detail_content = vfs.read(uri, ContentLevel::Detail).await.ok();
        memories.push(json!({
            "uri": uri.to_string(),
            "is_directory": entry.is_directory(),
            "name": uri.path().last().map(|s| s.to_string()),
            "metadata": entry.metadata,
            "abstract": abstract_content,
            "overview": overview_content,
            "detail": detail_content,
        }));
    }

    info!(total = memories.len(), "记忆列表已返回");
    Ok(Json(
        json!({ "memories": memories, "total": memories.len() }),
    ))
}

/// 返回使用统计摘要（技能调用、文档访问、搜索热度）。
pub async fn get_stats_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stats = state.usage_stats();
    Ok(Json(stats.query_summary().await))
}

/// 检索轨迹列表查询参数。
#[derive(Debug, Deserialize)]
pub struct TracesQuery {
    /// 返回条数（默认 20，上限 100）。
    #[serde(default = "default_trace_limit")]
    pub limit: usize,
}

fn default_trace_limit() -> usize {
    20
}

/// 返回最近的检索轨迹（单次检索的完整过程快照，供调试）。
///
/// 每条轨迹包含：查询、意图分析、每步搜索（URI/分数/Token）、
/// 内容加载层级、总耗时——用于回溯"为什么这次检索成这样"。
pub async fn list_traces_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TracesQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.min(100);
    let traces = state.usage_stats().query_recent_traces(limit).await;
    Ok(Json(json!({ "traces": traces, "total": traces.len() })))
}

/// LLM 用量统计查询参数。
#[derive(Debug, Deserialize)]
pub struct UsageStatsQuery {
    /// 统计时段（天）：7 / 30 / 0=全部；默认 7。
    #[serde(default = "default_usage_days")]
    pub days: i64,
    /// 按 provider 过滤（可选）。
    #[serde(default)]
    pub provider: Option<String>,
    /// 按 model 过滤（可选）。
    #[serde(default)]
    pub model: Option<String>,
    /// 分组维度：provider | model（可选；缺省按 model）。
    #[serde(default)]
    pub group_by: Option<String>,
}

fn default_usage_days() -> i64 {
    7
}

/// 返回 LLM 用量统计（总计 + 按 provider/model 分组）。
///
/// 覆盖聊天、子代理委托、演化任务等所有走 AgentLoop 的调用。
pub async fn get_usage_stats_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let since_ts = if query.days > 0 {
        Some(chrono::Utc::now().timestamp() - query.days * 86400)
    } else {
        None
    };
    let group_by = query.group_by.as_deref().unwrap_or("model");
    let log = state.usage_log();
    let total = log.stats(since_ts, query.provider.as_deref(), query.model.as_deref(), "none").await;
    let grouped = log.stats(since_ts, query.provider.as_deref(), query.model.as_deref(), group_by).await;
    Ok(Json(json!({
        "days": query.days,
        "total": total.first().map(|s| serde_json::to_value(s).unwrap_or_default()),
        "grouped": grouped,
    })))
}

/// 返回定时任务调度器状态。
///
/// 无启用的模型 Provider 时调度器未装配，返回空状态（running=false、tasks=[]）。
pub async fn get_scheduler_status_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.scheduler().await {
        Some(scheduler) => {
            let tasks = scheduler.snapshot().await;
            Ok(Json(json!({
                "running": scheduler.is_running(),
                "tasks": tasks,
            })))
        }
        None => Ok(Json(json!({ "running": false, "tasks": [] }))),
    }
}

/// 返回审批状态（工作流配置 + 待处理请求 + 最近审计记录）。
///
/// 审批链路：工具执行门控 → 风险分级 → 自动审批/拒绝 → "询问用户"降级确认；
/// 本端点暴露该链路的当前状态与历史记录。
pub async fn get_approval_status_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent = state.agent().await;
    let snapshot = agent.approval_status().await?;
    // serde 序列化失败为内部错误（已有 From<serde_json::Error> 映射面向反序列化，此处显式 Internal）
    serde_json::to_value(snapshot)
        .map(Json)
        .map_err(|e| ApiError::Internal(format!("审批状态序列化失败：{e}")))
}

/// 审批响应请求体（GUI 审批面板调用）。
#[derive(Debug, serde::Deserialize)]
pub struct RespondApprovalRequest {
    /// 待处理审批请求 ID（来自 approval/status 的 pending_approvals[].request_id）。
    pub request_id: String,
    /// 决策：approve / deny / request_more_info。
    pub decision: String,
    /// 理由（可选，记入审计记录）。
    pub reason: Option<String>,
    /// 用户编辑后的命令（可选；纠正/改写场景，仅 decision=approve 时生效，
    /// 记入审计记录与通知文本，不影响实际执行——执行仍由 agent 按原命令发起）。
    #[serde(default)]
    pub edited_command: Option<String>,
}

impl RespondApprovalRequest {
    /// 解析决策字符串为内部枚举（snake_case API 语义）。
    fn parse_decision(&self) -> Result<tianyan::executor::approval::ApprovalDecision, ApiError> {
        match self.decision.as_str() {
            "approve" => Ok(tianyan::executor::approval::ApprovalDecision::Approve),
            "deny" => Ok(tianyan::executor::approval::ApprovalDecision::Deny),
            "request_more_info" => {
                Ok(tianyan::executor::approval::ApprovalDecision::RequestMoreInfo)
            }
            other => Err(ApiError::BadRequest(format!(
                "无效的审批决策：{other}（可选：approve / deny / request_more_info）"
            ))),
        }
    }
}

/// 响应待处理审批请求。
///
/// 请求不存在或已超时（默认 300s，超时自动拒绝）→ 404；
/// 向导模式（审批工作流未装配）→ 500。
pub async fn respond_approval_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RespondApprovalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let decision = request.parse_decision()?;
    let agent = state.agent().await;
    agent
        .respond_approval(
            &request.request_id,
            decision,
            request.reason,
            request.edited_command,
        )
        .await
        // ? 传播：From<TianyanError> 语义谓词映射（not_found → 404 等，ADR-014），
        // 不在此处重实现分类
        ?;
    Ok(Json(json!({ "ok": true })))
}
