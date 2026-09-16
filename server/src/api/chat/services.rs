use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tracing::info;

use tianyan::agent::stream_forward::{
    inject_stream_event_fields, spawn_stream_forwarder, BroadcastJsonDeliver, StreamEventDeliver,
    StreamEventMapper,
};
use tianyan::agent::AgentCoordinator;
use tianyan::common::types::StructuredMessage;
use tianyan::session::SessionManager;
use tianyan::Message as CoreMessage;
use tianyan::MessageRole as CoreMessageRole;

use crate::api::chat::types::{ChatRequest, ChatStreamEvent, SkillCallInfo, StreamUsage};
use crate::api::shared::error::ApiError;
use crate::api::shared::short_uuid;
use crate::api::shared::types::{ChatMessage, MessageRole};
use crate::state::RoleSync;

/// 把 API 层消息转换为 core 消息：携带图片时构造多模态消息。
fn to_core_message(msg: &ChatMessage) -> CoreMessage {
    match &msg.images {
        Some(images) if !images.is_empty() => {
            CoreMessage::user_with_images(msg.content.clone().unwrap_or_default(), images.clone())
        }
        _ => CoreMessage::user(msg.content.clone().unwrap_or_default()),
    }
}

/// 对话服务，处理对话逻辑
pub struct ChatService {
    agent: Arc<dyn AgentCoordinator>,
    session_manager: Arc<dyn SessionManager>,
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
            role_sync: None,
            context_window: tianyan::model::spec::ModelSpec::default().context_length as u64,
        }
    }

    /// 设置聊天模型上下文窗口（由调用方按模型规格解析注入）。
    pub fn with_context_window(mut self, context_window: u64) -> Self {
        self.context_window = context_window;
        self
    }

    /// 挂载角色注册表同步句柄（ADR-016：新会话创建时刷新学习角色）。
    pub fn with_role_sync(mut self, sync: RoleSync) -> Self {
        self.role_sync = Some(sync);
        self
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
    ///
    /// ADR-032：流式接线收敛到 core 统一转发器——本方法创建转发器
    /// （建通道即消费）、把 sender 交给 core 轮、等待流排空后补发 assistant
    /// 消息边界事件。`event_tx` 为统一事件通道（SSE 广播源），与唤醒轮 /
    /// 子代理路径同一送达抽象。
    pub async fn process_message_stream(
        &self,
        request: ChatRequest,
        event_tx: tokio::sync::broadcast::Sender<String>,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), ApiError> {
        let last_text = request.message.content.clone().unwrap_or_default();
        let last_message = to_core_message(&request.message);

        let (session_id, is_new) = resolve_or_create_session(
            self.session_manager.as_ref(),
            request.session_id.as_deref(),
            &last_text,
            request.working_directory.as_deref(),
        )
        .await?;
        if is_new {
            self.refresh_learned_roles().await;
        }

        // 一次流式响应用一个响应 id（与非流式 ChatResponse.id 语义一致）。
        // SSE 事件 id 用于 Last-Event-ID 重连，同一响应流的所有 chunk 共享该 id。
        let stream_id = format!("chatcmpl-{}", short_uuid());

        // ADR-032 统一转发器：通道先建并立即消费，sender 交给 core 轮。
        let mapper = self.chunk_mapper(&stream_id);
        let deliver: Arc<dyn StreamEventDeliver> = Arc::new(BroadcastJsonDeliver::new(event_tx));
        let (sender, forward_handle) =
            spawn_stream_forwarder(session_id.clone(), mapper, deliver.clone());

        // 「终止后显示上一轮结论」修复：记录轮前最后一条 assistant 消息 id。
        //
        // 取消 / 失败轮**不落库** assistant 消息；若边界事件无条件取"最后一条
        // assistant"，就会把上一轮的结论当作本轮结果下发，前端把它合并进本轮
        // 占位气泡 → 陈旧内容显示在当前轮位置。水位用于只认"本轮新产生的"消息。
        let prev_assistant_id = self
            .session_manager
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .and_then(|s| last_assistant_message(&s.messages).map(|m| m.id.clone()));

        let result = self
            .agent
            .process_message_stream(
                &session_id,
                &last_message,
                request.model.as_deref(),
                Some(cancel),
                request.thinking,
                // ADR-031：乐观渲染定位键（落库后经 UserMessageId 确认事件回显）
                request.message.user_message_id.as_deref(),
                sender,
            )
            .await;

        // 轮结束（sender 已释放）→ 等待流排空：尾部事件（完成/usage）不丢。
        let _ = forward_handle.await;
        result?;

        // assistant 消息边界（统一结构）：流结束后取最后一条 assistant 消息，
        // 以完整 ChatMessage 结构下发——前端本地 assistant 消息 id 同步为
        // 服务端 id（与用户消息边界同构；经同一送达目标，排在流事件之后）。
        self.send_assistant_boundary(
            &session_id,
            &stream_id,
            &deliver,
            prev_assistant_id.as_deref(),
        )
        .await?;

        info!("流式处理完成，会话：{}", session_id);

        Ok(())
    }

    /// 构造 chunk → 事件 JSON 映射（统一转发器用；`stream_id` 固定本响应 id）。
    fn chunk_mapper(&self, stream_id: &str) -> StreamEventMapper {
        let stream_id = stream_id.to_string();
        let context_window = self.context_window;
        Arc::new(move |session_id, chunk| {
            serde_json::to_value(map_chunk_to_event(
                chunk,
                &stream_id,
                session_id,
                context_window,
            ))
            .ok()
        })
    }

    /// 下发 assistant 消息边界事件（流结束后，经同一送达目标）。
    ///
    /// 只下发**本轮新产生的** assistant 消息（判据见 [`select_turn_assistant`]）：
    /// 取消 / 失败轮没有新消息时**不下发**——否则会把上一轮的结论当作本轮结果
    /// 推给前端（用户实测：终止后前端渲染出上一轮的回答）。
    ///
    /// `prev_assistant_id` 为轮前最后一条 assistant 消息 id（水位）。
    async fn send_assistant_boundary(
        &self,
        session_id: &str,
        stream_id: &str,
        deliver: &Arc<dyn StreamEventDeliver>,
        prev_assistant_id: Option<&str>,
    ) -> Result<(), ApiError> {
        let Some(am) = self
            .session_manager
            .get_session(session_id)
            .await?
            .and_then(|s| select_turn_assistant(&s.messages, prev_assistant_id).cloned())
        else {
            return Ok(());
        };
        let event = ChatStreamEvent::message_boundary(
            stream_id,
            session_id,
            ChatMessage::from_structured_light(&am),
        );
        let mut payload = serde_json::to_value(&event)
            .map_err(|e| ApiError::Internal(format!("序列化边界事件失败：{e}")))?;
        inject_stream_event_fields(&mut payload, session_id);
        deliver.deliver(session_id, payload).await;
        Ok(())
    }
}

