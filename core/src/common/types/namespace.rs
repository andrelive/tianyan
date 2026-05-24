use serde::{Deserialize, Serialize};

/// 虚拟文件系统中的上下文命名空间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextNamespace {
    /// 用户信息（档案、偏好、实体）
    User,
    /// 会话记录（从 Memory 提升）
    Session,
    /// 记忆系统（长期记忆、事件）
    Memory,
    /// 知识库（文档、项目、数据）
    Knowledge,
    /// Agent 自身（核心提示词）
    Agent,
    /// 技能库（新增）
    Skill,
}

impl ContextNamespace {
    /// 所有命名空间的列表。
    pub const ALL: &[ContextNamespace] = &[
        ContextNamespace::User,
        ContextNamespace::Session,
        ContextNamespace::Memory,
        ContextNamespace::Knowledge,
        ContextNamespace::Agent,
        ContextNamespace::Skill,
    ];

    /// 获取此命名空间的目录名。
    pub fn dir_name(&self) -> &'static str {
        match self {
            ContextNamespace::User => "user",
            ContextNamespace::Session => "session",
            ContextNamespace::Memory => "memory",
            ContextNamespace::Knowledge => "knowledge",
            ContextNamespace::Agent => "agent",
            ContextNamespace::Skill => "skill",
        }
    }

    /// 从字符串解析。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "user" => Some(Self::User),
            "session" => Some(Self::Session),
            "memory" => Some(Self::Memory),
            "knowledge" => Some(Self::Knowledge),
            "agent" => Some(Self::Agent),
            "skill" => Some(Self::Skill),
            _ => None,
        }
    }
}

impl std::fmt::Display for ContextNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.dir_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_namespace_dir_name() {
        assert_eq!(ContextNamespace::User.dir_name(), "user");
        assert_eq!(ContextNamespace::Session.dir_name(), "session");
        assert_eq!(ContextNamespace::Memory.dir_name(), "memory");
        assert_eq!(ContextNamespace::Knowledge.dir_name(), "knowledge");
        assert_eq!(ContextNamespace::Agent.dir_name(), "agent");
        assert_eq!(ContextNamespace::Skill.dir_name(), "skill");
    }

    #[test]
    fn test_context_namespace_parse() {
        assert_eq!(
            ContextNamespace::parse("user"),
            Some(ContextNamespace::User)
        );
        assert_eq!(
            ContextNamespace::parse("session"),
            Some(ContextNamespace::Session)
        );
        assert_eq!(
            ContextNamespace::parse("memory"),
            Some(ContextNamespace::Memory)
        );
        assert_eq!(
            ContextNamespace::parse("knowledge"),
            Some(ContextNamespace::Knowledge)
        );
        assert_eq!(
            ContextNamespace::parse("agent"),
            Some(ContextNamespace::Agent)
        );
        assert_eq!(
            ContextNamespace::parse("skill"),
            Some(ContextNamespace::Skill)
        );
        assert_eq!(ContextNamespace::parse("invalid"), None);
        assert_eq!(
            ContextNamespace::parse("USER"),
            Some(ContextNamespace::User)
        );
    }

    #[test]
    fn test_context_namespace_display() {
        assert_eq!(format!("{}", ContextNamespace::User), "user");
        assert_eq!(format!("{}", ContextNamespace::Knowledge), "knowledge");
        assert_eq!(format!("{}", ContextNamespace::Session), "session");
        assert_eq!(format!("{}", ContextNamespace::Skill), "skill");
    }
}
