//! VFS 技能管理器。
//!
//! 本模块提供从 VFS 的 skill/ 命名空间发现和管理技能的能力。

use std::sync::Arc;

use crate::common::error::Result;
use crate::common::types::{ContextNamespace, TianyanUri};
use crate::storage::VirtualFileSystem;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_manager_new() {
        // 仅验证类型可构造
    }
}
