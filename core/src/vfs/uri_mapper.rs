//! URI 到文件系统路径的映射器。

use std::path::{Path, PathBuf};

use crate::common::types::TianyanUri;
use crate::config::StorageConfig;

/// URI 到文件系统路径的映射器。
#[derive(Debug, Clone)]
pub struct UriMapper {
    config: StorageConfig,
}

impl UriMapper {
    /// 使用给定配置创建新的 URI 映射器。
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    /// 获取存储配置。
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// 将 URI 转换为文件系统路径。
    ///
    /// 示例：
    /// - `tianyan://user/profile/basic_info` -> `~/.tianyan/user/profile/basic_info`
    pub fn uri_to_path(&self, uri: &TianyanUri) -> PathBuf {
        let mut path = self.config.data_dir.clone();
        path.push(uri.namespace().dir_name());
        for segment in uri.path() {
            path.push(segment);
        }
        path
    }

    /// 获取抽象内容路径（L0）。
    ///
    /// 抽象内容存储在父目录中，文件名为 `.abstract.md`。
    pub fn get_abstract_path(&self, uri: &TianyanUri) -> PathBuf {
        let dir_path = self.uri_to_path(uri);
        dir_path.join(".meta").join("abstract.md")
    }

    /// 获取概览内容路径（L1）。
    ///
    /// 概览内容存储在父目录中，文件名为 `.overview.md`。
    pub fn get_overview_path(&self, uri: &TianyanUri) -> PathBuf {
        let dir_path = self.uri_to_path(uri);
        dir_path.join(".meta").join("overview.md")
    }

    /// 获取详情内容路径（L2）。
    pub fn get_detail_path(&self, uri: &TianyanUri) -> PathBuf {
        let dir_path = self.uri_to_path(uri);
        let extension = Self::get_detail_extension(uri);
        dir_path.join(format!("content.{}", extension))
    }

    /// 根据 URI 路径确定 Detail 层的文件扩展名。
    ///
    /// 规则：
    /// - 会话（`memory/sessions/`）使用 JSON
    /// - 记忆（`memory/long_term/`）使用 JSON
    /// - 知识库（`knowledge/`）使用 Markdown
    /// - 其他默认使用 Markdown
    pub fn get_detail_extension(uri: &TianyanUri) -> &'static str {
        let path = uri.path();
        let namespace = uri.namespace();

        match namespace {
            crate::common::types::ContextNamespace::Session => "jsonl",
            crate::common::types::ContextNamespace::Skill => "md",
            crate::common::types::ContextNamespace::Memory => {
                if !path.is_empty() {
                    match path[0].as_str() {
                        "sessions" | "long_term" => "jsonl",
                        _ => "md",
                    }
                } else {
                    "md"
                }
            }
            crate::common::types::ContextNamespace::Knowledge => "md",
            _ => "md",
        }
    }

    /// 检查路径是否为特殊文件（抽象、概览、索引等）。
    pub fn is_special_file(path: &Path) -> bool {
        if path.components().any(|c| c.as_os_str() == ".meta") {
            return true;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            name.starts_with('.')
                || name == "index.json"
                || name.starts_with("content.")
                || name == "thumbnail.jpg"
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::ContextNamespace;

    #[test]
    fn test_uri_mapper_uri_to_path() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );
        let path = mapper.uri_to_path(&uri);

        assert!(path.to_string_lossy().contains("user"));
        assert!(path.to_string_lossy().contains("profile"));
        assert!(path.to_string_lossy().contains("basic_info"));
    }

    #[test]
    fn test_uri_mapper_abstract_path() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );
        let path = mapper.get_abstract_path(&uri);

        assert!(path.ends_with(".meta\\abstract.md") || path.ends_with(".meta/abstract.md"));
    }

    #[test]
    fn test_uri_mapper_overview_path() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );
        let path = mapper.get_overview_path(&uri);

        assert!(path.ends_with(".meta\\overview.md") || path.ends_with(".meta/overview.md"));
    }

    #[test]
    fn test_uri_mapper_detail_path() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::User,
            vec!["profile".to_string(), "basic_info".to_string()],
        );
        let path = mapper.get_detail_path(&uri);

        assert!(path.ends_with("content.md"));
    }

    #[test]
    fn test_uri_mapper_detail_path_sessions() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec![
                "sessions".to_string(),
                "2024".to_string(),
                "session_001".to_string(),
            ],
        );
        let path = mapper.get_detail_path(&uri);

        assert!(path.ends_with("content.jsonl"));
    }

    #[test]
    fn test_uri_mapper_detail_path_long_term() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["long_term".to_string(), "memory_001".to_string()],
        );
        let path = mapper.get_detail_path(&uri);

        assert!(path.ends_with("content.jsonl"));
    }

    #[test]
    fn test_uri_mapper_detail_path_knowledge() {
        let config = StorageConfig::default();
        let mapper = UriMapper::new(config);

        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "api_spec".to_string()],
        );
        let path = mapper.get_detail_path(&uri);

        assert!(path.ends_with("content.md"));
    }

    #[test]
    fn test_get_detail_extension() {
        let session_uri = TianyanUri::new(
            ContextNamespace::Session,
            vec!["2024-01-15".to_string(), "test".to_string()],
        );
        assert_eq!(UriMapper::get_detail_extension(&session_uri), "jsonl");

        let skill_uri = TianyanUri::new(ContextNamespace::Skill, vec!["file_ops".to_string()]);
        assert_eq!(UriMapper::get_detail_extension(&skill_uri), "md");

        let memory_session_uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["sessions".to_string(), "test".to_string()],
        );
        assert_eq!(
            UriMapper::get_detail_extension(&memory_session_uri),
            "jsonl"
        );

        let long_term_uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["long_term".to_string(), "test".to_string()],
        );
        assert_eq!(UriMapper::get_detail_extension(&long_term_uri), "jsonl");

        let other_memory_uri = TianyanUri::new(
            ContextNamespace::Memory,
            vec!["other".to_string(), "test".to_string()],
        );
        assert_eq!(UriMapper::get_detail_extension(&other_memory_uri), "md");

        let knowledge_uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "test".to_string()],
        );
        assert_eq!(UriMapper::get_detail_extension(&knowledge_uri), "md");

        let user_uri = TianyanUri::new(ContextNamespace::User, vec!["profile".to_string()]);
        assert_eq!(UriMapper::get_detail_extension(&user_uri), "md");
    }

    #[test]
    fn test_is_special_file() {
        assert!(UriMapper::is_special_file(Path::new(".abstract.md")));
        assert!(UriMapper::is_special_file(Path::new(".overview.md")));
        assert!(UriMapper::is_special_file(Path::new("index.json")));
        assert!(UriMapper::is_special_file(Path::new("content.md")));
        assert!(UriMapper::is_special_file(Path::new("content.json")));
        assert!(UriMapper::is_special_file(Path::new(".meta/abstract.md")));
        assert!(UriMapper::is_special_file(Path::new(".meta/overview.md")));
        assert!(!UriMapper::is_special_file(Path::new("normal_file.md")));
    }
}
