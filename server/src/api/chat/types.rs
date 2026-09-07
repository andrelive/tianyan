use serde::{Deserialize, Serialize};

use crate::api::shared::types::ChatMessage;

/// 对话完成请求
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// 会话标识
    pub session_id: Option<String>,
    /// 本轮输入消息（单条）。
    ///
    /// 历史由服务端会话持久化（VFS JSONL）提供——请求只携带本轮输入，
    /// 服务端以会话文件为准组装上下文（防篡改、与本地 UI 状态解耦）。
    pub message: ChatMessage,
    #[serde(default)]
    /// 是否启用流式响应
    pub stream: bool,
    #[serde(default)]
    /// 生成温度
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    /// 最大生成令牌数
    pub max_tokens: u32,
    /// 指定使用的模型。未提供时使用配置中的默认模型。
    #[serde(default)]
    pub model: Option<String>,
    /// 本会话思考强度档位（会话时选择；None 时使用模型默认）。
    /// 值为当前模型声明的档位（如 "low"/"high"/"max"），"off" 表示关闭；
    /// 仅对支持思考的模型生效，与全局配置无关。
    #[serde(default)]
    pub thinking: Option<String>,
    /// 新会话绑定的工作目录（工作区归属：会话的父级分组）。
    /// 仅新建会话时生效；已存在会话忽略此字段。
    #[serde(default)]
    pub working_directory: Option<String>,
}

fn default_max_tokens() -> u32 {
    2048
}

impl ChatRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref id) = self.session_id {
            if id.trim().is_empty() {
                return Err("session_id 不能为空".to_string());
            }
        }
        // 内容允许为空——当且仅当该消息携带图片（多模态消息以图片为主体）。
        if self.message.content.as_deref().unwrap_or_default().trim().is_empty()
            && self
                .message
                .images
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            return Err("消息内容不能为空".to_string());
        }
        if self
            .message
            .images
            .as_deref()
            .map(|imgs| imgs.iter().any(|u| !u.starts_with("data:")))
            .unwrap_or(false)
        {
            return Err("图片必须为 data URL（data:image/...;base64,...）".to_string());
        }
        if !(0.0..=2.0).contains(&self.temperature) {
            return Err("temperature 必须在 0.0 到 2.0 之间".to_string());
        }
        if self.max_tokens == 0 || self.max_tokens > 32_768 {
            return Err("max_tokens 必须在 1 到 32768 之间".to_string());
        }
        Ok(())
    }
}

/// 追问回答提交请求（ask_user 同步工具：回答提交到等待通道，工具执行恢复）。
#[derive(Debug, Deserialize)]
pub struct AnswerRequest {
    /// 会话标识（等待通道按会话索引，必填）
    pub session_id: String,
    /// 用户回答（JSON 值：`{answers: [{question, answer}], extra}`——
    /// 作为 ask_user 工具结果返回给模型）。
    pub answers: serde_json::Value,
}

/// 技能调用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCallInfo {
    /// 技能标识
    pub skill_id: String,
    /// 技能名称
    pub skill_name: String,
    /// 是否执行成功
    pub success: bool,
    /// 执行耗时（毫秒）
    pub execution_time_ms: u64,
    /// 错误信息
    pub error: Option<String>,
}

/// 流式事件携带的 token 用量（完成 chunk 附带；提供商未返回时缺省）。
///
/// 展示语义（对齐 DSH ContextMeter）：上下文占用 = prompt_tokens（含缓存命中部分），
/// 缓存命中率 = cache_read / prompt_tokens；context_window 供前端计算占用百分比。
#[derive(Debug, Clone, Serialize)]
pub struct StreamUsage {
    /// 提示词 token 数（含缓存命中部分）。
    pub prompt_tokens: u64,
    /// 完成 token 数。
    pub completion_tokens: u64,
    /// 总 token 数。
    pub total_tokens: u64,
    /// 缓存命中（读取）token 数（提供商返回缓存明细时才有意义，否则为 0）。
    pub cache_read: u64,
    /// 缓存写入 token 数（提供商一般不下发，保持 0）。
    pub cache_write: u64,
    /// 当前聊天模型上下文窗口（token；规格解析失败时取默认 32K）。
    pub context_window: u64,
}

/// SSE 流式事件
#[derive(Debug, Serialize)]
pub struct ChatStreamEvent {
    /// 事件标识
    pub id: String,
    /// 会话标识
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 消息边界载荷（chunk_type=message）：流开始携带用户消息、流结束携带
    /// assistant 消息的完整结构（与历史加载 ChatMessage 同构）——前端本地
    /// 消息 id/内容直接来自服务端统一结构，不做两套形态的补丁同步。
    pub message: Option<ChatMessage>,
    /// 增量内容
    pub delta: String,
    /// 结束原因
    pub finish_reason: Option<String>,
    /// 数据块类型
    pub chunk_type: tianyan::agent::StreamChunkType,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 思考过程增量（Thought chunk 携带；正文走 delta，前端分开渲染）
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 技能调用列表
    pub skill_calls: Option<Vec<SkillCallInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 结构化工具调用信息（A2 展示契约；前端据此渲染 tool card）
    pub tool_call: Option<tianyan::agent::ToolCallEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 结构化工具结果信息（Observation chunk 携带；耗时/成败 + 结果内容，
    /// 前端实时挂到对应工具卡片）。
    pub tool_result: Option<tianyan::agent::ToolResultEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 本轮 token 用量（完成 chunk 携带；上下文占用 / 缓存命中展示用）。
    pub usage: Option<StreamUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 用户消息落库确认（chunk_type=user_message_id，ADR-031）：前端生成的
    /// 临时 id（乐观渲染定位键）——确认后生命周期结束，不持久化。
    pub user_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 用户消息落库后的真实 id（user_message_id 确认事件携带）。
    pub message_id: Option<String>,
}

