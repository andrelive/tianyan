//! 天演智能体系统的会话类型定义。
//!
//! 本模块定义会话相关的核心数据结构。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::types::{InjectableContext, StructuredMessage, TianyanUri};

/// 会话 JSONL 首行的会话级状态头部。
///
/// 与消息行区分：`session_header` 标记恒为 1；解析时据此识别头部行
/// （StructuredMessage 不含该字段，天然互斥）。旧格式会话（首行即消息）
/// 兼容加载，仅在下次重写时补齐头部。
///
/// 承载两类会话级状态：
/// - `injectable_snapshot`（soul/rules/memories 前缀快照）：前缀内容随会话
///   固化——重启后旧会话沿用同一份快照，不重新检索，保证前缀内容与重启前
///   一致（prompt 缓存不失效、语义不漂移）；仅在会话首次加载（无快照）与
///   压缩点（会话转换）更新。
/// - 会话元数据（`created_at` / `title` / `ended_at` / `working_directory`）：
///   会话身份字段的唯一持久化 home（此前写入 VFS custom metadata 只写不读，
///   重启即丢）。新增字段均为 `Option`（serde 缺省为 None），旧头部与新头部
///   双向兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    /// 头部标记（恒为 1；用于与消息行区分）。
    pub session_header: Option<u8>,
    /// 注入上下文快照（首次加载时固化，压缩点刷新）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injectable_snapshot: Option<InjectableContext>,
    /// 会话创建时间（RFC3339；旧会话缺省时回退加载时刻）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// 会话标题（重启后恢复；旧会话缺省为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 会话结束时间（已结束会话重启后恢复；缺省为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// 会话绑定的工作目录（工作区归属：会话的父级分组；缺省 None = 使用
    /// 全局 [agent] working_directory 配置兜底）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

impl Default for SessionHeader {
    fn default() -> Self {
        Self {
            session_header: Some(Self::MARKER),
            injectable_snapshot: None,
            created_at: None,
            title: None,
            ended_at: None,
            working_directory: None,
        }
    }
}

impl SessionHeader {
    /// 头部标记值。
    pub const MARKER: u8 = 1;

    /// 判断该 JSONL 行是否为会话头部行。
    pub fn parse_line(line: &str) -> Option<Self> {
        serde_json::from_str::<SessionHeader>(line)
            .ok()
            .filter(|h| h.session_header == Some(Self::MARKER))
    }

    /// 是否为空头部（无任何快照载荷）。
    pub fn is_empty(&self) -> bool {
        self.injectable_snapshot.is_none()
    }
}

/// 解析会话 JSONL 文本中的消息行。
///
/// 跳过首行 SessionHeader（`SessionHeader::parse_line` 识别的行）与空行，
/// 解析失败的行静默跳过。**JSONL 格式知识（头部行 vs 消息行）的唯一收敛点**——
/// 消费方（会话加载、记忆提取水位线计数、消息溯源）不得自行解析格式内部。
///
/// 语义要点：返回值为「消息」数/列表，不含头部行——记忆提取水位线据此
/// 精确比较，不再出现旧版把头部行计入导致的差一问题。
pub fn parse_message_lines(jsonl: &str) -> Vec<StructuredMessage> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| SessionHeader::parse_line(l).is_none())
        .filter_map(|l| serde_json::from_str::<StructuredMessage>(l.trim()).ok())
        .collect()
}

/// 表示对话或交互的记忆会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 唯一会话标识符。
    pub session_id: String,
    /// 会话创建时间戳。
    pub created_at: DateTime<Utc>,
    /// 会话结束时间戳（如果已结束）。
    pub ended_at: Option<DateTime<Utc>>,
    /// 会话中的消息。
    pub messages: Vec<StructuredMessage>,
    /// 会话摘要（会话结束后生成）。
    pub summary: Option<String>,
    /// 会话标题（可选）。
    pub title: Option<String>,
    /// 会话级状态头部（存注入上下文快照 + 会话元数据；ADR-018 后存 session_meta 表）。
    #[serde(default)]
    pub header: SessionHeader,
    /// 消息数（轻量列表时预填；None = 未加载消息，按需经 get_session 获取）。
    #[serde(default)]
    pub message_count: Option<usize>,
}