/// 取会话中最后一条 assistant 消息（用作"轮前水位"）。
fn last_assistant_message(messages: &[StructuredMessage]) -> Option<&StructuredMessage> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
}

/// 取**本轮新产生的** assistant 消息（无 → `None`）。
///
/// 判据：从尾部找最后一条 assistant，且其 id 必须与轮前水位不同。
/// 为什么必须这样判：取消 / 失败轮**不落库** assistant 消息，无条件取"最后一条
/// assistant"会把上一轮结论当作本轮结果（前端合并进本轮占位气泡 → 渲染陈旧内容）。
fn select_turn_assistant<'a>(
    messages: &'a [StructuredMessage],
    prev_assistant_id: Option<&str>,
) -> Option<&'a StructuredMessage> {
    last_assistant_message(messages).filter(|m| Some(m.id.as_str()) != prev_assistant_id)
}

/// 核心流式 chunk → API 事件映射（正文只收真正的回答/错误文本）：
/// - Thought → thinking 字段（思考块，折叠渲染）
/// - ToolCall → tool_call 字段（A2 工具卡片），delta 丢弃（"调用: xxx" 不进正文）
/// - Observation → 丢弃（工具结果 JSON 不再淹没正文）
/// - Answer / Error → delta（正文增量）
pub(crate) fn map_chunk_to_event(
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
        // Message chunk（core 入库侧发送）：携带完整结构化消息
        message: chunk
            .message
            .as_ref()
            .map(ChatMessage::from_structured_light),
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
        // ADR-031：用户消息落库确认（乐观渲染的 id 回显）
        user_message_id: chunk.user_message_id,
        message_id: chunk.message_id,
        // ADR-035 §9：轮状态（前端联动输入框与停止按钮；U10）
        turn_state: chunk.turn_state,
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
    /// 回归测试：**取消 / 失败轮不得把上一轮 assistant 结论当作本轮结果**。
    ///
    /// 场景复现：会话里已有上一轮 assistant 消息，本轮被终止且没有落库新消息——
    /// 此时必须返回 None（旧实现会取到上一轮消息 → 前端渲染到本轮位置）。
    #[test]
    fn test_select_turn_assistant_returns_none_when_no_new_message() {
        let old = StructuredMessage::assistant("s1", "上一轮结论");
        let messages = vec![old.clone()];

        assert!(
            select_turn_assistant(&messages, Some(old.id.as_str())).is_none(),
            "无新 assistant 消息时不得回退到上一轮消息"
        );
        assert_eq!(
            last_assistant_message(&messages).map(|m| m.id.clone()),
            Some(old.id.clone()),
            "水位函数应返回最后一条 assistant（无论新旧）"
        );
    }

    /// 本轮落库了新 assistant 消息 → 选中它（而不是上一轮）。
    #[test]
    fn test_select_turn_assistant_picks_new_message() {
        let old = StructuredMessage::assistant("s1", "上一轮结论");
        let new = StructuredMessage::assistant("s1", "本轮结论");
        let messages = vec![old.clone(), new.clone()];

        let picked =
            select_turn_assistant(&messages, Some(old.id.as_str())).expect("应选中本轮新消息");
        assert_eq!(picked.id, new.id, "必须下发本轮消息而非上一轮");
    }

    /// 首轮（会话尚无 assistant 消息，水位 None）→ 选中新消息。
    #[test]
    fn test_select_turn_assistant_first_turn() {
        let new = StructuredMessage::assistant("s1", "首个回答");
        let messages = vec![new.clone()];

        assert!(last_assistant_message(&messages).is_some());
        let picked = select_turn_assistant(&messages, None).expect("首轮应选中新消息");
        assert_eq!(picked.id, new.id);
    }

    /// 无 assistant 消息 → None（不 panic）。
    #[test]
    fn test_select_turn_assistant_empty() {
        assert!(select_turn_assistant(&[], Some("x")).is_none());
        assert!(last_assistant_message(&[]).is_none());
    }
}
