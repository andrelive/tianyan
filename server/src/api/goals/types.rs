//! 目标 API 请求/响应类型。

use serde::Deserialize;
use tianyan::goals::GoalStatus;

/// 创建目标请求。
#[derive(Debug, Deserialize)]
pub struct CreateGoalRequest {
    /// 标题（必填，非空）。
    pub title: String,
    /// 详细描述（可选）。
    #[serde(default)]
    pub description: Option<String>,
    /// 目标日期（epoch 秒；可选）。
    #[serde(default)]
    pub target_date: Option<i64>,
    /// 归属会话 id（可选；正常路径由 goal 工具自动归属当前会话，
    /// REST 直传便于 QA/测试造会话绑定数据）。
    #[serde(default)]
    pub session_id: Option<String>,
}

/// 更新目标请求（全部字段可选；缺省 = 不修改）。
#[derive(Debug, Deserialize)]
pub struct UpdateGoalRequest {
    /// 新标题（空串 = 不修改）。
    #[serde(default)]
    pub title: Option<String>,
    /// 新描述（缺省 = 不修改）。
    #[serde(default)]
    pub description: Option<String>,
    /// 新状态（active/completed/archived；缺省 = 不修改）。
    #[serde(default)]
    pub status: Option<String>,
    /// 目标日期（<=0 = 清除）。
    #[serde(default)]
    pub target_date: Option<i64>,
}

impl UpdateGoalRequest {
    /// 解析状态（未知值 → 400）。
    pub fn parse_status(&self) -> Result<Option<GoalStatus>, String> {
        match &self.status {
            None => Ok(None),
            Some(s) => GoalStatus::parse(s)
                .map(Some)
                .ok_or_else(|| format!("无效的状态：{s}（可选：active / completed / archived）")),
        }
    }
}
