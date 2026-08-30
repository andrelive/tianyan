//! 角色基础类型（ADR-016：统一角色实体模型）。
//!
//! **纯类型层**：被 config（`[agent_roles]` 配置）、agent（注册表/委托）、
//! scheduler（演化任务）等各方依赖，不依赖任何领域模块——打破
//! `config ↔ agent` 与 `scheduler ↔ agent` 循环（这些模块只需本层的类型，
//! 不应反向依赖 agent 的注册表/存储实现）。

use serde::{Deserialize, Serialize};

/// 角色来源（ADR-016：三类来源平级，来源只是元数据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleSource {
    /// 内置种子（researcher / editor / reviewer；首次启动写入 VFS）。
    Builtin,
    /// 用户配置（`[agent_roles]` 节；降级为种子来源）。
    #[default]
    User,
    /// 学习演化（GEPA 角色管线产物）。
    Learned,
}

/// 角色激活状态（ADR-016：试验性只展示不可调用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleStatus {
    /// 正式：可被 delegate_to_agent 调用。
    #[default]
    Active,
    /// 试验性：出现在 delegate 描述中供评估，调用被拒绝。
    Experimental,
}

impl RoleSource {
    /// 是否用户来源（配置序列化时省略该字段）。
    pub(crate) fn is_user(&self) -> bool {
        *self == Self::User
    }
}

impl RoleStatus {
    /// 是否正式（配置序列化时省略该字段）。
    pub(crate) fn is_active(&self) -> bool {
        *self == Self::Active
    }
}

/// 子 Agent 角色定义。
///
/// 所有字段均为可选：缺省时回落到主 Agent 配置（模型/轮数/超时）或不生效
/// （系统提示 / 工具白名单）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRole {
    /// 角色名（配置节 `[agent_roles.<name>]` 的键，序列化时省略——名字即键）。
    #[serde(skip)]
    pub name: String,
    /// 角色使用的模型名称（与主 Agent 模型同空间；缺省回落主 Agent 模型）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 角色系统提示（缺省无系统提示，仅携带任务描述）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 工具白名单（缺省不限制——与主 Agent 相同；Some 时白名单外工具被拒绝）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// 子 Agent 最大循环轮数（缺省 200，范围 1-500）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// 委托整体超时（秒；缺省不限制）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 来源（内置种子 / 用户配置 / 学习演化；缺省 User）。
    #[serde(default, skip_serializing_if = "RoleSource::is_user")]
    pub source: RoleSource,
    /// 进化版本（1 起始；学习更新时递增；缺省视为 1——roundtrip 保真）。
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub version: u32,
    /// 激活状态（缺省 Active）。
    #[serde(default, skip_serializing_if = "RoleStatus::is_active")]
    pub status: RoleStatus,
    /// 进化来源角色名（回退链；内置/用户配置为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<String>,
}

/// 版本 1 判定（序列化省略）。
fn is_one(v: &u32) -> bool {
    *v == 1
}

/// 版本缺省值（1）。
fn one() -> u32 {
    1
}

/// 角色使用统计（ADR-016 P3：退役信号/演化门控输入）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RoleUsage {
    /// 累计调用次数。
    pub calls: u32,
    /// 成功次数。
    pub success: u32,
    /// 失败次数。
    pub failed: u32,
    /// 最后使用时间（epoch 毫秒）。
    pub last_used: i64,
}

impl RoleUsage {
    /// 记录一次调用结果（success 决定成功/失败计数）。
    pub fn record(&mut self, success: bool) {
        self.calls += 1;
        if success {
            self.success += 1;
        } else {
            self.failed += 1;
        }
        self.last_used = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
    }

    /// 成功率（无调用时 0）。
    pub fn success_rate(&self) -> f32 {
        if self.calls == 0 {
            0.0
        } else {
            self.success as f32 / self.calls as f32
        }
    }
}

/// 单次委托记录（ADR-016：统计面板明细）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationRecord {
    /// 委托时间（epoch 毫秒）。
    pub ts: i64,
    /// 角色名。
    pub role: String,
    /// 任务描述（截断）。
    pub task: String,
    /// 是否成功。
    pub success: bool,
    /// 会话模式（continue/new/discard）。
    pub mode: String,
    /// 委托耗时（毫秒）。
    pub duration_ms: u64,
    /// 子 Agent token 消耗。
    pub tokens: usize,
}
