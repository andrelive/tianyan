use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 技能信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// 技能标识
    pub id: String,
    /// 技能名称
    pub name: String,
    /// 技能描述
    pub description: String,
    /// 技能类别
    pub category: String,
    /// 版本号
    pub version: String,
    /// 是否启用
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 参数定义列表
    pub parameters: Option<Vec<SkillParameter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 图标
    pub icon: Option<String>,
}

/// 技能参数定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    /// 参数名称
    pub name: String,
    /// 参数描述
    pub description: String,
    /// 参数类型（序列化为 `type` 与前端类型定义对齐）
    #[serde(rename = "type")]
    pub param_type: String,
    /// 是否必填
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 默认值
    pub default_value: Option<Value>,
}

/// 执行技能请求
#[derive(Debug, Deserialize)]
pub struct ExecuteSkillRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 技能参数
    pub parameters: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 执行上下文
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
    /// 是否成功
    pub success: bool,
    /// 任务标识
    pub job_id: String,
    /// 技能标识
    pub skill_id: String,
    /// 响应消息
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 执行结果
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 错误信息
    pub error: Option<String>,
}

/// 列出技能响应
#[derive(Debug, Serialize)]
pub struct ListSkillsResponse {
    /// 技能列表
    pub skills: Vec<Skill>,
    /// 技能总数
    pub total: usize,
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

    #[test]
    fn test_skill_parameter_serializes_type_field() {
        // 前端类型定义使用 `type`，后端字段为 `param_type`，必须序列化为 `type`
        let param = SkillParameter {
            name: "verbose".to_string(),
            description: "详细输出".to_string(),
            param_type: "boolean".to_string(),
            required: false,
            default_value: None,
        };
        let json = serde_json::to_string(&param).unwrap();
        assert!(
            json.contains("\"type\":\"boolean\""),
            "序列化应输出 type 字段: {json}"
        );
        assert!(
            !json.contains("param_type"),
            "不应输出 param_type 字段: {json}"
        );
    }
}
