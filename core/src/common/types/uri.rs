use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use url::Url;

use super::namespace::ContextNamespace;
use crate::common::error;

/// Tianyan 虚拟文件系统使用的 URI 方案。
pub const TIANYAN_URI_SCHEME: &str = "tianyan";

/// Agent 命名空间下的固定子路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentPath {
    /// 核心提示词 (tianyan://agent/soul)
    Soul,
    /// 学习规则目录 (tianyan://agent/learned/)
    Learned,
}

impl AgentPath {
    /// 获取路径段名称。
    pub fn as_segment(&self) -> &'static str {
        match self {
            Self::Soul => "soul",
            Self::Learned => "learned",
        }
    }

    /// 构建对应的 TianyanUri（根路径，不含子路径）。
    pub fn uri(&self) -> TianyanUri {
        TianyanUri::new(ContextNamespace::Agent, vec![self.as_segment().to_string()])
    }

    /// 学习规则归档目录（tianyan://agent/learned/archive/；过时规则归档段）。
    pub fn learned_archive() -> TianyanUri {
        TianyanUri::new(
            ContextNamespace::Agent,
            vec![
                Self::Learned.as_segment().to_string(),
                "archive".to_string(),
            ],
        )
    }
}

/// Memory 命名空间的固定子路径段（uri_mapper 等共用，禁止散落字面量）。
pub mod memory_paths {
    /// 会话快照段（tianyan://memory/sessions/）。
    pub const SESSIONS: &str = "sessions";
    /// 长期记忆段（tianyan://memory/long_term/）。
    pub const LONG_TERM: &str = "long_term";

    /// 运维数据子域（非记忆内容）：面板展示、摘要生成与检索消费一律排除。
    ///
    /// - `events/extraction_state/`：会话提取状态（旧提取管道遗物）；
    /// - `events/evolution_reports/`：演化报告（运维日志）；
    /// - `events/task_states/`：定时任务持久状态（水位线）；
    /// - `archive/`：演化软删除归档。
    pub const OPERATIONAL_SUBDIRS: &[&[&str]] = &[
        &["events", "extraction_state"],
        &["events", "evolution_reports"],
        &["events", "task_states"],
        &["archive"],
    ];

    /// 判断 URI 是否位于记忆命名空间的运维数据子域（非记忆内容）。
    ///
    /// 消费面（记忆面板、摘要任务、语义检索）统一经本判定过滤——
    /// 运维数据只作为系统状态/日志存在，不作为「记忆」被展示或检索。
    pub fn is_operational_path(uri: &super::TianyanUri) -> bool {
        if uri.namespace() != super::ContextNamespace::Memory {
            return false;
        }
        let path: Vec<&str> = uri.path().iter().map(|s| s.as_str()).collect();
        OPERATIONAL_SUBDIRS
            .iter()
            .any(|sub| path.len() >= sub.len() && path.iter().zip(sub.iter()).all(|(a, b)| a == b))
    }
}

/// 用于标识上下文条目的 Tianyan URI。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TianyanUri {
    /// 完整的 URI 字符串
    uri: String,
    /// 解析后的命名空间
    namespace: ContextNamespace,
    /// 命名空间后的路径组件
    path: Vec<String>,
}

impl TianyanUri {
    /// 从组件创建新的 TianyanUri。
    pub fn new(namespace: ContextNamespace, path: Vec<String>) -> Self {
        let path_str = path.join("/");
        let uri = if path.is_empty() {
            format!("{}://{}", TIANYAN_URI_SCHEME, namespace.dir_name())
        } else {
            format!(
                "{}://{}/{}",
                TIANYAN_URI_SCHEME,
                namespace.dir_name(),
                path_str
            )
        };
        Self {
            uri,
            namespace,
            path,
        }
    }