impl Session {
    /// 使用给定 ID 创建新会话。
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            created_at: Utc::now(),
            ended_at: None,
            messages: Vec::new(),
            summary: None,
            title: None,
            header: SessionHeader::default(),
            message_count: None,
        }
    }

    /// 向会话添加结构化消息。
    pub fn add_structured_message(&mut self, msg: StructuredMessage) {
        self.messages.push(msg);
    }

    /// 结束会话。
    pub fn end(&mut self) {
        self.ended_at = Some(Utc::now());
    }

    /// 检查会话是否已结束。
    pub fn is_ended(&self) -> bool {
        self.ended_at.is_some()
    }

    /// 获取会话持续时间（秒）。
    pub fn duration_seconds(&self) -> Option<i64> {
        self.ended_at
            .map(|end| (end - self.created_at).num_seconds())
    }

    /// 获取消息数量。
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// 获取此会话的 URI。
    pub fn uri(&self) -> TianyanUri {
        TianyanUri::new(
            crate::common::types::ContextNamespace::Session,
            vec![self.session_id.clone()],
        )
    }

    /// 解析会话生效的工作目录：会话级绑定（header.working_directory）优先——
    /// 绑定的目录必须真实存在，否则视为未绑定；缺省回退 fallback（全局配置）。
    pub fn working_directory(&self, fallback: Option<&Path>) -> Option<PathBuf> {
        if let Some(wd) = self.header.working_directory.as_deref() {
            let p = PathBuf::from(wd);
            if p.is_dir() {
                return Some(p);
            }
        }
        fallback.map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::types::{DetailedTokenUsage, MessageRole, MessageTime, Part, PartTime};

    #[test]
    fn test_session_creation() {
        let session = Session::new("test-session");
        assert_eq!(session.session_id, "test-session");
        assert!(!session.is_ended());
        assert_eq!(session.message_count(), 0);
    }

    #[test]
    fn test_session_messages() {
        let mut session = Session::new("test-session");
        session.add_structured_message(StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: "你好".to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "test-session".to_string(),
            finish: None,
            compression_marker: false,
        });
        assert_eq!(session.message_count(), 1);
    }

    #[test]
    fn test_session_end() {
        let mut session = Session::new("test-session");
        session.end();
        assert!(session.is_ended());
        assert!(session.duration_seconds().is_some());
    }

    #[test]
    fn test_session_uri() {
        let session = Session::new("test-123");
        let uri = session.uri();
        assert_eq!(uri.to_string(), "tianyan://session/test-123");
    }

    /// 构造单条消息（测试辅助）。
    fn make_msg(id: &str) -> StructuredMessage {
        StructuredMessage {
            id: id.to_string(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: "hi".to_string(),
                time: PartTime::default(),
            }],
            tokens: DetailedTokenUsage::default(),
            cost: 0.0,
            model_id: None,
            time: MessageTime::default(),
            session_id: "s".to_string(),
            finish: None,
            compression_marker: false,
        }
    }

    #[test]
    fn test_parse_message_lines_skips_header_and_bad_lines() {
        // 头部行、坏行、空行均不得进入消息列表；返回值为真实消息数。
        let header = serde_json::to_string(&SessionHeader::default()).unwrap();
        let m1 = serde_json::to_string(&make_msg("m1")).unwrap();
        let m2 = serde_json::to_string(&make_msg("m2")).unwrap();
        let jsonl = format!("{header}\n{m1}\n不是 JSON 的坏行\n{m2}\n\n");

        let parsed = parse_message_lines(&jsonl);
        assert_eq!(parsed.len(), 2, "头部行/坏行/空行应被跳过");
        assert_eq!(parsed[0].id, "m1");
        assert_eq!(parsed[1].id, "m2");
    }

    #[test]
    fn test_working_directory_resolution() {
        // 会话绑定优先：绑定的目录存在时返回绑定值
        let dir = std::env::temp_dir();
        let mut session = Session::new("ws-test");
        session.header.working_directory = Some(dir.to_string_lossy().into_owned());
        let resolved = session.working_directory(Some(Path::new("C:/fallback")));
        assert_eq!(resolved, Some(dir.clone()));

        // 绑定目录不存在 → 视为未绑定，回退 fallback
        session.header.working_directory = Some("Z:/no-such-dir-xyz".to_string());
        let resolved = session.working_directory(Some(Path::new("C:/fallback")));
        assert_eq!(resolved, Some(PathBuf::from("C:/fallback")));

        // 未绑定 + 无 fallback → None
        session.header.working_directory = None;
        assert_eq!(session.working_directory(None), None);
    }

    #[test]
    fn test_session_header_working_directory_roundtrip() {
        // 新字段序列化/反序列化双向兼容：缺省不写盘，旧头部可加载
        let header = SessionHeader {
            working_directory: Some("D:/proj".to_string()),
            ..SessionHeader::default()
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("working_directory"), "字段应序列化: {json}");
        let parsed: SessionHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.working_directory.as_deref(), Some("D:/proj"));

        // 缺省值不序列化该字段（旧格式兼容）
        let default_json = serde_json::to_string(&SessionHeader::default()).unwrap();
        assert!(
            !default_json.contains("working_directory"),
            "缺省不应写盘: {default_json}"
        );
        // 旧头部 JSON（无该字段）可解析
        let legacy = r#"{"session_header":1,"created_at":"2026-01-01T00:00:00Z"}"#;
        let parsed: SessionHeader = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.working_directory, None);
    }

    #[test]
    fn test_parse_message_lines_legacy_without_header() {
        // 旧格式（首行即消息，无 SessionHeader）：全部按消息解析
        let m1 = serde_json::to_string(&make_msg("m1")).unwrap();
        let m2 = serde_json::to_string(&make_msg("m2")).unwrap();
        let parsed = parse_message_lines(&format!("{m1}\n{m2}\n"));
        assert_eq!(parsed.len(), 2);
    }
}

