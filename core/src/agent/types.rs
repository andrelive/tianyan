//! 智能体类型定义。
//!
//! 本模块包含 Agent 使用的核心数据类型和流式事件发送器。

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::common::error::Result;
use crate::common::types::{DetailedTokenUsage, StructuredMessage, TokenUsage};

/// 用于跟踪执行状态的智能体状态。
#[derive(Debug, Clone, Default)]
pub struct AgentState {
    /// 是否已初始化。
    pub initialized: bool,
    /// 已处理的对话数。
    pub conversations_processed: usize,
    /// Token 使用详情（含 input/output/reasoning/cache 明细）。
    pub total_tokens: DetailedTokenUsage,
}

/// 智能体的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// 响应内容。
    pub content: String,
    /// Token 使用情况。
    pub token_usage: TokenUsage,
    /// 处理时间（毫秒）。
    pub processing_time_ms: u64,
}

impl AgentResponse {
    /// 创建简单回答响应。
    ///
    /// - `content` - 回答内容
    /// - returns: AgentResponse 实例
    pub fn simple(content: String) -> Self {
        Self {
            content,
            token_usage: TokenUsage::default(),
            processing_time_ms: 0,
        }
    }

    /// 创建错误响应。
    ///
    /// - `message` - 错误消息
    /// - returns: AgentResponse 实例
    pub fn error(message: String) -> Self {
        Self {
            content: message,
            token_usage: TokenUsage::default(),
            processing_time_ms: 0,
        }
    }
}

/// 技能调用信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCallInfo {
    /// 技能 ID。
    pub skill_id: String,
    /// 是否成功。
    pub success: bool,
    /// 执行时间（毫秒）。
    pub execution_time_ms: u64,
    /// 错误信息。
    pub error: Option<String>,
}

/// 流式响应块的类型。
///
/// 序列化为小写（answer/tool_call/...），与前端 SSE 契约及
/// `ChatStreamEvent.chunk_type` 的类型定义保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamChunkType {
    /// 思考过程。
    Thought,
    /// 工具调用。
    ToolCall,
    /// 观察结果。
    Observation,
    /// 回答内容。
    #[default]
    Answer,
    /// 错误信息。
    Error,
    /// 消息边界（流开始/结束：携带完整 ChatMessage 元数据，与历史加载同构；
    /// 前端据此用服务端消息结构同步本地消息 id/内容）。
    Message,
    /// 用户消息落库确认（ADR-031：乐观渲染的 id 回显——携带前端生成的
    /// `user_message_id` 与落库后的真实 `message_id`，前端比对后把本地
    /// 乐观消息替换为真实 id；确认后 user_message_id 生命周期结束）。
    UserMessageId,
    /// 轮状态（ADR-035 §9，U10 根治）：轮开始/结束时下发；前端据此联动
    /// 输入框与停止按钮（auto 轮 = 唤醒轮/子代理轮，可中止）。
    TurnState,
    /// 重试提示（T1 重试可见性）：上游请求失败/空响应而重试时下发——
    /// 前端据此显示"正在重试"（避免数百秒静默被当作卡死）。
    /// 只作提示，**不进入消息正文**。
    Retry,
}

/// 流式响应块。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStreamChunk {
    /// 增量内容。
    pub delta: String,
    /// 是否完成。
    pub is_complete: bool,
    /// Token 使用情况。
    pub token_usage: Option<TokenUsage>,
    /// 块类型。
    #[serde(default)]
    pub chunk_type: StreamChunkType,
    /// 技能调用信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_calls: Option<Vec<SkillCallInfo>>,
    /// 完成原因（finish_reason；来自模型响应的最终 choice/chunk）。
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// 结构化工具调用信息（A2 展示契约；ToolCall chunk 携带，供前端渲染 card）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallEvent>,
    /// 结构化工具结果信息（Observation chunk 携带；前端在工具卡片上
    /// 显示耗时/成败）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultEvent>,
    /// 消息边界载体（Message chunk：入库完成后的完整结构化消息；服务端
    /// 据此构造边界事件，前端同步本地消息 id/内容——由入库侧发送保证
    /// 时序正确，pump 侧不再猜测"最后一条消息"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<StructuredMessage>,
    /// 用户消息落库确认（UserMessageId chunk，ADR-031）：前端生成的临时
    /// id（乐观渲染定位键）——确认后生命周期结束，不持久化。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    /// 用户消息落库后的真实 id（UserMessageId chunk 携带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// 轮状态载荷（TurnState chunk 携带，ADR-035 §9）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_state: Option<TurnStateEvent>,
}

