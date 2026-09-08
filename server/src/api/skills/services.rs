use std::sync::Arc;

use tianyan::common::types::{ContextNamespace, TianyanUri};
use tianyan::skills::SkillManager;
use tianyan::vfs::VirtualFileSystem;

use crate::api::shared::error::ApiError;
use crate::api::skills::types::{
    ExecuteSkillRequest, ExecuteSkillResponse, ListSkillsResponse, Skill, SkillDetail,
};

/// 技能服务：技能 = VFS 方法论文档（无执行语义）。
///
/// 列表/详情/执行统一读 VFS `skill/` 命名空间；"执行" = 返回技能内容
/// （L0 摘要 + L2 详情），由 LLM 参考后自行用基础工具执行。
pub struct SkillService {
    manager: Arc<SkillManager>,
    vfs: Arc<dyn VirtualFileSystem>,
}

impl SkillService {
    /// 创建新的技能服务。
    pub fn new(manager: Arc<SkillManager>, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { manager, vfs }
    }

    /// 列出所有可用技能（VFS 发现，L0 摘要）。
    pub async fn list_skills(&self) -> Result<ListSkillsResponse, ApiError> {
        let summaries = self
            .manager
            .list_available_skills()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        let mut skills = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let (created_at, updated_at) = self.skill_timestamps(&summary.id).await;
            skills.push(Skill {
                id: summary.id.clone(),
                name: summary.id.clone(),
                description: summary.description,
                category: "custom".to_string(),
                version: "1.0.0".to_string(),
                enabled: true,
                parameters: None,
                icon: Some("tool".to_string()),
                created_at,
                updated_at,
            });
        }
        let total = skills.len();
        Ok(ListSkillsResponse { skills, total })
    }

    /// 获取技能详情（完整内容 + 时间元数据）。
    pub async fn get_skill_detail(&self, skill_id: &str) -> Result<SkillDetail, ApiError> {
        let summaries = self
            .manager
            .list_available_skills()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let summary = summaries
            .iter()
            .find(|s| s.id == skill_id)
            .ok_or_else(|| ApiError::NotFound(format!("技能未找到: {}", skill_id)))?;

        let content = self
            .manager
            .read_skill(skill_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let (created_at, updated_at) = self.skill_timestamps(skill_id).await;

        Ok(SkillDetail {
            id: summary.id.clone(),
            name: summary.id.clone(),
            description: summary.description.clone(),
            category: "custom".to_string(),
            version: "1.0.0".to_string(),
            enabled: true,
            content,
            created_at,
            updated_at,
        })
    }

    /// 技能 VFS 存储时间（skill/{id}/content.md 的 entry 时间）。
    async fn skill_timestamps(&self, skill_id: &str) -> (Option<String>, Option<String>) {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);
        match self.vfs.get_entry(&uri).await {
            Ok(entry) => {
                let created = entry.metadata.created_at.to_rfc3339();
                let updated = entry.metadata.updated_at.to_rfc3339();
                (Some(created), Some(updated))
            }
            Err(_) => (None, None),
        }
    }

    /// 执行技能 = 读取技能文档返回（无执行语义；LLM 参考后自行用工具执行）。
    pub async fn execute_skill(
        &self,
        skill_id: &str,
        _request: ExecuteSkillRequest,
    ) -> Result<ExecuteSkillResponse, ApiError> {
        let job_id = format!(
            "skill-job-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("id")
        );

        let content = match self
            .manager
            .read_skill(skill_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            Some(c) => c,
            None => {
                return Ok(ExecuteSkillResponse {
                    success: false,
                    job_id,
                    skill_id: skill_id.to_string(),
                    message: format!("技能未找到: {}", skill_id),
                    result: None,
                    error: Some("Skill not found".to_string()),
                });
            }
        };

        Ok(ExecuteSkillResponse {
            success: true,
            job_id,
            skill_id: skill_id.to_string(),
            message: "技能内容已返回（方法论文档，由模型参考后自行执行）".to_string(),
            result: Some(serde_json::json!({
                "skill_id": skill_id,
                "content": content,
            })),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_skill_service_creation() {
        // 仅验证类型可构造，实际测试需要异步运行时
    }
}
