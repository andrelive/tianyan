use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use tianyan::agent::AgentCoordinator;
use tianyan::session::SessionManager;
use tianyan::Message as CoreMessage;
use tianyan::MessageRole as CoreMessageRole;

use crate::api::chat::types::{
    ChatRequest, ChatResponse, ChatStreamEvent, SkillCallInfo, StreamUsage,
};
use crate::api::shared::error::ApiError;
use crate::api::shared::short_uuid;
use crate::api::shared::types::{ChatMessage, MessageRole, TokenUsage};
use crate::state::{RoleSync, SkillSync};

/// 把 API 层消息转换为 core 消息：携带图片时构造多模态消息。
fn to_core_message(msg: &ChatMessage) -> CoreMessage {
    match &msg.images {
        Some(images) if !images.is_empty() => {
            CoreMessage::user_with_images(msg.content.clone(), images.clone())
        }
        _ => CoreMessage::user(msg.content.clone()),
    }
}

/// 对话服务，处理对话逻辑
pub struct ChatService {
    agent: Arc<dyn AgentCoordinator>,
    session_manager: Arc<dyn SessionManager>,
    /// 技能注册表同步句柄：新会话创建时刷新已学习技能（会话边界刷新）。
    skill_sync: Option<SkillSync>,
    /// 角色注册表同步句柄（ADR-016）：新会话创建时刷新学习角色。
    role_sync: Option<RoleSync>,
    /// 当前聊天模型上下文窗口（token），随流式 usage 事件下发供前端计算占用百分比。
    context_window: u64,
}

impl ChatService {
    /// 创建新的对话服务
    pub fn new(agent: Arc<dyn AgentCoordinator>, session_manager: Arc<dyn SessionManager>) -> Self {
        Self {
            agent,
            session_manager,
            skill_sync: None,
            role_sync: None,
            context_window: tianyan::model::spec::ModelSpec::default().context_length as u64,
        }
    }

    /// 设置聊天模型上下文窗口（由调用方按模型规格解析注入）。
    pub fn with_context_window(mut self, context_window: u64) -> Self {
        self.context_window = context_window;
        self
    }

    /// 挂载技能注册表同步句柄（新会话创建时刷新已学习技能）。
    pub fn with_skill_sync(mut self, sync: SkillSync) -> Self {
        self.skill_sync = Some(sync);
        self
    }

    /// 挂载角色注册表同步句柄（ADR-016：新会话创建时刷新学习角色）。
    pub fn with_role_sync(mut self, sync: RoleSync) -> Self {
        self.role_sync = Some(sync);
        self
    }

    /// 会话边界刷新：新会话创建后把 VFS 中已学习技能增量注册进注册表。
    ///
    /// 幂等（仅注册新技能）；失败仅告警，不影响对话。
    async fn refresh_learned_skills(&self) {
        if let Some(sync) = &self.skill_sync {
            if let Err(e) = sync.refresh().await {
                tracing::warn!(error = %e, "新会话技能刷新失败（不影响对话）");
            }
        }
    }

    /// 会话边界刷新（ADR-016）：新会话创建后把 VFS 学习角色增量合并进注册表。
    ///
    /// 幂等；失败仅告警，不影响对话。
    async fn refresh_learned_roles(&self) {
        if let Some(sync) = &self.role_sync {
            if let Err(e) = sync.refresh().await {
                tracing::warn!(error = %e, "新会话角色刷新失败（不影响对话）");
            }
        }
    }