    /// 解析 URI 字符串。
    pub fn parse(uri: &str) -> error::Result<Self> {
        let url = Url::parse(uri)?;

        if url.scheme() != TIANYAN_URI_SCHEME {
            return Err(error::TianyanError::Custom(format!(
                "不支持的 URI 方案 '{}'。预期使用 'tianyan://'",
                url.scheme()
            )));
        }

        let host = url.host_str().ok_or_else(|| {
            error::TianyanError::Custom(format!("无效的 URI：URI 中缺少主机：{}", uri))
        })?;

        let namespace = ContextNamespace::parse(host).ok_or_else(|| {
            error::TianyanError::Custom(format!("无效的 URI：URI 中包含无效命名空间：{}", host))
        })?;

        let path: Vec<String> = url
            .path_segments()
            .map(|segments| {
                segments
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            uri: uri.to_string(),
            namespace,
            path,
        })
    }

    /// 获取 URI 字符串。
    pub fn as_str(&self) -> &str {
        &self.uri
    }

    /// 获取命名空间。
    pub fn namespace(&self) -> ContextNamespace {
        self.namespace
    }

    /// 获取路径组件。
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// 获取父 URI（如果存在）。
    pub fn parent(&self) -> Option<Self> {
        if self.path.is_empty() {
            return None;
        }
        let mut parent_path = self.path.clone();
        parent_path.pop();
        Some(Self::new(self.namespace, parent_path))
    }

    /// 检查此 URI 是否表示根命名空间。
    pub fn is_namespace_root(&self) -> bool {
        self.path.is_empty()
    }

    /// 追加路径组件。
    pub fn append(&self, component: &str) -> Self {
        let mut new_path = self.path.clone();
        new_path.push(component.to_string());
        Self::new(self.namespace, new_path)
    }

    /// 生成向量存储点 ID。
    ///
    /// 点 ID 由 URI 字符串的 hash 确定性生成。
    pub fn to_point_id(&self) -> String {
        let uri_str = self.to_string();

        let mut hasher1 = DefaultHasher::new();
        uri_str.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        (uri_str + "salt").hash(&mut hasher2);
        let hash2 = hasher2.finish();

        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            hash1 as u32,
            (hash1 >> 32) as u16,
            (hash2 & 0x0FFF) as u16 | 0x4000,
            (hash2 >> 16) as u16 & 0x3FFF | 0x8000,
            hash2 >> 32
        )
    }
}

impl std::fmt::Display for TianyanUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.uri)
    }
}

impl From<TianyanUri> for String {
    fn from(uri: TianyanUri) -> String {
        uri.uri
    }
}

