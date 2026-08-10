use serde::{Deserialize, Serialize};

use super::namespace::ContextNamespace;
use super::uri::TianyanUri;

/// 记忆类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    /// 用户偏好。
    Preference,
    /// 重要决定。
    Decision,
    /// 成功案例/任务。
    SuccessfulCase,
    /// 失败案例/任务（经验教训）。
    FailedCase,
    /// 学习到的模式。
    Pattern,
    /// 实体信息。
    Entity,
    /// 一般事实。
    Fact,
}

impl MemoryCategory {
    /// 获取此类别的存储路径前缀。
    pub fn storage_path(&self) -> &'static str {
        match self {
            MemoryCategory::Preference => "user/preferences",
            MemoryCategory::Decision => "memory/events/decisions",
            MemoryCategory::SuccessfulCase => "memory/cases/successful_tasks",
            MemoryCategory::FailedCase => "memory/cases/failed_tasks",
            MemoryCategory::Pattern => "agent/patterns",
            MemoryCategory::Entity => "user/entities",
            MemoryCategory::Fact => "memory/facts",
        }
    }

    /// 获取此类别记忆的默认 URI。
    pub fn default_uri(&self, id: &str) -> TianyanUri {
        let path_parts: Vec<String> = self
            .storage_path()
            .split('/')
            .map(|s| s.to_string())
            .chain(std::iter::once(id.to_string()))
            .collect();

        let category = match path_parts.first().map(|s| s.as_str()) {
            Some("user") => ContextNamespace::User,
            Some("memory") => ContextNamespace::Memory,
            Some("agent") => ContextNamespace::Agent,
            _ => ContextNamespace::Memory,
        };

        let remaining: Vec<String> = path_parts.into_iter().skip(1).collect();
        TianyanUri::new(category, remaining)
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::Preference => write!(f, "preference"),
            MemoryCategory::Decision => write!(f, "decision"),
            MemoryCategory::SuccessfulCase => write!(f, "successful_case"),
            MemoryCategory::FailedCase => write!(f, "failed_case"),
            MemoryCategory::Pattern => write!(f, "pattern"),
            MemoryCategory::Entity => write!(f, "entity"),
            MemoryCategory::Fact => write!(f, "fact"),
        }
    }
}

/// 长期记忆系统中的记忆条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// 唯一记忆标识符。
    pub id: String,
    /// 存储 URI。
    pub uri: TianyanUri,
    /// 记忆内容。
    pub content: String,
    /// 记忆类别。
    pub category: MemoryCategory,
    /// 重要性分数（0.0 - 1.0）。
    pub importance: f32,
    /// 访问次数。
    pub access_count: u64,
    /// 创建时间戳。
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 最后更新时间戳。
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// 最后访问时间戳。
    pub last_accessed: Option<chrono::DateTime<chrono::Utc>>,
    /// 记忆标签。
    pub tags: Vec<String>,
    /// 来源会话 ID。
    pub source_session: Option<String>,
    /// 来源消息 ID 列表（消息级溯源；提取自会话 JSONL 中对应消息的 id）。
    #[serde(default)]
    pub source_message_ids: Vec<String>,
    /// 相关记忆 ID。
    pub related_memories: Vec<String>,
}

impl MemoryEntry {
    /// 创建新的记忆条目。
    pub fn new(
        id: impl Into<String>,
        content: impl Into<String>,
        category: MemoryCategory,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: id.into(),
            uri: category.default_uri(&format!("{}", now.timestamp())),
            content: content.into(),
            category,
            importance: 0.5,
            access_count: 0,
            created_at: now,
            updated_at: now,
            last_accessed: None,
            tags: Vec::new(),
            source_session: None,
            source_message_ids: Vec::new(),
            related_memories: Vec::new(),
        }
    }

    /// 记录对此记忆的访问。
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_accessed = Some(chrono::Utc::now());
    }

    /// 更新重要性分数。
    pub fn set_importance(&mut self, importance: f32) {
        self.importance = importance.clamp(0.0, 1.0);
        self.updated_at = chrono::Utc::now();
    }

    /// 添加标签。
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    /// 获取记忆天数。
    pub fn age_days(&self) -> i64 {
        (chrono::Utc::now() - self.created_at).num_days()
    }

    /// 获取距离上次访问的天数。
    pub fn days_since_access(&self) -> Option<i64> {
        self.last_accessed
            .map(|la| (chrono::Utc::now() - la).num_days())
    }

    /// 设置重要性并返回自身（链式调用）。
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// 设置标签并返回自身（链式调用）。
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// 设置来源会话并返回自身（链式调用）。
    pub fn with_source_session(mut self, session_id: impl Into<String>) -> Self {
        self.source_session = Some(session_id.into());
        self
    }

    /// 设置来源消息 ID 列表并返回自身（链式调用）。
    pub fn with_source_message_ids(mut self, ids: Vec<String>) -> Self {
        self.source_message_ids = ids;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_category_uri() {
        let uri = MemoryCategory::Preference.default_uri("pref-1");
        assert!(uri.to_string().contains("user"));
        assert!(uri.to_string().contains("preferences"));
    }

    #[test]
    fn test_memory_category_display() {
        assert_eq!(format!("{}", MemoryCategory::Preference), "preference");
        assert_eq!(format!("{}", MemoryCategory::Decision), "decision");
        assert_eq!(format!("{}", MemoryCategory::Fact), "fact");
    }

    #[test]
    fn test_memory_entry() {
        let entry = MemoryEntry::new("mem-1", "测试内容", MemoryCategory::Preference);
        assert_eq!(entry.id, "mem-1");
        assert_eq!(entry.content, "测试内容");
        assert_eq!(entry.category, MemoryCategory::Preference);
        assert_eq!(entry.access_count, 0);
    }

    #[test]
    fn test_memory_access() {
        let mut entry = MemoryEntry::new("mem-1", "测试", MemoryCategory::Fact);
        entry.record_access();
        entry.record_access();
        assert_eq!(entry.access_count, 2);
        assert!(entry.last_accessed.is_some());
    }

    #[test]
    fn test_memory_importance() {
        let mut entry = MemoryEntry::new("mem-1", "测试", MemoryCategory::Fact);
        entry.set_importance(0.8);
        assert!((entry.importance - 0.8).abs() < 0.001);

        entry.set_importance(1.5);
        assert!((entry.importance - 1.0).abs() < 0.001);
    }
}
