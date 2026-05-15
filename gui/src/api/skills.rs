use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{get, post, ApiResult};

/// 技能信息
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub version: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<SkillParameter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// 技能参数定义
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillParameter {
    pub name: String,
    pub description: String,
    pub param_type: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<Value>,
}

/// 执行技能请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteSkillRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

/// 执行技能响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecuteSkillResponse {
    pub success: bool,
    pub job_id: String,
    pub skill_id: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 列出技能响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListSkillsResponse {
    pub skills: Vec<Skill>,
    pub total: usize,
}

/// 技能执行状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillExecutionStatus {
    pub job_id: String,
    pub skill_id: String,
    pub status: String,
    pub progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn list_skills() -> ApiResult<ListSkillsResponse> {
    get("/skills").await
}

pub async fn execute_skill(
    skill_id: &str,
    request: &ExecuteSkillRequest,
) -> ApiResult<ExecuteSkillResponse> {
    post(&format!("/skills/{}/execute", skill_id), request).await
}

pub async fn get_skill_status(skill_id: &str, job_id: &str) -> ApiResult<SkillExecutionStatus> {
    get(&format!("/skills/{}/jobs/{}/status", skill_id, job_id)).await
}
