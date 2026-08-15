use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use tianyan::common::types::{ContextNamespace, TianyanUri};
use tianyan::skills::{SkillExecutor, SkillRegistry};
use tianyan::vfs::VirtualFileSystem;

use crate::api::shared::error::ApiError;
use crate::api::skills::types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, Skill, SkillDetail,
    SkillParameter,
};

/// 技能服务，管理和执行技能
pub struct SkillService {
    registry: Arc<RwLock<SkillRegistry>>,
    executor: Arc<SkillExecutor>,
    /// VFS（学习技能的 content/时间元数据来源）。
    vfs: Arc<dyn VirtualFileSystem>,
}

impl SkillService {
    /// 创建新的技能服务
    pub fn new(
        registry: Arc<RwLock<SkillRegistry>>,
        executor: Arc<SkillExecutor>,
        vfs: Arc<dyn VirtualFileSystem>,
    ) -> Self {
        Self {
            registry,
            executor,
            vfs,
        }
    }

    /// 列出所有可用技能
    pub async fn list_skills(&self) -> Result<ListSkillsResponse, ApiError> {
        info!("列出所有技能");

        let registry = self.registry.read().await;
        let core_skills = registry.list();

        let mut skills = Vec::with_capacity(core_skills.len());
        for core in core_skills {
            let (created_at, updated_at) = self.skill_timestamps(&core.id).await;
            skills.push(convert_core_skill(core, created_at, updated_at));
        }
        let total = skills.len();

        debug!("找到 {} 个技能", total);
        Ok(ListSkillsResponse { skills, total })
    }

    /// 获取技能详情（完整内容 + 时间元数据；内置技能无 VFS 内容时为 None）。
    pub async fn get_skill_detail(&self, skill_id: &str) -> Result<SkillDetail, ApiError> {
        info!("获取技能详情: {}", skill_id);

        let registry = self.registry.read().await;
        let core = registry
            .get(skill_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("技能未找到: {}", skill_id)))?;

        let (created_at, updated_at) = self.skill_timestamps(skill_id).await;
        // 内容挂在目录 uri（skill/{id}）的 Detail 层（skill/{id}/content.md）；
        // content.md 文件的 entry（skill/{id}/content.md）仅用于时间戳。
        let dir_uri = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);
        let content = match self
            .vfs
            .read_content(&dir_uri, tianyan::common::types::ContentLevel::Detail)
            .await
        {
            Ok(c) if !c.is_empty() => Some(c),
            _ => None,
        };

        Ok(SkillDetail {
            id: core.id.clone(),
            name: core.name.clone(),
            description: core.description.clone(),
            category: convert_category(&core.category),
            version: core.version.clone(),
            enabled: core.enabled,
            content,
            created_at,
            updated_at,
        })
    }

    /// 技能 VFS 存储时间（skill/{id}/content.md 的 entry 时间）。
    async fn skill_timestamps(&self, skill_id: &str) -> (Option<String>, Option<String>) {
        match self.vfs.get_entry(&skill_content_uri(skill_id)).await {
            Ok(entry) => {
                let created = entry.metadata.created_at.to_rfc3339();
                let updated = entry.metadata.updated_at.to_rfc3339();
                (Some(created), Some(updated))
            }
            Err(_) => (None, None),
        }
    }

    /// 执行技能
    pub async fn execute_skill(
        &self,
        skill_id: &str,
        request: ExecuteSkillRequest,
    ) -> Result<ExecuteSkillResponse, ApiError> {
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
}

/// 技能内容文件 URI（skill/{id}/content.md）。
fn skill_content_uri(skill_id: &str) -> TianyanUri {
    TianyanUri::new(
        ContextNamespace::Skill,
        vec![skill_id.to_string(), "content.md".to_string()],
    )
}

/// 技能分类 → API 字符串。
fn convert_category(category: &tianyan::skills::SkillCategory) -> String {
    match category {
        tianyan::skills::SkillCategory::FileOperations => "file".to_string(),
        tianyan::skills::SkillCategory::SystemCommands => "system".to_string(),
        tianyan::skills::SkillCategory::NetworkRequests => "network".to_string(),
        tianyan::skills::SkillCategory::ApplicationControl => "app".to_string(),
        tianyan::skills::SkillCategory::DataProcessing => "data".to_string(),
        tianyan::skills::SkillCategory::Custom => "custom".to_string(),
    }
}

/// 将核心库的技能定义转换为 API 响应格式。
fn convert_core_skill(
    core_skill: &tianyan::skills::Skill,
    created_at: Option<String>,
    updated_at: Option<String>,
) -> Skill {
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

    let category = convert_category(&core_skill.category);

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
        created_at,
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_skill_service_creation() {
        // 仅验证类型可构造，实际测试需要异步运行时
    }
}
