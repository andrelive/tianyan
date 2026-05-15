use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use tianyan::skills::{SkillExecutor, SkillRegistry};

use crate::api::skills::types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, Skill, SkillExecutionStatus,
    SkillParameter,
};

/// 技能服务，管理和执行技能
pub struct SkillService {
    registry: Arc<RwLock<SkillRegistry>>,
    executor: Arc<SkillExecutor>,
}

impl SkillService {
    /// 创建新的技能服务
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, executor: Arc<SkillExecutor>) -> Self {
        Self { registry, executor }
    }

    /// 列出所有可用技能
    pub async fn list_skills(&self) -> anyhow::Result<ListSkillsResponse> {
        info!("列出所有技能");

        let registry = self.registry.read().await;
        let core_skills = registry.list();

        let skills: Vec<Skill> = core_skills.into_iter().map(convert_core_skill).collect();
        let total = skills.len();

        debug!("找到 {} 个技能", total);
        Ok(ListSkillsResponse { skills, total })
    }

    /// 执行技能
    pub async fn execute_skill(
        &self,
        skill_id: &str,
        request: ExecuteSkillRequest,
    ) -> anyhow::Result<ExecuteSkillResponse> {
        info!("执行技能: {}", skill_id);
        debug!("参数: {:?}", request.parameters);

        let job_id = format!(
            "skill-job-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("id")
        );

        // 转换参数
        let parameters: HashMap<String, Value> = match request.parameters {
            Some(Value::Object(map)) => map.into_iter().collect(),
            Some(_other) => {
                return Ok(ExecuteSkillResponse {
                    success: false,
                    job_id,
                    skill_id: skill_id.to_string(),
                    message: "参数必须是 JSON 对象".to_string(),
                    result: None,
                    error: Some("Invalid parameters format".to_string()),
                });
            }
            None => HashMap::new(),
        };

        // 构建核心执行请求
        let core_request =
            tianyan::skills::SkillExecutionRequest::new(skill_id.to_string(), parameters);

        // 执行技能
        match self.executor.execute(core_request).await {
            Ok(result) => {
                let message = if result.success {
                    result
                        .output
                        .clone()
                        .unwrap_or_else(|| "执行成功".to_string())
                } else {
                    result
                        .error
                        .clone()
                        .unwrap_or_else(|| "执行失败".to_string())
                };

                // 构建结果 JSON
                let result_value = if result.success {
                    let mut data = result.data.clone();
                    if let Some(output) = &result.output {
                        data.insert("output".to_string(), Value::String(output.clone()));
                    }
                    if let Some(code) = result.exit_code {
                        data.insert("exit_code".to_string(), Value::Number(code.into()));
                    }
                    if !data.is_empty() {
                        Some(Value::Object(data.into_iter().collect()))
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(ExecuteSkillResponse {
                    success: result.success,
                    job_id,
                    skill_id: skill_id.to_string(),
                    message,
                    result: result_value,
                    error: result.error,
                })
            }
            Err(e) => {
                warn!("技能执行失败: {}", e);
                Ok(ExecuteSkillResponse {
                    success: false,
                    job_id,
                    skill_id: skill_id.to_string(),
                    message: format!("执行失败: {}", e),
                    result: None,
                    error: Some(e.to_string()),
                })
            }
        }
    }

    /// 获取技能执行状态
    pub async fn get_execution_status(
        &self,
        skill_id: &str,
        job_id: &str,
    ) -> anyhow::Result<SkillExecutionStatus> {
        info!("获取技能 {} 任务 {} 的状态", skill_id, job_id);

        // 当前执行器是同步完成的，直接返回完成状态
        // 后续如果需要异步执行，可以扩展此处逻辑
        Ok(SkillExecutionStatus {
            job_id: job_id.to_string(),
            skill_id: skill_id.to_string(),
            status: "completed".to_string(),
            progress: 1.0,
            result: Some(serde_json::json!({"message": "执行完成"})),
            error: None,
        })
    }
}

/// 将核心库的技能定义转换为 API 响应格式
fn convert_core_skill(core_skill: &tianyan::skills::Skill) -> Skill {
    let parameters = core_skill.parameters.as_ref().map(|schema| {
        schema
            .properties
            .iter()
            .map(|(name, def)| SkillParameter {
                name: name.clone(),
                description: def.description.clone().unwrap_or_default(),
                param_type: format!("{:?}", def.param_type).to_lowercase(),
                required: schema.required.contains(name),
                default_value: def.default.clone(),
            })
            .collect()
    });

    let category = match core_skill.category {
        tianyan::skills::SkillCategory::FileOperations => "file".to_string(),
        tianyan::skills::SkillCategory::SystemCommands => "system".to_string(),
        tianyan::skills::SkillCategory::NetworkRequests => "network".to_string(),
        tianyan::skills::SkillCategory::ApplicationControl => "app".to_string(),
        tianyan::skills::SkillCategory::DataProcessing => "data".to_string(),
        tianyan::skills::SkillCategory::Custom => "custom".to_string(),
    };

    let icon = match core_skill.category {
        tianyan::skills::SkillCategory::FileOperations => Some("file".to_string()),
        tianyan::skills::SkillCategory::SystemCommands => Some("code".to_string()),
        tianyan::skills::SkillCategory::NetworkRequests => Some("network".to_string()),
        _ => Some("tool".to_string()),
    };

    Skill {
        id: core_skill.id.clone(),
        name: core_skill.name.clone(),
        description: core_skill.description.clone(),
        category,
        version: core_skill.version.clone(),
        enabled: core_skill.enabled,
        parameters,
        icon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_service_creation() {
        // 仅验证类型可构造，实际测试需要异步运行时
    }
}