impl AsRef<str> for TianyanUri {
    fn as_ref(&self) -> &str {
        &self.uri
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::error::TianyanError;

    #[test]
    fn test_tianyan_uri_parse() {
        let uri = TianyanUri::parse("tianyan://user/profile/basic_info").unwrap();
        assert_eq!(uri.namespace(), ContextNamespace::User);
        assert_eq!(uri.path(), &["profile", "basic_info"]);
    }

    #[test]
    fn test_tianyan_uri_parse_all_namespaces() {
        let user_uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        assert_eq!(user_uri.namespace(), ContextNamespace::User);

        let session_uri = TianyanUri::parse("tianyan://session/2024-01-15-abc").unwrap();
        assert_eq!(session_uri.namespace(), ContextNamespace::Session);

        let memory_uri = TianyanUri::parse("tianyan://memory/sessions/2024/01").unwrap();
        assert_eq!(memory_uri.namespace(), ContextNamespace::Memory);

        let knowledge_uri = TianyanUri::parse("tianyan://knowledge/documents/api_spec").unwrap();
        assert_eq!(knowledge_uri.namespace(), ContextNamespace::Knowledge);

        let agent_uri = TianyanUri::parse("tianyan://agent/skills/file_ops").unwrap();
        assert_eq!(agent_uri.namespace(), ContextNamespace::Agent);

        let skill_uri = TianyanUri::parse("tianyan://skill/file-ops").unwrap();
        assert_eq!(skill_uri.namespace(), ContextNamespace::Skill);
    }

    #[test]
    fn test_tianyan_uri_parse_root_namespace() {
        let uri = TianyanUri::parse("tianyan://user").unwrap();
        assert_eq!(uri.namespace(), ContextNamespace::User);
        assert!(uri.path().is_empty());
        assert!(uri.is_namespace_root());
    }

    #[test]
    fn test_tianyan_uri_parse_deep_path() {
        let uri = TianyanUri::parse("tianyan://memory/sessions/2024/01/15/session_001").unwrap();
        assert_eq!(uri.namespace(), ContextNamespace::Memory);
        assert_eq!(uri.path(), &["sessions", "2024", "01", "15", "session_001"]);
    }

    #[test]
    fn test_tianyan_uri_parse_invalid_scheme() {
        let result = TianyanUri::parse("http://user/profile");
        assert!(result.is_err());
        if let Err(TianyanError::Custom(msg)) = result {
            assert!(msg.contains("不支持的 URI 方案"));
            assert!(msg.contains("http"));
        } else {
            panic!("预期 Custom 错误（不支持的 URI 方案）");
        }
    }

    #[test]
    fn test_tianyan_uri_parse_invalid_category() {
        let result = TianyanUri::parse("tianyan://invalid_category/test");
        assert!(result.is_err());
    }

    #[test]
    fn test_tianyan_uri_parse_empty() {
        let result = TianyanUri::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn test_tianyan_uri_parent() {
        let uri = TianyanUri::parse("tianyan://user/profile/basic_info").unwrap();
        let parent = uri.parent().unwrap();
        assert_eq!(parent.as_str(), "tianyan://user/profile");

        let root_uri = TianyanUri::parse("tianyan://user").unwrap();
        assert!(root_uri.parent().is_none());
    }

    #[test]
    fn test_tianyan_uri_append() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let new_uri = uri.append("basic_info");
        assert_eq!(new_uri.as_str(), "tianyan://user/profile/basic_info");
    }

    #[test]
    fn test_tianyan_uri_new() {
        let uri = TianyanUri::new(
            ContextNamespace::Knowledge,
            vec!["documents".to_string(), "test".to_string()],
        );
        assert_eq!(uri.namespace(), ContextNamespace::Knowledge);
        assert_eq!(uri.path(), &["documents", "test"]);
        assert_eq!(uri.as_str(), "tianyan://knowledge/documents/test");
    }

    #[test]
    fn test_tianyan_uri_new_empty_path() {
        let uri = TianyanUri::new(ContextNamespace::User, vec![]);
        assert!(uri.is_namespace_root());
        assert_eq!(uri.as_str(), "tianyan://user");
    }

    #[test]
    fn test_tianyan_uri_display() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let display = format!("{}", uri);
        assert_eq!(display, "tianyan://user/profile");
    }

    #[test]
    fn test_tianyan_uri_into_string() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let s: String = uri.into();
        assert_eq!(s, "tianyan://user/profile");
    }

    #[test]
    fn test_tianyan_uri_as_ref() {
        let uri = TianyanUri::parse("tianyan://user/profile").unwrap();
        let s: &str = uri.as_ref();
        assert_eq!(s, "tianyan://user/profile");
    }

    // ── memory_paths::is_operational_path ────────────────────────────

    #[test]
    fn test_is_operational_path_matches_subdirs() {
        let cases = [
            // 四个运维子域（含其深层子路径）均命中
            ("tianyan://memory/events/extraction_state/session-1", true),
            (
                "tianyan://memory/events/evolution_reports/20260913-061422.md",
                true,
            ),
            ("tianyan://memory/events/task_states/evolution.md", true),
            ("tianyan://memory/archive/old-entry", true),
            ("tianyan://memory/events/extraction_state", true),
            // 真记忆内容：不得命中
            ("tianyan://memory/cases/failed_tasks/1786957130", false),
            ("tianyan://memory/facts/evo-1789280062", false),
            ("tianyan://memory/events/decisions/evo-1789280062", false),
            ("tianyan://memory/clipboard/123", false),
            // 相似前缀但不同域：不得命中（防前缀误伤）
            ("tianyan://memory/events/extraction_state_notes", false),
            ("tianyan://memory/archive_notes/x", false),
            // 非记忆命名空间：不适用（返回 false）
            ("tianyan://agent/learned/rule-1", false),
            ("tianyan://skill/learned/tianyan-release-build", false),
        ];
        for (uri_str, expected) in cases {
            let uri = TianyanUri::parse(uri_str).unwrap();
            assert_eq!(
                memory_paths::is_operational_path(&uri),
                expected,
                "uri={uri_str}"
            );
        }
    }
}