/// 回忆窗口默认半径（命中前后各 N 条）。
pub const DEFAULT_WINDOW_RADIUS: i64 = 5;

/// 回忆命中（全文检索结果）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallHit {
    /// 来源会话。
    pub session_id: String,
    /// 消息序号（会话内 0 起始）。
    pub seq: i64,
    /// 消息 ID。
    pub message_id: String,
    /// 消息角色。
    pub role: String,
    /// 文本内容（user/assistant）。
    pub text: String,
    /// 相关度（BM25 分数取负，越大越相关）。
    pub score: f64,
}

/// 回忆窗口消息。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecallMessage {
    /// 来源会话。
    pub session_id: String,
    /// 消息序号。
    pub seq: i64,
    /// 消息 ID。
    pub message_id: String,
    /// 消息角色。
    pub role: String,
    /// 文本内容（user/assistant）。
    pub text: String,
    /// 工具文本（截断要点）。
    pub tool_text: String,
    /// 消息时间（epoch 毫秒）。
    pub ts: i64,
}

/// 会话元数据（轻量列出，不加载消息）。
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// 会话 ID。
    pub session_id: String,
    /// 会话级头部（title/ended_at/injectable 快照）。
    pub header: SessionHeader,
    /// 会话创建时间（epoch 毫秒；缺省 0 = 未知，由调用方兜底）。
    pub created_at: i64,
    /// 消息数（走 (session_id, seq) 索引计数，O(1)）。
    pub message_count: usize,
}
