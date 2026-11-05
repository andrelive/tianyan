//! VFS 技能管理器。
//!
//! 技能 = VFS `skill/` 命名空间下的方法论文档（content.md + abstract.md）。
//! 本模块提供发现（L0 摘要列表）与读取（L2 详情）能力，是技能的唯一读入口。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{system_paths, ContentLevel, ContextNamespace, TianyanUri};
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
            // 技能有两种存储形态，**两种都必须列出**：
            // - `skill/{id}`（单文件）：正文即 detail 内容——这是演化任务后期
            //   写的主流形态（现网 44 个）；
            // - `skill/{id}/`（目录）：多文件技能（content.md + abstract.md）。
            //
            // 此前这里只认 `is_directory()`，导致 44 个文件形态技能从未出现在
            // 任何列表里：技能面板看不到、演化任务的注册表清单看不到，也无从注入。
            // 归档与评审记录由 `system_paths` 统一排除（判定单一真相源）。
            if system_paths::is_system_path(entry.uri()) {
                continue;
            }
            let id = entry.uri().path().last().cloned().unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let description = self
                .vfs
                .read_abstract(entry.uri())
                .await
                .unwrap_or_default();
            skills.push(SkillSummary { id, description });
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

    /// 归档技能（软删除）：`skill/{id}` → `skill/_archive/{id}`。
    ///
    /// 内容与向量点随 `move_entry` 一起迁移，可恢复；归档子路径被
    /// `system_paths::is_system_path` 排除，故归档后不再出现在列表、检索与
    /// 上下文前缀中。
    ///
    /// 返回 `Ok(false)` 的三种情况都**不算失败**：技能不存在（可能已删）、
    /// 目标已存在（重复归档）、参数是保留名（`_archive` / `_reviews`）。
    pub async fn archive_skill(&self, skill_id: &str) -> Result<bool> {
        if skill_id.is_empty()
            || skill_id == crate::skills::ARCHIVE_PREFIX
            || skill_id == crate::skills::REVIEWS_PREFIX
        {
            return Ok(false);
        }

        let source = TianyanUri::new(ContextNamespace::Skill, vec![skill_id.to_string()]);
        if !self.vfs.exists(&source).await? {
            return Ok(false);
        }

        let destination = TianyanUri::new(
            ContextNamespace::Skill,
            vec![
                crate::skills::ARCHIVE_PREFIX.to_string(),
                skill_id.to_string(),
            ],
        );
        if self.vfs.exists(&destination).await? {
            return Ok(false);
        }

        // 多文件技能（目录含子条目）当前不支持归档：`move_entry` 只迁移顶层
        // 条目，子条目会被静默孤儿化（它会直接拒绝）。现网目录形态技能都是
        // 空目录（正文存节点自身），正常路径不受影响；真出现多文件技能时
        // 在此扩展为递归迁移。
        self.vfs.move_entry(&source, &destination).await?;
        Ok(true)
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

    /// 回归测试：**文件形态**技能（`skill/{id}` 是文件）同样要被列出。
    ///
    /// 判别力：`list_available_skills` 若恢复成只认 `is_directory()`，本用例立刻
    /// 变红——现网 44 个技能正是这种形态，漏掉它们等于技能面板与上下文注入
    /// 看不到绝大部分技能。
    #[tokio::test]
    async fn test_list_includes_file_shaped_skills() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let file_skill = TianyanUri::new(ContextNamespace::Skill, vec!["file_skill".to_string()]);
        let mock = MockVfs::new();
        mock.add_entry(&skill_root, &file_skill);
        mock.set_content(&file_skill, ContentLevel::Abstract, "文件形态技能摘要");
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs);
        let skills = manager.list_available_skills().await.unwrap();
        assert!(
            skills.iter().any(|s| s.id == "file_skill"),
            "文件形态技能必须被列出，实际: {:?}",
            skills.iter().map(|s| s.id.as_str()).collect::<Vec<_>>()
        );
    }

    /// 回归测试：归档目录（`skill/_archive`）不得被当成技能列出。
    #[tokio::test]
    async fn test_list_skips_archive_dir() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let archive_uri = TianyanUri::new(
            ContextNamespace::Skill,
            vec![crate::skills::ARCHIVE_PREFIX.to_string()],
        );
        let mock = MockVfs::new();
        mock.add_directory(&skill_root, &archive_uri);
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(mock);

        let manager = SkillManager::new(vfs);
        let skills = manager.list_available_skills().await.unwrap();
        assert!(skills.is_empty(), "_archive 不应被列为技能");
    }

    /// 归档：`skill/{id}` → `skill/_archive/{id}`；不存在或重复归档返回 false。
    #[tokio::test]
    async fn test_archive_skill_moves_to_archive() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let skill = TianyanUri::new(ContextNamespace::Skill, vec!["old-skill".to_string()]);
        let mock = Arc::new(MockVfs::new());
        mock.add_entry(&skill_root, &skill);
        mock.set_content(&skill, ContentLevel::Detail, "# 正文");
        let vfs: Arc<dyn VirtualFileSystem> = mock.clone();

        let manager = SkillManager::new(vfs);
        assert!(
            manager.archive_skill("old-skill").await.unwrap(),
            "存在的技能应归档成功"
        );

        let calls = mock.move_calls();
        assert_eq!(calls.len(), 1, "应发生一次 move_entry");
        assert_eq!(calls[0].0, "tianyan://skill/old-skill");
        assert_eq!(calls[0].1, "tianyan://skill/_archive/old-skill");

        assert!(
            !manager.archive_skill("not-exist").await.unwrap(),
            "不存在的技能返回 false"
        );
        assert!(
            !manager
                .archive_skill(crate::skills::ARCHIVE_PREFIX)
                .await
                .unwrap(),
            "保留名不得归档"
        );
    }
}
