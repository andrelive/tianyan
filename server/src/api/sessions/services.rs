use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;

use tianyan::common::types::Part;
use tianyan::session::SessionManager;

use crate::api::sessions::types::{
    DeleteMessageRequest, DeleteSessionResponse, ListSessionsResponse, RedoRequest, Session,
    SessionDetail, SessionMessagesResponse, UpdateTitleRequest,
};
use crate::api::shared::error::ApiError;
use crate::api::shared::types::{ChatMessage, TokenUsage, ToolCallWithResult};
use tianyan::common::types::{MessageRole, StructuredMessage};
use tianyan::snapshot::SnapshotManager;

/// 会话服务，管理对话会话
pub struct SessionService {
    session_manager: Arc<dyn SessionManager>,
    /// 工作区快照管理器（配置了 working_directory 时启用）。
    snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 全局默认工作目录（[agent] working_directory；会话级绑定缺省时的快照根）。
    default_working_dir: Option<PathBuf>,
}

impl SessionService {
    /// 创建新的会话服务
    pub fn new(
        session_manager: Arc<dyn SessionManager>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
        default_working_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            session_manager,
            snapshot_manager,
            default_working_dir,
        }
    }

    /// 解析会话生效的工作目录（会话级绑定优先，缺省回退全局配置）。
    fn resolve_workdir(&self, session: &tianyan::session::Session) -> Option<PathBuf> {
        session.working_directory(self.default_working_dir.as_deref())
    }

    /// 列出所有会话
    pub async fn list_sessions(&self) -> Result<ListSessionsResponse, ApiError> {
        // 从会话管理器获取所有会话
        let core_sessions = self.session_manager.list_sessions().await?;

        // 将核心 Session 转换为 API Session
        let sessions: Vec<Session> = core_sessions.iter().map(Session::from_core).collect();

        let total = sessions.len();

        Ok(ListSessionsResponse { sessions, total })
    }

    /// 获取会话详情
    pub async fn get_session_detail(&self, session_id: &str) -> Result<SessionDetail, ApiError> {
        info!("获取会话详情: {}", session_id);

        let core_session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        // 跨消息收集工具结果：JSONL 中工具结果位于独立的 role=tool 消息
        // （工具执行后单独持久化），按 tool_call_id 上挂到对应调用卡片，
        // 实现"调用 + 结果"合并渲染。
        // 元数据：耗时由 Part::ToolResult.time（start/end 毫秒）差值推导，
        // 失败原因经 error 字段透传（历史数据缺失时均为 None，前端不显示）。
        #[derive(Clone)]
        struct ToolResultMeta {
            content: String,
            duration_ms: Option<i64>,
            error: Option<String>,
        }
        let mut result_map: std::collections::HashMap<String, ToolResultMeta> =
            std::collections::HashMap::new();
        for m in &core_session.messages {
            for p in &m.parts {
                if let Part::ToolResult {
                    tool_call_id,
                    content,
                    error,
                    time,
                } = p
                {
                    let duration_ms = if time.end > 0 && time.start > 0 {
                        Some((time.end - time.start).max(0))
                    } else {
                        None
                    };
                    result_map.insert(
                        tool_call_id.clone(),
                        ToolResultMeta {
                            content: content.clone(),
                            duration_ms,
                            error: error.clone(),
                        },
                    );
                }
            }
        }

        let messages = core_session
            .messages
            .into_iter()
            .filter_map(|m| {
                // 结构化转换（与流式 A2 展示契约对齐，前端时间线渲染）：
                // - Part::Text → content（正文）
                // - Part::Reasoning → thinking（可折叠思考块）
                // - Part::ToolCall + Part::ToolResult → tool_calls（工具卡片，
                //   调用信息与对应执行结果按 tool_call_id 合并渲染）
                // 工具结果/参数不拼进正文，也不做展示层截断——LLM 上下文
                // 组装走原始 StructuredMessage（JSONL），与本展示转换无关。
                // 正文/思考/图片合并（与流式边界事件同一语义：extract_parts）
                let (content, thinking, images) = ChatMessage::extract_parts(&m.parts);
                let mut tool_calls: Vec<ToolCallWithResult> = Vec::new();
                for p in &m.parts {
                    match p {
                        Part::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            // 结果已跨消息收集：此处取出挂到卡片上
                            // （含耗时/失败元数据，前端卡片显示 ✓/✗ + 耗时）
                            let meta = result_map.remove(&id.clone());
                            tool_calls.push(ToolCallWithResult {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                presentation: tianyan::agent::ToolRegistry::default_presentation(
                                    name,
                                )
                                .as_str()
                                .to_string(),
                                result: meta.as_ref().map(|m| m.content.clone()),
                                duration_ms: meta.as_ref().and_then(|m| m.duration_ms),
                                error: meta.as_ref().and_then(|m| m.error.clone()),
                            });
                        }
                        Part::ToolResult {
                            tool_call_id,
                            content: result_content,
                            error,
                            time,
                        } => {
                            // remove 返回 None：结果已被前置调用卡片消费（已合并
                            // 进调用卡片）→ 跳过；返回 Some：结果仍在 map 中
                            // （无对应调用，孤立结果）→ 输出结果-only 卡片。
                            if result_map.remove(tool_call_id).is_some() {
                                let duration_ms = if time.end > 0 && time.start > 0 {
                                    Some((time.end - time.start).max(0))
                                } else {
                                    None
                                };
                                tool_calls.push(ToolCallWithResult {
                                    id: tool_call_id.clone(),
                                    name: String::new(),
                                    arguments: String::new(),
                                    presentation: "generic".to_string(),
                                    result: Some(result_content.clone()),
                                    duration_ms,
                                    error: error.clone(),
                                });
                            }
                        }
                        // 图片不进文本拼接（前端经 images 字段渲染）
                        Part::Image { .. } => {}
                        // 正文/思考已由 extract_parts 合并，此处跳过
                        Part::Text { .. } | Part::Reasoning { .. } => {}
                    }
                }
                // 工具结果被合并进调用卡片后，原 role=tool 消息无任何可展示
                // 内容 → 跳过该消息（避免空气泡）；孤立结果消息保留。
                // 注意：assistant 空消息（唤醒轮空输出等）**必须保留**——
                // 前端唤醒轮询以"出现新的 assistant 消息"为停止信号，
                // 过滤掉会导致轮询永不停止（指示器卡死）。渲染层
                // MessageBubble 已跳过空消息，保留不影响展示。
                if content.is_empty()
                    && thinking.is_empty()
                    && tool_calls.is_empty()
                    && images.is_empty()
                    && m.role == MessageRole::Tool
                {
                    return None;
                }
                // 历史消息携带持久化 usage（DetailedTokenUsage → API TokenUsage）：
                // 前端按会话独立计算上下文占用 / 缓存命中，避免跨会话串值。
                let usage = (m.tokens.total > 0 || m.tokens.input > 0).then_some(TokenUsage {
                    prompt_tokens: m.tokens.input as u32,
                    completion_tokens: m.tokens.output as u32,
                    total_tokens: m.tokens.total as u32,
                    cache_read: m.tokens.cache.read as u32,
                    cache_write: m.tokens.cache.write as u32,
                });
                Some(ChatMessage {
                    // 消息 ID：回退/重做的定位键（前端按 ID 调删除/恢复）
                    id: Some(m.id),
                    // Tool 角色在 API 层映射为 Assistant（与旧 core_bridge 转换一致），
                    // 工具结果已合并进 tool_calls.result，前端不消费 tool 角色。
                    role: match m.role {
                        MessageRole::Tool => MessageRole::Assistant,
                        role => role,
                    },
                    // 纯文本视图已移除（Option 仅请求方向承载）：正文在
                    // segments 的 Text 段（单一事实源）；上方 content 变量仅
                    // 用于空消息（tool 消息）跳过判断。
                    user_message_id: None,
                    content: None,
                    thinking: if thinking.is_empty() {
                        None
                    } else {
                        Some(thinking)
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    images: if images.is_empty() {
                        None
                    } else {
                        Some(images)
                    },
                    // finish=length（token 上限或流式中断）：历史加载与流式
                    // 渲染一致地显示截断提示。
                    truncated_by_length: m.finish.as_deref() == Some("length"),
                    interrupted: m.finish.as_deref() == Some("interrupted"),
                    usage,
                    // 压缩摘要标记：历史加载与压缩响应一致（前端识别摘要消息）
                    compression_marker: m.compression_marker,
                    timestamp: None,
                    // 时间线（方案 B）：parts 顺序 = 真实到达顺序——历史与
                    // 流式共用同一渲染管线（前端 SegmentBlocks）
                    segments: ChatMessage::segments_from_parts(&m.role, &m.parts),
                })
            })
            .collect();

        Ok(SessionDetail {
            id: core_session.session_id,
            title: core_session.title.unwrap_or_else(|| "新对话".to_string()),
            created_at: core_session.created_at.to_rfc3339(),
            updated_at: core_session
                .last_message_at
                .or(core_session.ended_at)
                .unwrap_or(core_session.created_at)
                .to_rfc3339(),
            messages,
        })
    }

    /// 获取会话消息
    pub async fn get_messages(
        &self,
        session_id: &str,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!("获取会话消息: {}", session_id);

        let detail = self.get_session_detail(session_id).await?;

        Ok(SessionMessagesResponse {
            session_id: detail.id,
            messages: detail.messages,
        })
    }

    /// 删除消息及其后的所有消息（与编辑/重新生成一致的截断语义）。
    ///
    /// 按消息 ID 定位原始索引后截断——前端展示列表经合并/过滤（tool/system
    /// 消息）后索引与服务端消息列表错位，数字索引会删过头；ID 是稳定定位键。
    pub async fn delete_message(
        &self,
        session_id: &str,
        request: DeleteMessageRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        // 消息 ID → 服务端原始列表索引（快照/截断仍按索引，恢复语义不变）
        let message_index = session
            .messages
            .iter()
            .position(|m| m.id == request.message_id)
            .ok_or_else(|| ApiError::NotFound(format!("消息不存在: {}", request.message_id)))?;
        info!(
            "删除消息: 会话={}, 消息={}, 索引={}",
            session_id, request.message_id, message_index
        );

        // 保存重做状态：当前工作区 + 被截断的消息（按消息 ID 组织；
        // 配置了 working_directory 时快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let truncated: Vec<StructuredMessage> = session.messages[message_index..].to_vec();
            if let Some(workdir) = self.resolve_workdir(&session) {
                if let Err(e) = sm
                    .with_workdir(workdir)
                    .save_redo(session_id, &request.message_id, &truncated)
                    .await
                {
                    tracing::warn!(error = %e, "保存重做状态失败");
                }
            }
        }

        // 截断到该消息之前（不含）
        session.messages.truncate(message_index);

        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::error!("更新会话失败: {}", e);
            return Err(e.into());
        }
        if let Err(e) = self
            .session_manager
            .rewrite_messages(session_id, &session.messages)
            .await
        {
            tracing::error!("重写会话历史失败: {}", e);
            return Err(e.into());
        }

        // 回退工作区文件到该消息处理前的快照（配置了 working_directory 时生效；
        // 快照根取会话生效的工作目录）
        if let Some(sm) = &self.snapshot_manager {
            let Some(workdir) = self.resolve_workdir(&session) else {
                return self.get_messages(session_id).await;
            };
            match sm
                .with_workdir(workdir)
                .restore(session_id, message_index)
                .await
            {
                Ok(n) => {
                    info!(
                        "回退工作区: 会话={}, 索引={}, 恢复 {} 个文件",
                        session_id, message_index, n
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = %session_id,
                        index = message_index,
                        "工作区快照恢复失败（会话已回退，文件未回退）"
                    );
                }
            }
        }

        // 返回剩余消息，前端可直接替换本地状态
        self.get_messages(session_id).await
    }

    /// 重做：恢复被回退的消息与工作区文件。
    pub async fn redo_message(
        &self,
        session_id: &str,
        request: RedoRequest,
    ) -> Result<SessionMessagesResponse, ApiError> {
        info!("重做消息: 会话={}, 消息={}", session_id, request.message_id);

        // 0. 存在性检查：不存在的会话 → 404（优先于重做状态检查）；
        //    同时解析当前会话生效的工作目录（快照根，会话级绑定优先）
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;
        let workdir = self
            .resolve_workdir(&session)
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?;

        // 1. 恢复工作区文件 + 取出被截断的消息（快照管理器一次性语义，
        //    重做数据按被回退消息的 ID 组织）
        let Some((messages, restored)) = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| ApiError::BadRequest("未启用工作区快照，无法重做".to_string()))?
            .with_workdir(workdir)
            .load_redo(session_id, &request.message_id)
            .await?
        else {
            return Err(ApiError::BadRequest("没有可重做的状态".to_string()));
        };

        // 2. 恢复会话消息
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        for msg in messages {
            session.add_structured_message(msg);
        }
        if let Err(e) = self.session_manager.update_session(&session).await {
            tracing::error!("更新会话失败: {}", e);
            return Err(e.into());
        }
        if let Err(e) = self
            .session_manager
            .rewrite_messages(session_id, &session.messages)
            .await
        {
            tracing::error!("重写会话历史失败: {}", e);
            return Err(e.into());
        }

        tracing::info!(session = %session_id, restored, "重做完成");
        self.get_messages(session_id).await
    }

    /// 删除会话
    pub async fn delete_session(
        &self,
        session_id: &str,
    ) -> Result<DeleteSessionResponse, ApiError> {
        info!("删除会话: {}", session_id);

        // 存在性检查：不存在的会话删除 → 404（幂等语义，避免 500）
        let exists = self
            .session_manager
            .get_session(session_id)
            .await?
            .is_some();
        if !exists {
            return Err(ApiError::NotFound(format!("会话未找到: {}", session_id)));
        }

        self.session_manager.delete_session(session_id).await?;

        Ok(DeleteSessionResponse {
            success: true,
            message: format!("会话 {} 删除成功", session_id),
        })
    }

    /// 更新会话标题
    pub async fn update_title(
        &self,
        session_id: &str,
        request: UpdateTitleRequest,
    ) -> Result<Session, ApiError> {
        info!("更新会话标题: {} -> {}", session_id, request.title);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        session.title = Some(request.title);

        self.session_manager.update_session(&session).await?;

        Ok(Session::from_core(&session))
    }

    /// 更新会话绑定的工作目录（工作区归属；空串清除绑定）。
    ///
    /// 目录必须存在，否则 400。
    pub async fn update_workspace(
        &self,
        session_id: &str,
        working_directory: &str,
    ) -> Result<Session, ApiError> {
        info!("更新会话工作目录: {} -> {}", session_id, working_directory);

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("会话未找到: {}", session_id)))?;

        let trimmed = working_directory.trim();
        if trimmed.is_empty() {
            session.header.working_directory = None;
        } else {
            if !std::path::Path::new(trimmed).is_dir() {
                return Err(ApiError::BadRequest(format!("目录不存在：{trimmed}")));
            }
            session.header.working_directory = Some(trimmed.to_string());
        }

        self.session_manager.update_session(&session).await?;

        Ok(Session::from_core(&session))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tianyan::common::types::{MessageRole, Part, PartTime, StructuredMessage};
    use tianyan::session::{Session, SessionManager};

    use crate::api::sessions::services::SessionService;

    /// 内存会话管理器 mock（get_session_detail 回归测试用）。
    struct MockSessionManager {
        sessions: Mutex<HashMap<String, Session>>,
    }

    impl MockSessionManager {
        fn with_sessions(sessions: Vec<Session>) -> Self {
            let map = sessions
                .into_iter()
                .map(|s| (s.session_id.clone(), s))
                .collect();
            Self {
                sessions: Mutex::new(map),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionManager for MockSessionManager {
        async fn create_session(
            &self,
            id: &str,
            _message: tianyan::Message,
        ) -> tianyan::Result<Session> {
            let session = Session::new(id);
            self.sessions
                .lock()
                .unwrap()
                .insert(id.to_string(), session.clone());
            Ok(session)
        }

        async fn get_session(&self, id: &str) -> tianyan::Result<Option<Session>> {
            Ok(self.sessions.lock().unwrap().get(id).cloned())
        }

        async fn update_session(&self, session: &Session) -> tianyan::Result<()> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.session_id.clone(), session.clone());
            Ok(())
        }

        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> tianyan::Result<()> {
            Ok(())
        }

        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> tianyan::Result<()> {
            Ok(())
        }

        async fn list_sessions(&self) -> tianyan::Result<Vec<Session>> {
            Ok(self.sessions.lock().unwrap().values().cloned().collect())
        }

        async fn delete_session(&self, id: &str) -> tianyan::Result<()> {
            self.sessions.lock().unwrap().remove(id);
            Ok(())
        }
    }

    #[test]
    fn test_session_service_creation() {
        // 编译时测试
    }

    /// 回归：历史消息结构化转换 —— 工具调用/结果独立字段输出，
    /// 工具结果保持完整内容不截断（多字节 UTF-8 中文内容完整保留，
    /// 且不再有按字节切片导致的 panic，此前会导致 GET /sessions/{id}/messages 500）。
    #[tokio::test]
    async fn test_get_session_detail_structured_tool_parts_with_multibyte_content() {
        let chinese_result = "中文结果".repeat(300); // 900 字符 ≈ 2700 字节
        let chinese_args = "中文参数".repeat(150); // 450 字符 ≈ 1350 字节
        let msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: chinese_args,
                    time: PartTime::default(),
                },
                Part::ToolResult {
                    tool_call_id: "call_1".to_string(),
                    content: chinese_result,
                    error: None,
                    time: PartTime::default(),
                },
            ],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
        );

        // 不 panic；结构化输出：正文不含工具文本，工具调用与结果按 id 合并
        // 进同一张卡片，工具结果保持完整内容（不做展示层截断）
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");
        let msg = &detail.messages[0];
        assert!(
            msg.content.is_none(),
            "响应方向无纯文本字段（正文在 segments）"
        );
        assert!(msg.thinking.is_none(), "无思考 parts 时 thinking 为空");

        let calls = msg.tool_calls.as_ref().expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1, "调用与结果合并为一张卡片");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments.len(), 1800, "参数保持完整不截断");
        assert_eq!(calls[0].presentation, "read", "展示意图来自 core 默认映射");
        let result = calls[0].result.as_ref().expect("结果应合并进调用卡片");
        assert_eq!(result.len(), 3600, "工具结果保持完整内容（不截断）");
        assert!(result.starts_with("中文结果"), "中文内容完整保留");
    }

    /// 孤立工具结果（无对应 ToolCall part）仍以结果-only 卡片保留。
    #[tokio::test]
    async fn test_get_session_detail_keeps_orphan_tool_result() {
        let msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_x".to_string(),
                content: "孤立结果".to_string(),
                error: None,
                time: PartTime::default(),
            }],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");
        let calls = detail.messages[0]
            .tool_calls
            .as_ref()
            .expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_x");
        assert_eq!(calls[0].name, "", "孤立结果无工具名");
        assert_eq!(calls[0].result.as_deref(), Some("孤立结果"));
    }

    /// 跨消息合并：工具结果位于独立的 role=tool 消息（工具执行后单独
    /// 持久化），应上挂到前一条 assistant 消息的调用卡片，且原 tool 消息
    /// 不再产生空气泡。
    #[tokio::test]
    async fn test_get_session_detail_merges_result_across_messages() {
        let call_msg = StructuredMessage {
            id: "msg_1".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![
                Part::Text {
                    text: "先看看目录".to_string(),
                    time: PartTime::default(),
                },
                Part::ToolCall {
                    id: "call_1".to_string(),
                    name: "list_dir".to_string(),
                    arguments: r#"{"path":"."}"#.to_string(),
                    time: PartTime::default(),
                },
            ],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let result_msg = StructuredMessage {
            id: "msg_2".to_string(),
            parent_id: Some("msg_1".to_string()),
            role: MessageRole::Tool,
            parts: vec![Part::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: r#"{"count":2}"#.to_string(),
                error: None,
                time: PartTime::default(),
            }],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(call_msg);
        session.messages.push(result_msg);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");

        // 只有 1 条消息（role=tool 消息被合并消费后跳过）
        assert_eq!(detail.messages.len(), 1, "tool 结果消息应被合并后跳过");
        let msg = &detail.messages[0];
        // 正文在 segments 的 Text 段（响应方向无纯文本字段）
        let text: Vec<&str> = msg
            .segments
            .as_ref()
            .map(|s| {
                s.iter()
                    .filter_map(|x| match x {
                        crate::api::shared::types::MessageSegment::Text { text } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(text, vec!["先看看目录"]);
        let calls = msg.tool_calls.as_ref().expect("应输出 tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].presentation, "search");
        assert_eq!(
            calls[0].result.as_deref(),
            Some(r#"{"count":2}"#),
            "结果应跨消息合并进调用卡片"
        );
    }

    /// 回归：assistant 空消息（唤醒轮空输出）必须保留——前端唤醒轮询以
    /// "出现新的 assistant 消息"为停止信号，过滤掉会导致轮询永不停止
    /// （指示器卡死）。渲染层 MessageBubble 已跳过空消息，保留不影响展示。
    #[tokio::test]
    async fn test_get_session_detail_keeps_empty_assistant() {
        let empty_assistant = StructuredMessage {
            id: "msg_empty".to_string(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![],
            tokens: Default::default(),
            cost: 0.0,
            model_id: None,
            time: Default::default(),
            session_id: "session-test".to_string(),
            finish: None,
            compression_marker: false,
        };
        let mut session = Session::new("session-test");
        session.messages.push(empty_assistant);

        let service = SessionService::new(
            Arc::new(MockSessionManager::with_sessions(vec![session])),
            None,
            None,
        );
        let detail = service
            .get_session_detail("session-test")
            .await
            .expect("get_session_detail 不应失败");

        assert_eq!(
            detail.messages.len(),
            1,
            "assistant 空消息应保留（前端轮询停止信号）"
        );
        assert_eq!(detail.messages[0].role, MessageRole::Assistant);
        assert!(
            detail.messages[0].content.is_none(),
            "空 assistant 消息：无纯文本字段"
        );
    }
}