/// 轮状态事件（ADR-035 §9）。
///
/// 语义：`running` = 该会话有一个轮（用户轮或 auto 轮）开始执行；
/// `idle` = 轮结束（正常/取消/失败均发）。前端用 `auto` 区分是否可中止的
/// 自动轮（唤醒轮/子代理轮）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStateEvent {
    /// `"running"` | `"idle"`。
    pub state: String,
    /// 是否自动轮（唤醒轮/子代理轮；false = 用户轮）。
    pub auto: bool,
}

/// 工具执行结果事件（Observation chunk 携带）。
///
/// 工具执行完成后随观察结果透传：耗时/成败/错误结构化下发，
/// 前端在对应工具卡片上实时显示（消息级计时元数据的流式通道）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultEvent {
    /// 对应的工具调用 ID（关联 ToolCall 事件）。
    pub tool_call_id: String,
    /// 执行耗时（毫秒）。
    pub duration_ms: i64,
    /// 是否成功。
    pub success: bool,
    /// 失败原因（成功时为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 工具执行结果内容（Observation 的正文 delta 在转发层被丢弃，
    /// 结果内容由此字段透传，前端挂到对应工具卡片）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// 工具调用事件（A2 展示契约：工具自描述 UI 渲染意图）。
///
/// 随 [`StreamChunkType::ToolCall`] chunk 透传：工具名 + 参数 + 展示意图。
/// 前端按 `presentation` 渲染 card（read/terminal/diff/search/web/...），
/// 工具与 UI 解耦（DSH presentCall 吸收）。
/// `id` 为工具调用 ID（对应 `ToolResultEvent.tool_call_id`）：流式卡片
/// 据此关联执行结果事件（耗时/成败/结果内容），否则结果永远匹配不上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEvent {
    /// 工具调用 ID（关联 ToolResultEvent.tool_call_id）。
    pub id: String,
    /// 工具名称。
    pub name: String,
    /// 参数 JSON 字符串（原始 arguments）。
    pub arguments: String,
    /// 展示意图（generic/read/write/terminal/diff/search/web/skill/knowledge/delegate/code）。
    pub presentation: String,
}

/// 流式事件发送器（用于在Planner-Executor循环中实时推送事件）。
#[derive(Clone)]
pub struct StreamEventSender {
    tx: mpsc::Sender<Result<AgentStreamChunk>>,
}

impl StreamEventSender {
    /// 创建流式事件发送器。
    pub fn new(tx: mpsc::Sender<Result<AgentStreamChunk>>) -> Self {
        Self { tx }
    }

    /// 尽力发送事件；接收端已关闭（正常结束场景）时记录 debug 日志。
    async fn try_send(&self, chunk: AgentStreamChunk) {
        if let Err(e) = self.tx.send(Ok(chunk)).await {
            tracing::debug!(
                error = %e,
                closed = self.tx.is_closed(),
                "流式事件发送失败：接收端已关闭"
            );
        }
    }

    /// 发送思考开始事件。
    pub async fn send_thought(&self, content: &str) {
        self.try_send(AgentStreamChunk {
            delta: content.to_string(),
            chunk_type: StreamChunkType::Thought,
            ..Default::default()
        })
        .await;
    }

    /// 发送工具调用事件（A2：携带结构化 tool_call 信息供前端渲染 card）。
    pub async fn send_tool_call(&self, description: &str, tool_call: Option<ToolCallEvent>) {
        self.try_send(AgentStreamChunk {
            delta: description.to_string(),
            chunk_type: StreamChunkType::ToolCall,
            tool_call,
            ..Default::default()
        })
        .await;
    }

