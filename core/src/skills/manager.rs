//! VFS 技能管理器。
//!
//! 本模块提供从 VFS 的 skill/ 命名空间发现和管理技能的能力。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::skills::definition::{Skill, SkillRegistry};
use crate::skills::types::{SecurityLevel, SkillCategory};
use crate::vfs::VirtualFileSystem;

/// 技能注册表刷新钩子：在会话转换点（压缩）增量注册 VFS 中已学习技能。
///
/// 压缩发生时 system 前缀已重建（摘要消息插入），此刻刷新注册表零额外
/// 缓存成本——是会话内唯一的免费刷新点。
#[async_trait::async_trait]
pub trait SkillRefresher: Send + Sync {
    /// 将 VFS 中已学习技能增量注册进注册表，返回新注册数。
    async fn refresh_skills(&self) -> Result<usize>;
}

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

    /// 将 VFS 中已学习技能增量同步到注册表（幂等：已注册的跳过）。
    ///
    /// 会话边界刷新：新会话首次请求时调用，使 GEPA 进化产物对新会话立即可用；
    /// 会话内不调用，保持 system 前缀稳定以最大化 prompt 缓存命中。
    ///
    /// 返回本次新注册的技能数量。
    pub async fn refresh_registry(&self, registry: &mut SkillRegistry) -> Result<usize> {
        let learned = self.load_learned_skills().await?;
        let mut registered = 0usize;
        for skill in learned {
            if !registry.contains(&skill.id) {
                registry.register(skill);
                registered += 1;
            }
        }
        Ok(registered)
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

    /// 回归测试：refresh_registry 幂等且增量——首次注册全部、重复刷新 0 新增、
    /// VFS 新增技能后只注册增量（会话边界刷新语义）。
    #[tokio::test]
    async fn test_refresh_registry_idempotent_and_incremental() {
        let skill_root = TianyanUri::new(ContextNamespace::Skill, vec![]);
        let learned_a = TianyanUri::new(ContextNamespace::Skill, vec!["learned_a".to_string()]);
        let mock = Arc::new(MockVfs::new());
        mock.add_directory(&skill_root, &learned_a);
        mock.set_content(&learned_a, ContentLevel::Abstract, "技能 A 摘要");
        let vfs: Arc<dyn VirtualFileSystem> = mock.clone();

        let manager = SkillManager::new(vfs.clone());
        let mut registry = SkillRegistry::new();

        let first = manager.refresh_registry(&mut registry).await.unwrap();
        assert_eq!(first, 1, "首次刷新应注册全部学习技能");
        assert!(registry.contains("learned_a"));

        let second = manager.refresh_registry(&mut registry).await.unwrap();
        assert_eq!(second, 0, "重复刷新应幂等（0 新注册）");

        // VFS 新增技能后刷新只注册增量
        let learned_b = TianyanUri::new(ContextNamespace::Skill, vec!["learned_b".to_string()]);
        mock.add_directory(&skill_root, &learned_b);
        mock.set_content(&learned_b, ContentLevel::Abstract, "技能 B 摘要");
        let third = manager.refresh_registry(&mut registry).await.unwrap();
        assert_eq!(third, 1, "增量刷新应只注册新技能");
        assert!(registry.contains("learned_b"));
        assert_eq!(registry.count(), 2);
    }
}
