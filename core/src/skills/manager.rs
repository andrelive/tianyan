//! VFS 技能管理器。
//!
//! 技能 = VFS `skill/` 命名空间下的方法论文档（content.md + abstract.md）。
//! 本模块提供发现（L0 摘要列表）与读取（L2 详情）能力，是技能的唯一读入口。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use crate::vfs::VirtualFileSystem;

/// 技能摘要信息。
#[derive(Debug, Clone)]
pub struct SkillSummary {
    /// 技能 ID。
    pub id: String,
    /// 技能描述（L0 摘要）。
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

    /// 列出所有可用技能（VFS `skill/` 命名空间下的目录条目）。
    ///
    /// 评审记录目录（`_reviews`）不是技能，跳过。
    pub async fn list_available_skills(&self) -> Result<Vec<SkillSummary>> {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let entries = self.vfs.list(&skill_root).await?;

        let mut skills = Vec::new();
        for entry in entries {
            if entry.is_directory() {
                let id = entry.uri().path().last().cloned().unwrap_or_default();
                if id == crate::skills::REVIEWS_PREFIX {
                    continue;
                }
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

    /// 读取技能完整内容（L2 详情）。
    ///
    /// 返回 `None` 表示技能不存在（VFS 无该目录或内容为空）。
    pub async fn read_skill(&self, skill_id: &str) -> Result<Option<String>> {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);
        if !self.vfs.exists(&uri).await? {
            return Ok(None);
        }
        let content = self.vfs.read_content(&uri, ContentLevel::Detail).await?;
        if content.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::MockVfs;

    #[test]
    fn test_skill_manager_new() {
        // 仅验证类型可构造
    }

    /// 回归测试：VFS 技能目录可被发现（L0 摘要列表）。
    #[tokio::test]
    async fn test_list_available_skills() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let learned_uri = TianyanUri::new(ContextNamespace::Skill, vec!["learned_a".to_string()]);
        let mock = MockVfs::new();
        mock.add_directory(&skill_root, &learned_uri);
        mock.set_content(&learned_uri, ContentLevel::Abstract, "自动化的文档处理流程");
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs.clone());
        let skills = manager.list_available_skills().await.unwrap();
        let learned = skills.iter().find(|s| s.id == "learned_a");
        assert!(learned.is_some(), "应发现已学习技能");
        assert!(learned.unwrap().description.contains("文档处理"));
    }

    /// 回归测试：评审记录目录（_reviews）不应被当作技能列出。
    #[tokio::test]
    async fn test_list_skips_reviews_dir() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let reviews_uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![crate::skills::REVIEWS_PREFIX.to_string()],
        );
        let mock = MockVfs::new();
        mock.add_directory(&skill_root, &reviews_uri);
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs.clone());
        let skills = manager.list_available_skills().await.unwrap();
        assert!(skills.is_empty(), "_reviews 不应被列为技能");
    }

    /// 回归测试：read_skill 读取 L2 详情；不存在的技能返回 None。
    #[tokio::test]
    async fn test_read_skill() {
        let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["learned_a".to_string()]);
        let mock = MockVfs::new();
        mock.set_content(&skill_uri, ContentLevel::Detail, "# 技能内容\n\n步骤说明");
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs.clone());
        let content = manager.read_skill("learned_a").await.unwrap();
        assert!(content.is_some());
        assert!(content.unwrap().contains("步骤说明"));

        let missing = manager.read_skill("not_exist").await.unwrap();
        assert!(missing.is_none(), "不存在的技能应返回 None");
    }
}
