//! VFS 技能管理器。
//!
//! 本模块提供从 VFS 的 skill/ 命名空间发现和管理技能的能力。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::skills::definition::Skill;
use crate::skills::types::{SecurityLevel, SkillCategory};
use crate::vfs::VirtualFileSystem;

/// 技能摘要信息。
#[derive(Debug, Clone)]
pub struct SkillSummary {
    /// 技能 ID。
    pub id: String,
    /// 技能描述（从语义层读取）。
    pub description: String,
}

/// VFS 技能管理器。
pub struct SkillManager {
    vfs: Arc<dyn VirtualFileSystem>,
}

impl SkillManager {
    /// 创建新的技能管理器。
    pub fn new(vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { vfs }
    }

    /// 列出所有可用技能。
    pub async fn list_available_skills(&self) -> Result<Vec<SkillSummary>> {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let entries = self.vfs.list(&skill_root).await?;

        let mut skills = Vec::new();
        for entry in entries {
            if entry.is_directory() {
                let id = entry.uri().path().last().cloned().unwrap_or_default();
                let description = self
                    .vfs
                    .read_abstract(entry.uri())
                    .await
                    .unwrap_or_default();
                skills.push(SkillSummary { id, description });
            }
        }
        Ok(skills)
    }

    /// 读取技能定义（优先 skill.md，回退 content.md）。
    pub async fn read_skill_definition(&self, skill_id: &str) -> Result<String> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);

        if self.vfs.file_exists(&uri, "skill.md").await? {
            self.vfs.read_file(&uri, "skill.md").await
        } else {
            self.vfs.read_file(&uri, "content.md").await
        }
    }

    /// 列出技能的参考资料。
    pub async fn list_references(&self, skill_id: &str) -> Result<Vec<String>> {
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![skill_id.to_string(), "references".to_string()],
        );
        self.vfs.list_files(&uri).await
    }

    /// 读取技能的参考资料。
    pub async fn read_reference(&self, skill_id: &str, ref_name: &str) -> Result<String> {
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![skill_id.to_string(), "references".to_string()],
        );
        self.vfs.read_file(&uri, ref_name).await
    }

    /// 列出技能的脚本文件。
    pub async fn list_scripts(&self, skill_id: &str) -> Result<Vec<String>> {
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![skill_id.to_string(), "scripts".to_string()],
        );
        self.vfs.list_files(&uri).await
    }

    /// 读取技能的脚本文件。
    pub async fn read_script(&self, skill_id: &str, script_name: &str) -> Result<String> {
        let uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![skill_id.to_string(), "scripts".to_string()],
        );
        self.vfs.read_file(&uri, script_name).await
    }

    /// 检查技能是否存在。
    pub async fn skill_exists(&self, skill_id: &str) -> Result<bool> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);
        self.vfs.exists(&uri).await
    }

    /// 从 VFS 加载所有已学习技能（GEPA 引擎产物），构造可注册的 [`Skill`] 元数据。
    ///
    /// 已学习技能没有内置 handler（本质是操作流程说明），注册后可供发现、
    /// 列出与语义检索；`call_skill` 执行时由 [`SkillExecutor`] 返回说明指引。
    ///
    /// 返回的技能使用 `Custom` 分类与 `Moderate` 安全级别（保守默认）。
    pub async fn load_learned_skills(&self) -> Result<Vec<Skill>> {
        let summaries = self.list_available_skills().await?;
        let mut skills = Vec::new();
        for summary in summaries {
            skills.push(
                Skill::new(&summary.id, &summary.id, &summary.description)
                    .with_category(SkillCategory::Custom)
                    .with_security_level(SecurityLevel::Moderate)
                    .with_tag("learned"),
            );
        }
        Ok(skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContentLevel;
    use crate::test_utils::MockVfs;

    #[test]
    fn test_skill_manager_new() {
        // 仅验证类型可构造
    }

    /// 回归测试：GEPA 学习技能写入 VFS 后可通过 load_learned_skills 加载
    /// 为 Skill 元数据（学习回路闭环的加载端）。
    #[tokio::test]
    async fn test_load_learned_skills_roundtrip() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let learned_uri = TianyanUri::new(ContextNamespace::Skill, vec!["learned_a".to_string()]);
        let mock = MockVfs::new();
        mock.add_directory(&skill_root, &learned_uri);
        mock.set_content(&learned_uri, ContentLevel::Abstract, "自动化的文档处理流程");
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs.clone());
        let skills = manager.load_learned_skills().await.unwrap();
        let learned = skills.iter().find(|s| s.id == "learned_a");
        assert!(learned.is_some(), "应加载到已学习技能");
        let skill = learned.unwrap();
        assert_eq!(skill.name, "learned_a");
        assert_eq!(skill.category, SkillCategory::Custom);
        assert_eq!(skill.security_level, SecurityLevel::Moderate);
        assert!(skill.tags.contains(&"learned".to_string()));
        assert!(skill.description.contains("文档处理"));
    }
}
