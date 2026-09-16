use serde::{Deserialize, Serialize};

use crate::api::shared::types::ChatMessage;

/// 会话压缩响应。
#[derive(Debug, Serialize)]
pub struct CompressSessionResponse {
    /// 是否实际发生了压缩（消息不足 / token 未超阈值时为 false）。
    pub compressed: bool,
    /// 压缩生成的摘要消息（前端追加到消息流末尾；压缩未发生时省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<ChatMessage>,
}

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 会话标识
    pub id: String,
    /// 会话标题
    pub title: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 消息数量
    pub message_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 会话绑定的工作目录（工作区归属；None = 使用全局配置兜底）
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 会话元数据
    pub metadata: Option<SessionMetadata>,
}

impl Session {
    /// 从核心会话构造 API 会话（列表/更新标题/更新工作区共用同一转换）。
    pub(crate) fn from_core(s: &tianyan::session::Session) -> Self {
        Self {
            id: s.session_id.clone(),
            title: s.title.clone().unwrap_or_else(|| "新对话".to_string()),
            created_at: s.created_at.to_rfc3339(),
            // 最后对话时间优先（会话不"结束"——ADR-027 时序链模型，
            // ended_at 恒为 None，此前回退 created_at 导致列表显示创建时间）；
            // 无消息时回退创建时间。
            updated_at: s
                .last_message_at
                .or(s.ended_at)
                .unwrap_or(s.created_at)
                .to_rfc3339(),
            message_count: s.message_count.unwrap_or(s.messages.len()) as u32,
            working_directory: s.header.working_directory.clone(),
            metadata: Some(SessionMetadata {
                model: None,
                tags: None,
            }),
        }
    }
}

/// 会话元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 使用的模型名称
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 标签列表
    pub tags: Option<Vec<String>>,
}

/// 列出会话响应
#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    /// 会话列表
    pub sessions: Vec<Session>,
    /// 会话总数
    pub total: usize,
}

/// 会话消息响应
#[derive(Debug, Serialize)]
pub struct SessionMessagesResponse {
    /// 会话标识
    pub session_id: String,
    /// 消息列表
    pub messages: Vec<ChatMessage>,
    /// 上滚游标（ADR-035 §8）：还有更早历史时 = 本页最早一条的 seq；否则 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before_seq: Option<i64>,
    /// 是否还有更早历史（等价于 `next_before_seq.is_some()`，便于前端直读）。
    pub has_more: bool,
    /// 会话链尾 seq（分段对齐基准；无消息为 -1）。
    pub last_seq: i64,
}

/// 会话消息查询参数（ADR-035 §8 分段加载；省略则按全量/最近一页语义由服务决定）。
#[derive(Debug, Clone, Deserialize)]
pub struct SessionMessagesQuery {
    /// 上滚游标：取 seq 严格小于该值的最近一页（省略 = 最近一页）。
    pub before_seq: Option<i64>,
    /// 单页条数（默认 50，上限 200）。
    pub limit: Option<usize>,
}

/// 会话详情响应
#[derive(Debug, Serialize)]
pub struct SessionDetail {
    /// 会话标识
    pub id: String,
    /// 会话标题
    pub title: String,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
    /// 消息列表
    pub messages: Vec<ChatMessage>,
}

/// 删除会话响应
#[derive(Debug, Serialize)]
pub struct DeleteSessionResponse {
    /// 是否成功
    pub success: bool,
    /// 响应消息
    pub message: String,
}

/// 更新标题请求
#[derive(Debug, Deserialize)]
pub struct UpdateTitleRequest {
    /// 新标题
    pub title: String,
}

impl UpdateTitleRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        let trimmed = self.title.trim();
        if trimmed.is_empty() {
            return Err("标题不能为空".to_string());
        }
        if trimmed.len() > 100 {
            return Err("标题长度不能超过 100 个字符".to_string());
        }
        Ok(())
    }
}

/// 更新会话工作目录请求（工作区归属；空串 = 清除绑定）。
#[derive(Debug, Deserialize)]
pub struct UpdateWorkspaceRequest {
    /// 新工作目录绝对路径（空串清除绑定）。
    pub working_directory: String,
}

impl UpdateWorkspaceRequest {
    /// 验证请求参数。
    pub fn validate(&self) -> Result<(), String> {
        if self.working_directory.trim().len() > 4096 {
            return Err("工作目录路径过长".to_string());
        }
        Ok(())
    }
}

/// 删除消息请求 —— 删除指定消息及其后的所有消息。
///
/// 按消息 ID 定位（前端展示列表经合并/过滤后与服务端消息列表索引错位，
/// 数字索引不可靠；ID 是两端共享的稳定键）。
#[derive(Debug, Deserialize)]
pub struct DeleteMessageRequest {
    /// 要删除的消息 ID（该消息及其后的消息都会被删除）。
    pub message_id: String,
}

/// 重做请求 —— 恢复被回退的消息与工作区文件。
#[derive(Debug, Deserialize)]
pub struct RedoRequest {
    /// 回退时被删除的消息 ID（重做数据的定位键）。
    pub message_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_serialization() {
        let session = Session {
            id: "test-123".to_string(),
            title: "测试会话".to_string(),
            created_at: "2026-02-20T10:00:00Z".to_string(),
            updated_at: "2026-02-20T10:30:00Z".to_string(),
            message_count: 5,
            working_directory: None,
            metadata: None,
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("test-123"));
        assert!(json.contains("测试会话"));
    }
}
