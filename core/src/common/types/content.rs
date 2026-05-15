use serde::{Deserialize, Serialize};

/// 分层摘要系统的内容层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ContentLevel {
    /// L0: 抽象层级，约 100 个 token，用于向量搜索和快速筛选
    #[default]
    Abstract,
    /// L1: 概览层级，约 2K 个 token，用于内容导航和重排序
    Overview,
    /// L2: 详情层级，完整内容，按需加载
    Detail,
}

impl ContentLevel {
    /// 获取此层级的近似 token 限制。
    pub fn token_limit(&self) -> Option<usize> {
        match self {
            ContentLevel::Abstract => Some(100),
            ContentLevel::Overview => Some(2000),
            ContentLevel::Detail => None,
        }
    }

    /// 获取此层级的文件名。
    pub fn file_name(&self) -> &'static str {
        match self {
            ContentLevel::Abstract => ".abstract.md",
            ContentLevel::Overview => ".overview.md",
            ContentLevel::Detail => "content.md",
        }
    }
}

/// 虚拟文件系统中的条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum EntryType {
    /// 目录条目
    Directory,
    /// 文件条目
    #[default]
    File,
}

/// 内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ContentSource {
    /// 用户上传的内容
    UserUpload,
    /// 代理生成的内容
    AgentGenerated,
    /// 从外部源导入的内容
    ExternalImport,
    /// 未知来源
    #[default]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_level_token_limit() {
        assert_eq!(ContentLevel::Abstract.token_limit(), Some(100));
        assert_eq!(ContentLevel::Overview.token_limit(), Some(2000));
        assert_eq!(ContentLevel::Detail.token_limit(), None);
    }

    #[test]
    fn test_content_level_file_name() {
        assert_eq!(ContentLevel::Abstract.file_name(), ".abstract.md");
        assert_eq!(ContentLevel::Overview.file_name(), ".overview.md");
        assert_eq!(ContentLevel::Detail.file_name(), "content.md");
    }

    #[test]
    fn test_content_level_default() {
        let level = ContentLevel::default();
        assert_eq!(level, ContentLevel::Abstract);
    }

    #[test]
    fn test_entry_type_default() {
        let entry_type = EntryType::default();
        assert_eq!(entry_type, EntryType::File);
    }

    #[test]
    fn test_content_source_default() {
        let source = ContentSource::default();
        assert_eq!(source, ContentSource::Unknown);
    }
}