    /// 发送观察结果事件。
    pub async fn send_observation(&self, content: &str) {
        self.try_send(AgentStreamChunk {
            delta: content.to_string(),
            chunk_type: StreamChunkType::Observation,
            ..Default::default()
        })
        .await;
    }

    /// 发送带执行元数据的观察结果事件（工具结果 + 耗时/成败）。
    pub async fn send_tool_result(
        &self,
        content: &str,
        tool_call_id: &str,
        duration_ms: i64,
        success: bool,
        error: Option<String>,
    ) {
        self.try_send(AgentStreamChunk {
            delta: content.to_string(),
            chunk_type: StreamChunkType::Observation,
            tool_result: Some(ToolResultEvent {
                tool_call_id: tool_call_id.to_string(),
                duration_ms,
                success,
                error,
                content: Some(content.to_string()),
            }),
            ..Default::default()
        })
        .await;
    }

    /// 发送回答片段事件。
    pub async fn send_answer_delta(&self, delta: &str) {
        self.try_send(AgentStreamChunk {
            delta: delta.to_string(),
            chunk_type: StreamChunkType::Answer,
            ..Default::default()
        })
        .await;
    }

    /// 发送最终完成事件。
    pub async fn send_complete(
        &self,
        content: &str,
        chunk_type: StreamChunkType,
        skill_calls: Option<Vec<SkillCallInfo>>,
        finish_reason: Option<String>,
        token_usage: Option<TokenUsage>,
    ) {
        self.try_send(AgentStreamChunk {
            delta: content.to_string(),
            is_complete: true,
            token_usage,
            chunk_type,
            skill_calls,
            finish_reason,
            ..Default::default()
        })
        .await;
    }

    /// 发送单轮 LLM 调用用量事件（工具轮专用）。
    ///
    /// 最终轮由 [`send_complete`] 携带 usage（`last_turn_usage`），工具轮
    /// （调用工具后继续循环）此前不下发——前端本地消息只有最终轮带 usage，
    /// 会话消耗汇总（`sumSessionUsage`）漏计所有中间轮输入（每轮都是完整
    /// 上下文重发，O(n²) 量级）。本事件 delta 为空、`is_complete=false`，
    /// 前端归约器只消费 usage 字段（附加到最后一条 assistant 消息），
    /// 不产生正文/边界副作用。
    pub async fn send_turn_usage(&self, usage: &TokenUsage) {
        self.try_send(AgentStreamChunk {
            delta: String::new(),
            chunk_type: StreamChunkType::Answer,
            token_usage: Some(usage.clone()),
            ..Default::default()
        })
        .await;
    }

    /// 发送消息边界事件（入库完成后的完整结构化消息）。
    ///
    /// 由持久化侧（run_agent_turn）在用户消息入库后立即发送，保证边界
    /// 事件与入库时序一致——服务端 pump 不再读取"最后一条消息"猜测
    /// 当前轮归属（第二轮竞态：用户消息未入库时误发上一轮 assistant
    /// 边界，导致前端把上一轮输出合并进本轮占位）。
    pub async fn send_message_boundary(&self, message: StructuredMessage) {
        self.try_send(AgentStreamChunk {
            delta: String::new(),
            chunk_type: StreamChunkType::Message,
            message: Some(message),
            ..Default::default()
        })
        .await;
    }

    /// 发送用户消息落库确认事件（ADR-031：乐观渲染的 id 回显）。
    ///
    /// 由持久化侧（run_agent_turn）在用户消息入库后立即发送——携带前端
    /// 生成的 `user_message_id`（请求侧临时 id）与落库后的真实 `message_id`，
    /// 前端比对后把本地乐观消息替换为真实 id。确认后 user_message_id
    /// 生命周期结束（不持久化、不参与回退/对齐）。
    pub async fn send_user_message_id(&self, user_message_id: &str, message_id: &str) {
        self.try_send(AgentStreamChunk {
            delta: String::new(),
            chunk_type: StreamChunkType::UserMessageId,
            user_message_id: Some(user_message_id.to_string()),
            message_id: Some(message_id.to_string()),
            ..Default::default()
        })
        .await;
    }