    /// 处理对话消息（流式响应）
    ///
    /// 如果 `session_id` 未提供，会自动创建新会话。
    /// `cancel` 为服务层/传输层共享的取消标志：客户端断开或服务关停时置位，
    /// 使后台 AgentLoop 在轮次边界/流式 chunk 边界及时停止。
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatStreamEvent>,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), ApiError> {
        let last_text = request.message.content.clone();
        let last_message = to_core_message(&request.message);

        let (session_id, is_new) = resolve_or_create_session(
            self.session_manager.as_ref(),
            request.session_id.as_deref(),
            &last_text,
            request.working_directory.as_deref(),
        )
        .await?;
        if is_new {
            self.refresh_learned_skills().await;
            self.refresh_learned_roles().await;
        }

        let mut stream = self
            .agent
            .process_message_stream(
                &session_id,
                &last_message,
                request.model.as_deref(),
                Some(cancel),
                request.thinking,
            )
            .await?;

        // 一次流式响应对应一个响应 id（与非流式 ChatResponse.id 语义一致）。
        // SSE 事件 id 用于 Last-Event-ID 重连，同一响应流的所有 chunk 共享该 id。
        let stream_id = format!("chatcmpl-{}", short_uuid());

        while let Some(chunk_result) = stream.recv().await {
            match chunk_result {
                Ok(chunk) => {
                    let event =
                        map_chunk_to_event(chunk, &stream_id, &session_id, self.context_window);
                    if tx.send(event).await.is_err() {
                        debug!("客户端断开流式连接");
                        break;
                    }
                }
                Err(e) => {
                    error!("流式处理错误: {}", e);
                    let event = ChatStreamEvent {
                        id: stream_id.clone(),
                        session_id: session_id.clone(),
                        delta: format!("错误: {}", e),
                        thinking: None,
                        finish_reason: Some("error".to_string()),
                        chunk_type: tianyan::agent::StreamChunkType::Error,
                        skill_calls: None,
                        tool_call: None,
                        tool_result: None,
                        usage: None,
                    };
                    if tx.send(event).await.is_err() {
                        debug!("客户端已断开，错误事件未送达");
                    }
                    return Err(e.into());
                }
            }
        }

        info!("流式处理完成，会话：{}", session_id);

        Ok(())
    }

    /// 处理用户对追问的回答（流式，SSE）。
    ///
    /// 与非流式 [`Self::handle_clarification`] 共用确认语义；增量经事件通道
    /// 逐块推送（思考/工具/输出可见），避免整轮等待超过 HTTP 超时。
    pub async fn handle_clarification_stream(
        &self,
        session_id: &str,
        answer: &str,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> Result<(), ApiError> {
        let mut stream = self
            .agent
            .handle_clarification_stream(session_id, answer)
            .await?;
        let stream_id = format!("chatcmpl-{}", short_uuid());

        while let Some(chunk_result) = stream.recv().await {
            match chunk_result {
                Ok(chunk) => {
                    let event =
                        map_chunk_to_event(chunk, &stream_id, session_id, self.context_window);
                    if tx.send(event).await.is_err() {
                        debug!("客户端断开流式连接");
                        break;
                    }
                }
                Err(e) => {
                    error!("流式处理错误: {}", e);
                    let event = ChatStreamEvent {
                        id: stream_id.clone(),
                        session_id: session_id.to_string(),
                        delta: format!("错误: {}", e),
                        thinking: None,
                        finish_reason: Some("error".to_string()),
                        chunk_type: tianyan::agent::StreamChunkType::Error,
                        skill_calls: None,
                        tool_call: None,
                        tool_result: None,
                        usage: None,
                    };
                    if tx.send(event).await.is_err() {
                        debug!("客户端已断开，错误事件未送达");
                    }
                    return Err(e.into());
                }
            }
        }

        Ok(())
    }

    /// 处理用户对追问的回答（非流式）。
    ///
    /// 会话必须存在待处理的追问（由 AgentLoop 在 ask_user 工具触发时设置）。
    pub async fn handle_clarification(
        &self,
        session_id: &str,
        answer: &str,
    ) -> Result<ChatResponse, ApiError> {
        let response = self.agent.handle_clarification(session_id, answer).await?;
        Ok(to_chat_response(session_id, response))
    }
}

/// 核心流式 chunk → API 事件映射（正文只收真正的回答/错误文本）：
/// - Thought → thinking 字段（思考块，折叠渲染）
/// - ToolCall → tool_call 字段（A2 工具卡片），delta 丢弃（"调用: xxx" 不进正文）
/// - Observation → 丢弃（工具结果 JSON 不再淹没正文）
/// - Answer / Error → delta（正文增量）
fn map_chunk_to_event(
    chunk: tianyan::agent::AgentStreamChunk,
    stream_id: &str,
    session_id: &str,
    context_window: u64,
) -> ChatStreamEvent {
    let skill_calls = chunk.skill_calls.map(convert_skill_calls);
    let chunk_type = chunk.chunk_type;
    let is_thought = chunk_type == tianyan::agent::StreamChunkType::Thought;
    let is_observational = matches!(
        chunk_type,
        tianyan::agent::StreamChunkType::ToolCall | tianyan::agent::StreamChunkType::Observation
    );
    ChatStreamEvent {
        id: stream_id.to_string(),
        session_id: session_id.to_string(),
        delta: if is_thought || is_observational {
            String::new()
        } else {
            chunk.delta.clone()
        },
        thinking: if is_thought {
            Some(chunk.delta.clone())
        } else {
            None
        },
        finish_reason: if chunk.is_complete {
            map_finish_reason(chunk.finish_reason.clone())
        } else {
            None
        },
        chunk_type,
        skill_calls,
        tool_call: chunk.tool_call,
        tool_result: chunk.tool_result,
        usage: chunk.token_usage.map(|u| StreamUsage {
            prompt_tokens: u.prompt_tokens as u64,
            completion_tokens: u.completion_tokens as u64,
            total_tokens: u.total_tokens as u64,
            cache_read: u.cache_read as u64,
            cache_write: u.cache_write as u64,
            context_window,
        }),
    }
}

/// 完成 chunk 的 finish_reason 映射：透传模型真实 finish_reason（length/tool_calls 等），
/// 无 finish_reason 时回退 "stop"（保持缺省线上行为）。
fn map_finish_reason(finish_reason: Option<String>) -> Option<String> {
    finish_reason.or_else(|| Some("stop".to_string()))
}

