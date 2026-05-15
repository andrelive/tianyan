use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 技能信息
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    pub name: String,
    pub description: String,
    pub param_type: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<Value>,
}

/// 执行技能请求
#[derive(Debug, Deserialize)]
pub struct ExecuteSkillRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

impl ExecuteSkillRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref params) = self.parameters {
            let size = serde_json::to_string(params)
                .map_err(|e| format!("参数序列化失败: {}", e))?
                .len();
            if size > 1_000_000 {
                return Err("参数大小不能超过 1MB".to_string());
            }
        }
        if let Some(ref ctx) = self.context {
            let size = serde_json::to_string(ctx)
                .map_err(|e| format!("上下文序列化失败: {}", e))?
                .len();
            if size > 1_000_000 {
                return Err("上下文大小不能超过 1MB".to_string());
            }
        }
        Ok(())
    }
}

/// 执行技能响应
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
pub struct ListSkillsResponse {
    pub skills: Vec<Skill>,
    pub total: usize,
}

/// 技能执行状态
#[derive(Debug, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_serialization() {
        let skill = Skill {
            id: "test-skill".to_string(),
            name: "测试技能".to_string(),
            description: "一个测试技能".to_string(),
            category: "test".to_string(),
            version: "1.0.0".to_string(),
            enabled: true,
            parameters: None,
            icon: None,
        };
        let json = serde_json::to_string(&skill).unwrap();
        assert!(json.contains("test-skill"));
        assert!(json.contains("测试技能"));
    }

    #[test]
    fn test_execute_skill_request_deserialization() {
        let json = r#"{
            "parameters": {"query": "test"},
            "context": {"session_id": "123"}
        }"#;
        let req: ExecuteSkillRequest = serde_json::from_str(json).unwrap();
        assert!(req.parameters.is_some());
        assert!(req.context.is_some());
    }

    #[test]
    fn test_execute_skill_response_serialization() {
        let response = ExecuteSkillResponse {
            success: true,
            job_id: "job-123".to_string(),
            skill_id: "skill-456".to_string(),
            message: "成功".to_string(),
            result: Some(serde_json::json!({"data": "value"})),
            error: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("成功"));
    }
}