    /// 发送错误事件。
    pub async fn send_error(&self, error: &str) {
        self.try_send(AgentStreamChunk {
            delta: error.to_string(),
            is_complete: true,
            chunk_type: StreamChunkType::Error,
            ..Default::default()
        })
        .await;
    }

    /// 发送轮状态事件（ADR-035 §9，U10：轮开始/结束；前端联动输入框与停止按钮）。
    pub async fn send_turn_state(&self, running: bool, auto: bool) {
        self.try_send(AgentStreamChunk {
            chunk_type: StreamChunkType::TurnState,
            turn_state: Some(TurnStateEvent {
                state: if running { "running" } else { "idle" }.to_string(),
                auto,
            }),
            ..Default::default()
        })
        .await;
    }

    /// 发送重试提示事件（T1 重试可见性）：上游失败/空响应重试时下发，
    /// 前端显示"正在重试"——避免重试/退避期间数百秒静默被误判为卡死。
    /// 仅作提示，不进入消息正文。
    pub async fn send_retry(&self, note: &str) {
        self.try_send(AgentStreamChunk {
            delta: note.to_string(),
            chunk_type: StreamChunkType::Retry,
            ..Default::default()
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AgentState ──────────────────────────────────────────────

    #[test]
    fn agent_state_default() {
        let state = AgentState::default();
        assert!(!state.initialized, "initialized 默认应为 false");
        assert_eq!(state.conversations_processed, 0);
        assert_eq!(state.total_tokens.total, 0);
    }

    // ── StreamChunkType ─────────────────────────────────────────

    #[test]
    fn stream_chunk_type_default_is_answer() {
        assert_eq!(StreamChunkType::default(), StreamChunkType::Answer);
    }

    #[test]
    fn stream_chunk_type_serde_roundtrip() {
        let variants = [
            StreamChunkType::Thought,
            StreamChunkType::ToolCall,
            StreamChunkType::Observation,
            StreamChunkType::Answer,
            StreamChunkType::Error,
            StreamChunkType::Message,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let deserialized: StreamChunkType = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, variant, "序列化/反序列化不匹配: {json}");
        }
    }

    #[test]
    fn stream_chunk_type_serialized_names() {
        // 小写契约：与前端 SSE 事件类型定义（'answer' | 'tool_call' | ...）一致
        assert_eq!(
            serde_json::to_value(StreamChunkType::Thought).unwrap(),
            serde_json::json!("thought")
        );
        assert_eq!(
            serde_json::to_value(StreamChunkType::ToolCall).unwrap(),
            serde_json::json!("tool_call")
        );
        assert_eq!(
            serde_json::to_value(StreamChunkType::Answer).unwrap(),
            serde_json::json!("answer")
        );
    }

    // ── AgentStreamChunk ────────────────────────────────────────

    #[test]
    fn agent_stream_chunk_creation_answer() {
        let chunk = AgentStreamChunk {
            turn_state: None,
            delta: "你好".to_string(),
            is_complete: false,
            token_usage: None,
            chunk_type: StreamChunkType::Answer,
            skill_calls: None,
            finish_reason: None,
            tool_call: None,
            tool_result: None,
            message: None,
            user_message_id: None,
            message_id: None,
        };
        assert_eq!(chunk.delta, "你好");
        assert!(!chunk.is_complete);
        assert!(chunk.token_usage.is_none());
        assert_eq!(chunk.chunk_type, StreamChunkType::Answer);
        assert!(chunk.skill_calls.is_none());
    }

    #[test]
    fn agent_stream_chunk_with_skill_calls() {
        let calls = vec![SkillCallInfo {
            skill_id: "test_skill".to_string(),
            success: true,
            execution_time_ms: 42,
            error: None,
        }];
        let chunk = AgentStreamChunk {
            turn_state: None,
            delta: "调用技能".to_string(),
            is_complete: true,
            token_usage: Some(TokenUsage::new(10, 20)),
            chunk_type: StreamChunkType::ToolCall,
            skill_calls: Some(calls.clone()),
            finish_reason: None,
            tool_call: None,
            tool_result: None,
            message: None,
            user_message_id: None,
            message_id: None,
        };
        assert_eq!(chunk.delta, "调用技能");
        assert!(chunk.is_complete);
        assert_eq!(chunk.token_usage.unwrap().total_tokens, 30);
        assert_eq!(chunk.chunk_type, StreamChunkType::ToolCall);
        assert_eq!(chunk.skill_calls.unwrap()[0].skill_id, "test_skill");
    }

    #[test]
    fn agent_stream_chunk_serde_roundtrip() {
        let chunk = AgentStreamChunk {
            turn_state: None,
            delta: "思考中...".to_string(),
            is_complete: false,
            token_usage: None,
            chunk_type: StreamChunkType::Thought,
            skill_calls: None,
            finish_reason: None,
            tool_call: None,
            tool_result: None,
            message: None,
            user_message_id: None,
            message_id: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let deserialized: AgentStreamChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.delta, "思考中...");
        assert!(!deserialized.is_complete);
        assert_eq!(deserialized.chunk_type, StreamChunkType::Thought);
        assert!(deserialized.skill_calls.is_none());
    }

    #[test]
    fn user_message_id_chunk_serde_roundtrip() {
        // ADR-031：确认事件携带前端临时 id + 落库真实 id，序列化为
        // chunk_type=user_message_id（前端 reducer 按此分支处理）
        let chunk = AgentStreamChunk {
            turn_state: None,
            delta: String::new(),
            is_complete: false,
            token_usage: None,
            chunk_type: StreamChunkType::UserMessageId,
            skill_calls: None,
            finish_reason: None,
            tool_call: None,
            tool_result: None,
            message: None,
            user_message_id: Some("umid-abc".to_string()),
            message_id: Some("msg_123".to_string()),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(
            json.contains("\"chunk_type\":\"user_message_id\""),
            "json: {json}"
        );
        assert!(
            json.contains("\"user_message_id\":\"umid-abc\""),
            "json: {json}"
        );
        assert!(json.contains("\"message_id\":\"msg_123\""), "json: {json}");
        let deserialized: AgentStreamChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.chunk_type, StreamChunkType::UserMessageId);
        assert_eq!(deserialized.user_message_id.as_deref(), Some("umid-abc"));
        assert_eq!(deserialized.message_id.as_deref(), Some("msg_123"));
    }

    #[test]
    fn agent_stream_chunk_serde_omits_skill_calls_when_none() {
        let chunk = AgentStreamChunk {
            turn_state: None,
            delta: "no calls".to_string(),
            is_complete: false,
            token_usage: None,
            chunk_type: StreamChunkType::Answer,
            skill_calls: None,
            finish_reason: None,
            tool_call: None,
            tool_result: None,
            message: None,
            user_message_id: None,
            message_id: None,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        // skill_calls 为 None 时不应出现在 JSON 中
        assert!(
            !json.contains("skill_calls"),
            "JSON 不应包含 skill_calls: {json}"
        );
    }

    // ── AgentResponse ───────────────────────────────────────────

    #[test]
    fn agent_response_simple_creation() {
        let resp = AgentResponse::simple("回答内容".to_string());
        assert_eq!(resp.content, "回答内容");
        assert_eq!(resp.token_usage.total_tokens, 0);
        assert_eq!(resp.processing_time_ms, 0);
    }

    #[test]
    fn agent_response_error_creation() {
        let resp = AgentResponse::error("出错了".to_string());
        assert_eq!(resp.content, "出错了");
    }

    #[test]
    fn agent_response_serde_roundtrip() {
        let resp = AgentResponse::simple("测试".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: AgentResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content, "测试");
    }

    // ── SkillCallInfo ───────────────────────────────────────────

    #[test]
    fn skill_call_info_serde_roundtrip() {
        let info = SkillCallInfo {
            skill_id: "file_ops".to_string(),
            success: true,
            execution_time_ms: 123,
            error: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: SkillCallInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.skill_id, "file_ops");
        assert!(deserialized.success);
        assert_eq!(deserialized.execution_time_ms, 123);
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn skill_call_info_with_error() {
        let info = SkillCallInfo {
            skill_id: "fail_skill".to_string(),
            success: false,
            execution_time_ms: 0,
            error: Some("权限不足".to_string()),
        };
        assert!(!info.success);
        assert_eq!(info.error.unwrap(), "权限不足");
    }

    // ── StreamEventSender ────────────────────────────────────────

    #[tokio::test]
    async fn stream_event_sender_send_answer_delta() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender.send_answer_delta("hello").await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "hello");
        assert!(!msg.is_complete);
        assert_eq!(msg.chunk_type, StreamChunkType::Answer);
    }

    #[tokio::test]
    async fn stream_event_sender_send_thought() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender.send_thought("thinking...").await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "thinking...");
        assert_eq!(msg.chunk_type, StreamChunkType::Thought);
    }

