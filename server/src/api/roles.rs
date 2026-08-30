//! 子智能体角色 API（ADR-016：角色列表 / 详情 / 回退 / 退役）。
//!
//! 数据源为 [`RoleStore`]（VFS 权威持久化），与 Agent 注册表同一后端；
//! 操作（reset / delete）写回 VFS，注册表由会话边界刷新 / 配置热重载同步。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;

use tianyan::agent::{RoleRegistry, RoleSource, RoleStatus, RoleStore};

use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 角色列表项（L0 渐进披露粒度）。
#[derive(Debug, Clone, Serialize)]
pub struct RoleSummary {
    /// 角色名。
    pub name: String,
    /// 来源（builtin / user / learned）。
    pub source: String,
    /// 激活状态（active / experimental）。
    pub status: String,
    /// 进化版本。
    pub version: u32,
    /// 进化来源（回退链）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<String>,
    /// 职责一句话（系统提示首行）。
    pub purpose: String,
    /// 工具白名单数量（None = 不限制）。
    pub tool_count: Option<usize>,
    /// 模型（None = 回落主 Agent）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 最大轮数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// 使用统计（调用/成功/失败/成功率；无记录时为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<RoleUsageSummary>,
}

/// 角色使用统计摘要（ADR-016 P3）。
#[derive(Debug, Clone, Serialize)]
pub struct RoleUsageSummary {
    /// 累计调用次数。
    pub calls: u32,
    /// 成功次数。
    pub success: u32,
    /// 失败次数。
    pub failed: u32,
    /// 成功率（0-1）。
    pub success_rate: f32,
    /// 最后使用时间（epoch 毫秒）。
    pub last_used: i64,
}

/// 角色详情（列表字段 + 完整系统提示 + 工具白名单）。
#[derive(Debug, Clone, Serialize)]
pub struct RoleDetail {
    /// 列表粒度摘要。
    #[serde(flatten)]
    pub summary: RoleSummary,
    /// 完整系统提示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 工具白名单（None = 不限制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

/// 角色列表响应。
#[derive(Debug, Serialize)]
pub struct ListRolesResponse {
    /// 角色列表。
    pub roles: Vec<RoleSummary>,
}

/// 按角色统计条目。
#[derive(Debug, Clone, Serialize)]
pub struct RoleUsageEntry {
    /// 角色名。
    pub name: String,
    /// 使用统计。
    #[serde(flatten)]
    pub usage: RoleUsageSummary,
}

/// 角色统计响应（ADR-016 统计面板）。
#[derive(Debug, Serialize)]
pub struct RolesStatsResponse {
    /// 累计委托次数。
    pub total_calls: u32,
    /// 累计成功次数。
    pub total_success: u32,
    /// 总体成功率（0-1；无调用时 0）。
    pub success_rate: f32,
    /// 按角色统计（有调用记录的角色）。
    pub by_role: Vec<RoleUsageEntry>,
    /// 按任务类型统计（分类计数；类别见 [`tianyan::agent::categorize_task_by_keyword`]）。
    pub by_task_type: Vec<(String, usize)>,
    /// 最近委托记录（时间倒序，最多 10 条）。
    pub recent: Vec<tianyan::roles::DelegationRecord>,
}

/// 角色列表（含会话摘要）。
pub async fn list_roles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListRolesResponse>, ApiError> {
    let store = state.role_store();
    let roles = store
        .load_roles()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut summaries = Vec::new();
    for role in roles {
        summaries.push(to_summary(&store, &role).await);
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(ListRolesResponse { roles: summaries }))
}

/// 角色详情。
pub async fn get_role_detail(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<RoleDetail>, ApiError> {
    let store = state.role_store();
    let role = store
        .load_role(&name)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("角色 {name} 不存在")))?;
    Ok(Json(to_detail(&store, &role).await))
}