impl ChatStreamEvent {
    /// 错误事件（校验失败/处理失败）：chunk_type=Error，delta 为人类可读消息，
    /// finish_reason=error。前端据此清理占位消息并提示；session_id 为空串时
    /// 由前端按流归属会话处理。
    pub fn error(stream_id: &str, session_id: &str, delta: impl Into<String>) -> Self {
        Self {
            id: stream_id.to_string(),
            session_id: session_id.to_string(),
            message: None,
            delta: delta.into(),
            finish_reason: Some("error".to_string()),
            chunk_type: tianyan::agent::StreamChunkType::Error,
            thinking: None,
            skill_calls: None,
            tool_call: None,
            tool_result: None,
            usage: None,
            user_message_id: None,
            message_id: None,
        }
    }

    /// 消息边界事件（chunk_type=Message）：流开始携带用户消息、流结束携带
    /// assistant 消息的完整结构——前端本地消息 id/内容直接来自服务端统一结构，
    /// 不做两套形态的补丁同步。
    pub fn message_boundary(stream_id: &str, session_id: &str, message: ChatMessage) -> Self {
        Self {
            id: stream_id.to_string(),
            session_id: session_id.to_string(),
            message: Some(message),
            delta: String::new(),
            finish_reason: None,
            chunk_type: tianyan::agent::StreamChunkType::Message,
            thinking: None,
            skill_calls: None,
            tool_call: None,
            tool_result: None,
            usage: None,
            user_message_id: None,
            message_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_deserialization() {
        let json = r#"{
            "session_id": "test-session",
            "message": {"role": "user", "content": "Hello"},
            "stream": false,
            "temperature": 0.7,
            "max_tokens": 100
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.session_id, Some("test-session".to_string()));
        assert_eq!(req.message.content.as_deref(), Some("Hello"));
        assert!(!req.stream);
    }

    #[test]
    fn test_chat_request_parses_user_message_id() {
        // ADR-031：乐观渲染定位键随请求透传（落库后经确认事件回显真实 id）
        let json = r#"{
            "session_id": "test-session",
            "message": {"role": "user", "content": "Hello", "user_message_id": "umid-xyz"},
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 100
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message.user_message_id.as_deref(), Some("umid-xyz"));
        // 无 user_message_id 的请求（旧客户端/非乐观路径）解析为 None
        let plain: ChatRequest = serde_json::from_str(
            r#"{"message": {"role": "user", "content": "Hi"}, "stream": true, "temperature": 0.7, "max_tokens": 100}"#,
        )
        .unwrap();
        assert!(plain.message.user_message_id.is_none());
    }

    #[test]
    fn test_answer_request_deserialization() {
        // 正常请求：session_id + answers（JSON 值）
        let json = r#"{
            "session_id": "session-123",
            "answers": {"answers": [{"question": "Q?", "answer": "A"}]}
        }"#;
        let req: AnswerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.session_id, "session-123");
        assert_eq!(req.answers["answers"][0]["answer"], "A");

        // 缺少 answers：反序列化失败
        let missing = r#"{"session_id": "session-123"}"#;
        assert!(serde_json::from_str::<AnswerRequest>(missing).is_err());
    }

    #[test]
    fn test_chat_stream_event_serialization() {
        let event = ChatStreamEvent {
            id: "chatcmpl-1".to_string(),
            session_id: "session-123".to_string(),
            message: None,
            delta: "Hello".to_string(),
            thinking: None,
            finish_reason: None,
            chunk_type: tianyan::agent::StreamChunkType::Answer,
            skill_calls: None,
            tool_call: None,
            tool_result: None,
            usage: None,
            user_message_id: None,
            message_id: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("chatcmpl-1"));
    }

    fn request_with_message(message: ChatMessage) -> ChatRequest {
        ChatRequest {
            session_id: Some("s1".to_string()),
            message,
            stream: false,
            temperature: 0.7,
            max_tokens: 100,
            model: None,
            thinking: None,
            working_directory: None,
        }
    }

    #[test]
    fn test_validate_plain_message_ok() {
        let req = request_with_message(ChatMessage::user("hello"));
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_image_only_message_ok() {
        // 内容为空但携带图片：多模态消息以图为主体，应通过
        let mut msg = ChatMessage::user("");
        msg.images = Some(vec!["data:image/png;base64,AAAA".to_string()]);
        let req = request_with_message(msg);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_content_without_images_rejected() {
        let req = request_with_message(ChatMessage::user("  "));
        assert!(req.validate().is_err(), "无图片时空内容应被拒绝");
    }

    #[test]
    fn test_validate_non_data_url_image_rejected() {
        let mut msg = ChatMessage::user("图");
        msg.images = Some(vec!["https://example.com/x.png".to_string()]);
        let req = request_with_message(msg);
        assert!(req.validate().is_err(), "非 data URL 图片应被拒绝");
    }

    #[test]
    fn test_chat_message_images_roundtrip() {
        let json = r#"{"role":"user","content":"","images":["data:image/png;base64,AAAA"]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg.images.as_deref(),
            Some(&["data:image/png;base64,AAAA".to_string()][..])
        );
        // 序列化应包含 images 字段
        let out = serde_json::to_string(&msg).unwrap();
        assert!(out.contains("images"));
    }
}