    #[tokio::test]
    async fn stream_event_sender_send_tool_call() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender.send_tool_call("calling tool", None).await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "calling tool");
        assert_eq!(msg.chunk_type, StreamChunkType::ToolCall);
        assert!(msg.tool_call.is_none());

        // A2：结构化 tool_call 事件透传
        sender
            .send_tool_call(
                "调用: read_file",
                Some(ToolCallEvent {
                    id: "call_test".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"a.txt"}"#.into(),
                    presentation: "read".into(),
                }),
            )
            .await;
        let msg = rx.recv().await.unwrap().unwrap();
        let event = msg.tool_call.unwrap();
        assert_eq!(event.name, "read_file");
        assert_eq!(event.presentation, "read");
    }

    #[tokio::test]
    async fn stream_event_sender_send_observation() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender.send_observation("observed").await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "observed");
        assert_eq!(msg.chunk_type, StreamChunkType::Observation);
    }

    #[tokio::test]
    async fn stream_event_sender_send_error() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender.send_error("error!").await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "error!");
        assert!(msg.is_complete);
        assert_eq!(msg.chunk_type, StreamChunkType::Error);
    }

    #[tokio::test]
    async fn stream_event_sender_send_complete() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        let calls = Some(vec![SkillCallInfo {
            skill_id: "s1".to_string(),
            success: true,
            execution_time_ms: 10,
            error: None,
        }]);

        sender
            .send_complete("finished", StreamChunkType::Answer, calls, None, None)
            .await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert_eq!(msg.delta, "finished");
        assert!(msg.is_complete);
        assert_eq!(msg.chunk_type, StreamChunkType::Answer);
        assert!(msg.skill_calls.is_some());
        assert_eq!(msg.finish_reason, None);
    }

    /// 完成事件携带 finish_reason（模型最终 chunk 的结束原因）。
    #[tokio::test]
    async fn stream_event_sender_send_complete_carries_finish_reason() {
        let (tx, mut rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);

        sender
            .send_complete(
                "done",
                StreamChunkType::Answer,
                None,
                Some("stop".to_string()),
                None,
            )
            .await;
        let msg = rx.recv().await.unwrap().unwrap();
        assert!(msg.is_complete);
        assert_eq!(msg.finish_reason.as_deref(), Some("stop"));
    }

    /// 旧 JSON（无 finish_reason 字段）反序列化兼容：缺省为 None。
    #[test]
    fn agent_stream_chunk_serde_missing_finish_reason_defaults_none() {
        let json =
            r#"{"delta":"旧数据","is_complete":true,"token_usage":null,"chunk_type":"answer"}"#;
        let chunk: AgentStreamChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.is_complete);
        assert_eq!(chunk.finish_reason, None);
        // 反序列化结果可再次序列化（roundtrip 一致性）
        let json2 = serde_json::to_string(&chunk).unwrap();
        let restored: AgentStreamChunk = serde_json::from_str(&json2).unwrap();
        assert_eq!(restored.finish_reason, None);
    }

    #[tokio::test]
    async fn stream_event_sender_dropped_receiver() {
        let (tx, rx) = mpsc::channel(8);
        let sender = StreamEventSender::new(tx);
        // 丢弃接收端
        drop(rx);

        // send 不应 panic（忽略发送错误）
        sender.send_answer_delta("test").await;
        // 测试通过即可
    }
}