/// 回退内置种子（恢复出厂定义；VFS 覆盖写，注册表由会话边界刷新同步）。
pub async fn reset_role(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.role_store();
    let seed = RoleRegistry::builtin()
        .get(&name)
        .ok_or_else(|| ApiError::NotFound(format!("内置角色 {name} 不存在（仅内置角色可回退）")))?;
    store
        .save_role(&seed)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    tracing::info!(role = %name, "角色已回退内置种子（注册表待会话边界刷新）");
    Ok(Json(serde_json::json!({ "name": name, "status": "reset" })))
}

/// 退役角色（删除定义 + 会话）。
pub async fn delete_role(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.role_store();
    store
        .delete_role(&name)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    tracing::info!(role = %name, "角色已退役（定义已删除）");
    Ok(Json(
        serde_json::json!({ "name": name, "status": "deleted" }),
    ))
}

/// 角色使用统计（总览 + 按角色 + 按任务类型 + 最近委托）。
pub async fn get_roles_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RolesStatsResponse>, ApiError> {
    let store = state.role_store();
    let mut total_calls: u32 = 0;
    let mut total_success: u32 = 0;
    let mut by_role: Vec<RoleUsageEntry> = Vec::new();
    for role in store
        .load_roles()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        let usage = store
            .load_role_usage(&role.name)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if usage.calls > 0 {
            total_calls += usage.calls;
            total_success += usage.success;
            by_role.push(RoleUsageEntry {
                name: role.name.clone(),
                usage: RoleUsageSummary {
                    calls: usage.calls,
                    success: usage.success,
                    failed: usage.failed,
                    success_rate: usage.success_rate(),
                    last_used: usage.last_used,
                },
            });
        }
    }
    // 按任务类型（委托历史分类计数，键序稳定）
    let records = store
        .load_delegation_records()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut by_type: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for r in &records {
        let t = tianyan::agent::categorize_task_by_keyword(&r.task);
        *by_type.entry(t).or_default() += 1;
    }
    let by_task_type: Vec<(String, usize)> = by_type.into_iter().collect();
    let recent: Vec<tianyan::roles::DelegationRecord> =
        records.iter().rev().take(10).cloned().collect();
    let success_rate = if total_calls == 0 {
        0.0
    } else {
        total_success as f32 / total_calls as f32
    };
    Ok(Json(RolesStatsResponse {
        total_calls,
        total_success,
        success_rate,
        by_role,
        by_task_type,
        recent,
    }))
}

async fn to_summary(store: &RoleStore, role: &tianyan::agent::AgentRole) -> RoleSummary {
    let purpose = role
        .system_prompt
        .as_deref()
        .map(|p| p.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "无系统提示".to_string());
    // 使用统计（退役信号 / 演化门控可见性）
    let usage = store
        .load_role_usage(&role.name)
        .await
        .ok()
        .filter(|u| u.calls > 0)
        .map(|u| RoleUsageSummary {
            calls: u.calls,
            success: u.success,
            failed: u.failed,
            success_rate: u.success_rate(),
            last_used: u.last_used,
        });
    RoleSummary {
        name: role.name.clone(),
        source: source_str(role.source),
        status: status_str(role.status),
        version: role.version,
        lineage: role.lineage.clone(),
        purpose,
        tool_count: role.tools.as_ref().map(|t| t.len()),
        model: role.model.clone(),
        max_turns: role.max_turns,
        usage,
    }
}

async fn to_detail(store: &RoleStore, role: &tianyan::agent::AgentRole) -> RoleDetail {
    RoleDetail {
        summary: to_summary(store, role).await,
        system_prompt: role.system_prompt.clone(),
        tools: role.tools.clone(),
    }
}

fn source_str(s: RoleSource) -> String {
    match s {
        RoleSource::Builtin => "builtin",
        RoleSource::User => "user",
        RoleSource::Learned => "learned",
    }
    .to_string()
}

fn status_str(s: RoleStatus) -> String {
    match s {
        RoleStatus::Active => "active",
        RoleStatus::Experimental => "experimental",
    }
    .to_string()
}

/// 角色路由（挂载于 `/api/v1`）。
pub fn routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/roles", get(list_roles))
        .route("/roles/stats", get(get_roles_stats))
        .route("/roles/{name}", get(get_role_detail).delete(delete_role))
        .route("/roles/{name}/reset", post(reset_role))
}