/// 将核心 AgentResponse 转换为 API 层 ChatResponse。
fn to_chat_response(session_id: &str, response: tianyan::agent::AgentResponse) -> ChatResponse {
    ChatResponse {
        id: format!("chatcmpl-{}", short_uuid()),
        session_id: session_id.to_string(),
        message: ChatMessage {
            role: MessageRole::Assistant,
            content: response.content,
            thinking: None,
            tool_calls: None,
            images: None,
            truncated_by_length: false,
            usage: Some(TokenUsage {
                prompt_tokens: response.token_usage.prompt_tokens as u32,
                completion_tokens: response.token_usage.completion_tokens as u32,
                total_tokens: response.token_usage.total_tokens as u32,
                cache_read: response.token_usage.cache_read as u32,
                cache_write: response.token_usage.cache_write as u32,
            }),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        },
        usage: TokenUsage {
            prompt_tokens: response.token_usage.prompt_tokens as u32,
            completion_tokens: response.token_usage.completion_tokens as u32,
            total_tokens: response.token_usage.total_tokens as u32,
            cache_read: response.token_usage.cache_read as u32,
            cache_write: response.token_usage.cache_write as u32,
        },
    }
}

/// 将核心 SkillCallInfo 转换为 API 层 SkillCallInfo
fn convert_skill_calls(calls: Vec<tianyan::agent::SkillCallInfo>) -> Vec<SkillCallInfo> {
    calls
        .into_iter()
        .map(|sc| {
            let skill_id = sc.skill_id.clone();
            SkillCallInfo {
                skill_id: skill_id.clone(),
                skill_name: skill_id,
                success: sc.success,
                execution_time_ms: sc.execution_time_ms,
                error: sc.error,
            }
        })
        .collect()
}

/// 解析或创建会话，返回 `(session_id, is_new)`。
///
/// - 已存在会话：复用，`is_new = false`（会话内，不触发技能刷新）
/// - 新会话：创建，`is_new = true`（会话边界，调用方刷新已学习技能）；
///   携带 `working_directory` 时绑定为新会话的工作目录（工作区归属）。
async fn resolve_or_create_session(
    session_manager: &dyn SessionManager,
    session_id: Option<&str>,
    initial_message: &str,
    working_directory: Option<&str>,
) -> Result<(String, bool), ApiError> {
    // 如果传了 session_id 且服务端已存在，直接复用
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if session_manager.get_session(sid).await?.is_some() {
            return Ok((sid.to_string(), false));
        }
    }

    // 新建会话：优先用前端传来的 session_id，否则生成一个
    let new_id = session_id
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("session-{}", short_uuid()));
    let msg = CoreMessage::new(CoreMessageRole::User, initial_message);
    // ? 传播：保留 core 错误语义（不吞成 Internal）
    let mut session = session_manager.create_session(&new_id, msg).await?;

    // 新会话绑定工作目录（工作区归属）：目录必须存在，坏值仅告警不阻断对话
    if let Some(wd) = working_directory.map(str::trim).filter(|w| !w.is_empty()) {
        if std::path::Path::new(wd).is_dir() {
            session.header.working_directory = Some(wd.to_string());
            if let Err(e) = session_manager.update_session(&session).await {
                tracing::warn!(error = %e, working_directory = %wd, "会话工作目录绑定持久化失败");
            }
        } else {
            tracing::warn!(working_directory = %wd, "新会话工作目录不存在，忽略绑定");
        }
    }

    // 用第一条用户消息生成标题（取第一行或前 30 个字符）
    let title = generate_session_title(initial_message);
    if !title.is_empty() {
        session.title = Some(title);
        if let Err(e) = session_manager.update_session(&session).await {
            tracing::warn!(error = %e, "会话标题更新失败");
        }
    }

    // 清空 create_session 预写的消息：会话历史统一由 agent.process_message 追加，
    // 避免同一条用户消息在历史中重复
    session.messages.clear();
    if let Err(e) = session_manager.rewrite_messages(&new_id, &[]).await {
        tracing::warn!(error = %e, "清空会话预写消息失败");
    }

    Ok((new_id, true))
}

/// 从用户消息中提取会话标题。
/// 优先取第一行（到第一个换行符），超过 30 字则截断并加 "…"。
fn generate_session_title(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return String::new();
    }
    if first_line.chars().count() > 30 {
        let truncated: String = first_line.chars().take(30).chain(['…']).collect();
        return truncated;
    }
    first_line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_service_creation() {
        // 这是一个编译时测试，确保 ChatService 可以被构造
        // 实际测试需要 mock 实现
    }

    #[test]
    fn test_map_finish_reason_passthrough_real_reason() {
        // 模型真实 finish_reason（如 length/tool_calls）应原样透传
        let fr = map_finish_reason(Some("length".to_string()));
        assert_eq!(fr.as_deref(), Some("length"));
    }

    #[test]
    fn test_map_finish_reason_defaults_to_stop() {
        // 无 finish_reason 时回退 "stop"（保持缺省线上行为）
        let fr = map_finish_reason(None);
        assert_eq!(fr.as_deref(), Some("stop"));
    }
}
