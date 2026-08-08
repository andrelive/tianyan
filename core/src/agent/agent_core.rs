//! 智能体核心实现。
//!
//! 包含 Agent 结构体定义、构造函数和所有辅助方法。
//! 将 Agent 与 AgentCoordinator trait 分离，消除循环依赖。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::tool_registry::DynamicToolExecutor;
use crate::agent::types::{AgentResponse, AgentState, ClarificationQuestion, QuestionType};
use crate::common::error::Result;
use crate::common::types::{Message, MessageRole, StructuredMessage, TokenUsage};
use crate::context::{ContextAssembler, ContextPipeline};
use crate::observability::AgentMetrics;
use crate::observability::TokenRecord;
use crate::session::{Session, SessionManager};
use crate::skills::SkillLearningEngine;
use crate::snapshot::SnapshotManager;

/// compression_marker 之后至少积累多少条消息才触发压缩。
const MIN_MESSAGES_BEFORE_COMPRESSION: usize = 6;

/// 智能体协调器的默认实现。
#[derive(Clone)]
pub struct Agent {
    /// 默认对话模型名（配置中指定，未被请求级 model 覆盖时使用）。
    pub(crate) default_model: String,
    pub(crate) context_pipeline: ContextPipeline,
    pub(crate) metrics: Arc<AgentMetrics>,
    pub(crate) skill_learning_engine: Option<SkillLearningEngine>,
    pub(crate) state: Arc<RwLock<AgentState>>,
    pub(crate) agent_loop: AgentLoop,
    pub(crate) session_manager: Arc<dyn SessionManager>,
    /// 工作区快照管理器（配置了 working_directory 时启用，用于会话回退恢复文件）。
    pub(crate) snapshot_manager: Option<Arc<SnapshotManager>>,
}

impl Agent {
    /// 创建新的 Agent 实例。
    ///
    /// 此构造函数由 AgentBuilder::build() 调用，外部应通过 Builder 创建 Agent。
    pub(crate) fn new(
        default_model: String,
        context_pipeline: ContextPipeline,
        metrics: Arc<AgentMetrics>,
        skill_learning_engine: Option<SkillLearningEngine>,
        agent_loop: AgentLoop,
        session_manager: Arc<dyn SessionManager>,
        snapshot_manager: Option<Arc<SnapshotManager>>,
    ) -> Self {
        Self {
            default_model,
            context_pipeline,
            metrics,
            skill_learning_engine,
            state: Arc::new(RwLock::new(AgentState::default())),
            agent_loop,
            session_manager,
            snapshot_manager,
        }
    }

    /// 注册动态工具（如 MCP 工具桥接）。
    ///
    /// 由 server 层在 Agent 构建后注入：配置中的 MCP 服务器工具经
    /// 桥接注册后，对 LLM 可见并可执行（与内置工具同等地位）。
    pub async fn register_dynamic_tools(&self, tools: Vec<Arc<dyn DynamicToolExecutor>>) {
        for tool in tools {
            self.agent_loop
                .tool_registry()
                .register_dynamic_tool(tool)
                .await;
        }
    }

    /// 捕获工作区快照（消息处理前调用，索引 = 当前消息数）。
    pub(crate) async fn capture_workspace_snapshot(&self, session_id: &str, state: &SessionState) {
        let Some(sm) = &self.snapshot_manager else {
            return;
        };
        let index = state.structured_messages.len();
        if let Err(e) = sm.capture(session_id, index).await {
            tracing::warn!(error = %e, session = %session_id, "工作区快照捕获失败");
        }
    }

    /// 从持久化存储加载会话并构建 SessionState。
    pub(crate) async fn load_and_build_state(
        &self,
        session_id: &str,
    ) -> Result<Arc<RwLock<SessionState>>> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .unwrap_or_else(|| Session::new(session_id));

        let state = Arc::new(RwLock::new(SessionState::new(session_id)));
        {
            let mut s = state.write().await;
            for sm in &session.messages {
                s.add_structured_message(sm.clone());
            }
        }
        Ok(state)
    }

    /// 准备当前轮次的上下文消息。
    ///
    /// 执行：写入用户消息 → 检查会话缓存 → 按需加载 injectable → 压缩 → 组装。
    /// soul/rules/memories 仅会话首次加载，后续轮次复用缓存，
    /// 确保 system prompt 前缀稳定以命中 DeepSeek 前缀缓存。
    ///
    /// 图片（`message.content_parts`）随用户消息写入会话状态，经
    /// `ContextAssembler` 组装为多模态传输消息，对 LLM 可见。
    pub(crate) async fn prepare_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        message: &Message,
    ) -> Vec<Message> {
        let query = message.content.clone();
        let images = message.image_urls();
        if images.is_empty() {
            state.write().await.add_user_message(&query);
        } else {
            state
                .write()
                .await
                .add_user_message_with_images(&query, images);
        }

        // 会话级缓存：soul 非空表示已加载过，跳过 I/O
        let soul_loaded = {
            let s = state.read().await;
            !s.injectable_context.soul.is_empty()
        };

        let injectable = if !soul_loaded {
            match self.context_pipeline.load_injectable(&query).await {
                Ok(ctx) => {
                    let injected_rules = ctx.rules_and_experiences.len();
                    if injected_rules > 0 {
                        self.metrics.record_rule_hit(injected_rules).await;
                    }
                    state.write().await.injectable_context = ctx;
                    state.read().await.injectable_context.clone()
                }
                Err(e) => {
                    tracing::warn!(error = %e, "加载可注入上下文失败");
                    self.metrics.record_pipeline_failure().await;
                    state.read().await.injectable_context.clone()
                }
            }
        } else {
            let s = state.read().await;
            s.injectable_context.clone()
        };

        let s = state.read().await;
        let mut messages = ContextAssembler::assemble(&s.structured_messages, &injectable, "");

        // 注入会话定位信息：早期对话被压缩后，摘要字段可能不足以恢复细节，
        // 告知 LLM 当前会话 URI，使其可用 vfs_read 检索被压缩的原始记录。
        // 位置固定在 system 前缀（soul/rules）之后、历史消息之前，
        // 同会话内内容恒定，不影响 DeepSeek 前缀缓存。
        let session_hint = format!(
            "## 会话定位\n当前会话 ID：{}\n若早期对话已被压缩且摘要信息不足，可用 vfs_read 工具读取 tianyan://session/{} 查看原始对话记录（JSONL 格式，含压缩前的完整消息）。",
            s.session_id, s.session_id
        );
        let insert_at = messages
            .iter()
            .position(|m| m.role != MessageRole::System)
            .unwrap_or(messages.len());
        messages.insert(insert_at, Message::system(session_hint));

        messages
    }

    /// 判断是否需要压缩，如果需要则生成摘要 StructuredMessage 并持久化。
    pub(crate) async fn maybe_compress_and_persist(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
    ) {
        let messages_since_marker: Vec<StructuredMessage> = {
            let s = state.read().await;
            let marker_pos = s
                .structured_messages
                .iter()
                .rposition(|m| m.compression_marker);
            let start_idx = marker_pos.unwrap_or(0);
            s.structured_messages[start_idx..].to_vec()
        };

        if messages_since_marker.len() < MIN_MESSAGES_BEFORE_COMPRESSION {
            return;
        }

        let conversation: Vec<Message> = messages_since_marker
            .iter()
            .flat_map(ContextAssembler::structured_to_messages)
            .collect();

        if let Some(summary_sm) = self
            .context_pipeline
            .compress_for_session(&conversation, session_id)
            .await
        {
            if let Err(e) = self
                .session_manager
                .add_structured_message(session_id, summary_sm.clone())
                .await
            {
                tracing::warn!(error = %e, "持久化压缩摘要失败");
            } else {
                state.write().await.add_structured_message(summary_sm);
            }
        }
    }

    /// 共享编排骨架：持久化用户消息 → 准备上下文 → AgentLoop → 结果组装 → 指标更新。
    ///
    /// `process_message` 与 `handle_clarification_response` 共用此骨架，
    /// 差异段（快照、审批确认、压缩、技能学习等）由调用方保留。
    /// - `model` — 本次使用的模型（调用方已解析）。
    /// - `start` — 由调用方传入，保证 `processing_time_ms` 测量窗口与调用前一致。
    /// - `parent_id` — 在 `prepare_context` 之后统一计算（挂接在当前用户消息之下），
    ///   与 process_message 原行为一致（澄清路径的回复链随之修正为 回答→回复）。
    /// - `cancel` — 取消标志（客户端断开/服务关停时置位）；`None` 表示不可取消。
    pub(crate) async fn run_agent_turn(
        &self,
        state: &Arc<RwLock<SessionState>>,
        session_id: &str,
        message: &Message,
        model: &str,
        start: Instant,
        cancel: Option<&AtomicBool>,
    ) -> Result<AgentResponse> {
        // 1. Persist current user message
        self.persist_user_message(session_id, state, message).await;

        // 2. Prepare context
        let messages = self.prepare_context(state, message).await;

        // 3. Run agent loop (internal persistence in AgentLoop)
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let loop_result = self
            .agent_loop
            .run(
                &mut messages.clone(),
                None,
                session_id,
                parent_id.as_deref(),
                model,
                cancel,
            )
            .await;

        // 4. Handle loop result
        let mut loop_tokens: Option<TokenUsage> = None;
        let response = match loop_result {
            Ok(AgentLoopResult::Answer {
                content,
                total_tokens,
                persisted_message,
                ..
            }) => {
                state
                    .write()
                    .await
                    .add_structured_message(*persisted_message);

                let mut resp = AgentResponse::simple(content);
                resp.token_usage = total_tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                self.metrics.record_execution(true).await;
                loop_tokens = Some(total_tokens);
                resp
            }
            Ok(AgentLoopResult::NeedsClarification {
                question,
                total_tokens,
                ..
            }) => {
                let question_obj = ClarificationQuestion {
                    question,
                    question_type: QuestionType::OpenEnded,
                    options: None,
                    required: true,
                };
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                let formatted = format_clarification_questions(std::slice::from_ref(&question_obj));
                let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                resp.token_usage = total_tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                loop_tokens = Some(total_tokens);
                resp
            }
            Ok(AgentLoopResult::Cancelled {
                total_tokens,
                turns,
            }) => {
                tracing::info!(turns, "AgentLoop 被取消");
                self.metrics.record_execution(false).await;
                let mut resp = AgentResponse::simple("任务已取消".to_string());
                resp.cancelled = true;
                resp.token_usage = total_tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                loop_tokens = Some(total_tokens);
                resp
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.metrics.record_execution(false).await;
                let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
        };

        // 5. Update agent metrics
        self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
            .await;

        Ok(response)
    }

    /// 处理用户对追问的回答。
    pub(crate) async fn handle_clarification_response(
        &self,
        state: &Arc<RwLock<SessionState>>,
        clarification_answers: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();

        if state.read().await.pending_clarification.is_none() {
            return Ok(AgentResponse::simple("当前没有待处理的追问".to_string()));
        }
        state.write().await.pending_clarification = None;

        let session_id = state.read().await.session_id.clone();

        // 审批降级链路：若存在待用户确认的审批操作，根据回答记录批准/拒绝。
        // 决策语义由审批模块（executor::approval::is_user_confirmation）解析。
        let approved = crate::executor::approval::is_user_confirmation(clarification_answers);
        if self
            .agent_loop
            .tool_registry()
            .confirm_pending_approval(approved)
            .await
        {
            tracing::info!(
                approved,
                answer = %clarification_answers,
                "已记录用户对审批追问的回应"
            );
        }

        // 共享编排骨架（parent_id 统一在 prepare_context 之后计算，回复挂接在用户回答之下，与 process_message 路径语义一致）
        let clarification_msg = Message::user(clarification_answers);
        self.run_agent_turn(
            state,
            &session_id,
            &clarification_msg,
            &self.default_model,
            start,
            None,
        )
        .await
    }

    /// 从会话执行历史中自动学习新技能（GEPA 进化引擎）。
    pub(crate) async fn learn_skills_from_session(&self, session_id: &str) {
        if let Some(ref engine) = self.skill_learning_engine {
            let history = self
                .agent_loop
                .tool_registry()
                .drain_execution_history()
                .await;
            if history.is_empty() {
                return;
            }
            match engine.learn_from_history(&history).await {
                Ok(skills) => {
                    if !skills.is_empty() {
                        tracing::info!(
                            session_id = %session_id,
                            count = skills.len(),
                            history_len = history.len(),
                            "GEPA 引擎生成新技能"
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!(session_id = %session_id, error = %e, "GEPA 学习跳过（历史不足或未启用）");
                }
            }
        }
    }

    /// 更新全局 Agent 指标（对话计数、token 统计）。
    pub(crate) async fn update_agent_metrics(
        &self,
        session_id: &str,
        loop_tokens: Option<&TokenUsage>,
        success: bool,
    ) {
        let mut st = self.state.write().await;
        st.conversations_processed += 1;
        if let Some(tokens) = loop_tokens {
            st.total_tokens.input += tokens.prompt_tokens;
            st.total_tokens.output += tokens.completion_tokens;
            st.total_tokens.total += tokens.total_tokens;
            self.metrics
                .record_token_usage(TokenRecord {
                    session_id: session_id.to_string(),
                    timestamp: chrono::Utc::now(),
                    total_tokens: tokens.total_tokens,
                    system_prompt_tokens: 0,
                    retrieved_tokens: 0,
                    success,
                })
                .await;
        }
    }

    /// 持久化当前用户消息到 SessionManager。
    pub(crate) async fn persist_user_message(
        &self,
        session_id: &str,
        state: &Arc<RwLock<SessionState>>,
        message: &Message,
    ) {
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let images = message.image_urls();
        let user_msg = if images.is_empty() {
            Message::user(message.content.clone())
        } else {
            Message::user_with_images(message.content.clone(), images)
        };
        let user_sm = ContextAssembler::message_to_structured(
            &user_msg,
            session_id,
            parent_id.as_deref(),
            None,
        );
        if let Err(e) = self
            .session_manager
            .add_structured_message(session_id, user_sm)
            .await
        {
            tracing::warn!(error = %e, "持久化用户消息失败");
        }
    }
}

/// 格式化追问问题为用户友好的文本。
pub(crate) fn format_clarification_questions(questions: &[ClarificationQuestion]) -> String {
    let mut content = String::from("为了更好帮助您，需要以下信息：\n\n");

    for (i, q) in questions.iter().enumerate() {
        content.push_str(&format!("{}. {}\n", i + 1, q.question));

        if let Some(options) = &q.options {
            content.push_str("   可选答案：\n");
            for (j, opt) in options.iter().enumerate() {
                content.push_str(&format!("   {}. {}\n", j + 1, opt));
            }
        }

        if q.required {
            content.push_str("   (必填)\n");
        }

        content.push('\n');
    }

    content.push_str("请提供以上信息，我会根据您的回答继续处理。");
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session_state::SessionState;
    use crate::common::types::{InjectableContext, MessageRole};
    use crate::context::assembler::ContextAssembler;

    // ── 编排路径集成测试（MockChatService + 真实 ContextPipeline/ToolRegistry）──

    use crate::agent::r#loop::AgentLoopConfig;
    use crate::agent::tool_registry::ToolRegistry;
    use crate::agent::AgentCoordinator;
    use crate::common::types::{FunctionCall, ToolCall, ToolCallType};
    use crate::context::compression::{CompressionConfig, ContextCompressor};
    use crate::context::retrieval::DualLayerRetriever;
    use crate::executor::SecurityPolicy;
    use crate::model::types::{ChatChoice, ChatCompletionResponse};
    use crate::model::MockChatService;
    use crate::session::{Session, SessionManager};
    use crate::test_utils::MockVfs;
    use crate::vfs::VirtualFileSystem;
    use async_trait::async_trait;
    use tokio::sync::Mutex as TokioMutex;

    struct MockSessionManager;
    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn add_structured_message(
            &self,
            _session_id: &str,
            _msg: StructuredMessage,
        ) -> Result<()> {
            Ok(())
        }
        async fn add_message(&self, _session_id: &str, _message: Message) -> Result<()> {
            Ok(())
        }
        async fn rewrite_messages(
            &self,
            _session_id: &str,
            _messages: &[StructuredMessage],
        ) -> Result<()> {
            Ok(())
        }
        async fn create_session(&self, _id: &str, _message: Message) -> Result<Session> {
            Ok(Session::new(_id))
        }
        async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn update_session(&self, _session: &Session) -> Result<()> {
            Ok(())
        }
        async fn list_sessions(&self) -> Result<Vec<Session>> {
            Ok(vec![])
        }
        async fn delete_session(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    fn response_with(assistant: Message) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "resp-1".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: assistant,
                finish_reason: Some("stop".to_string()),
            }],
            usage: TokenUsage::default(),
        }
    }

    fn ask_user_msg(question: &str) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "call_ask".to_string(),
                call_type: ToolCallType::Function,
                function: FunctionCall {
                    name: "ask_user".to_string(),
                    arguments: format!(r#"{{"question": "{}"}}"#, question),
                },
            }],
        )
    }

    fn make_agent(mock: MockChatService) -> Agent {
        let vfs = Arc::new(MockVfs::new());
        let retriever = Arc::new(DualLayerRetriever::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>
        ));
        // 压缩服务在短会话测试中不会被调用（< MIN_MESSAGES_BEFORE_COMPRESSION）
        let compressor = Arc::new(TokioMutex::new(ContextCompressor::new(
            Arc::new(MockChatService::new()),
            CompressionConfig::default(),
        )));
        let context_pipeline = ContextPipeline::new(
            vfs.clone() as Arc<dyn VirtualFileSystem>,
            retriever,
            compressor,
            10,
            5,
        );
        let agent_loop = AgentLoop::new(
            Arc::new(mock),
            ToolRegistry::new(SecurityPolicy::default()),
            Arc::new(MockSessionManager),
            AgentLoopConfig { max_turns: 5 },
        );
        Agent::new(
            "test-model".to_string(),
            context_pipeline,
            AgentMetrics::new(),
            None,
            agent_loop,
            Arc::new(MockSessionManager),
            None,
        )
    }

    #[tokio::test]
    async fn test_process_message_returns_clarification() {
        // 回归保护：process_message 编排路径（共享骨架 + 快照/压缩/技能学习 extras）。
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(response_with(ask_user_msg("测试问题"))));
        let agent = make_agent(mock);

        let resp = agent
            .process_message("session-1", &Message::user("帮我处理"), None)
            .await
            .unwrap();

        assert!(resp.needs_clarification, "ask_user 应触发追问响应");
        assert_eq!(resp.clarification_questions.len(), 1);
        assert_eq!(resp.clarification_questions[0].question, "测试问题");
    }

    #[tokio::test]
    async fn test_clarification_response_returns_answer() {
        // 回归保护：handle_clarification_response 编排路径（共享骨架 + 审批确认 extras）。
        // 注意：trait 层 handle_clarification 每次重新 load_and_build_state，
        // pending_clarification 不跨请求保留，故直接构造带待追问的状态调用
        // pub(crate) handle_clarification_response 以覆盖其编排路径。
        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .times(1)
            .returning(|_| Ok(response_with(Message::assistant("处理完成"))));
        let agent = make_agent(mock);

        let state = agent.load_and_build_state("session-1").await.unwrap();
        state.write().await.pending_clarification = Some(vec![ClarificationQuestion {
            question: "测试问题".to_string(),
            question_type: QuestionType::OpenEnded,
            options: None,
            required: true,
        }]);

        let resp = agent
            .handle_clarification_response(&state, "好的")
            .await
            .unwrap();

        assert!(!resp.needs_clarification, "回答追问后应返回正常回答");
        assert_eq!(resp.content, "处理完成");
        // 待追问状态已被清理
        assert!(state.read().await.pending_clarification.is_none());
    }

    #[tokio::test]
    async fn test_prepare_context_injects_session_hint() {
        // 回归保护：组装上下文必须包含会话定位信息（session_id + vfs 检索路径），
        // 使 LLM 在早期对话被压缩后仍能向上检索原始记录（resume 不丢意图的最后一环）。
        let mock = MockChatService::new();
        let agent = make_agent(mock);

        let state = agent.load_and_build_state("session-abc").await.unwrap();
        let query_msg = Message::user("你好");
        let messages = agent.prepare_context(&state, &query_msg).await;

        let hint = messages
            .iter()
            .find(|m| m.role == MessageRole::System && m.content.contains("会话定位"));
        assert!(hint.is_some(), "上下文应包含会话定位提示");
        let hint = hint.unwrap();
        assert!(
            hint.content.contains("session-abc"),
            "提示应包含当前会话 ID"
        );
        assert!(
            hint.content.contains("tianyan://session/session-abc"),
            "提示应包含可检索的会话 URI"
        );
        // 提示应位于 system 前缀之后、历史/当前输入之前
        let hint_idx = messages
            .iter()
            .position(|m| m.content == hint.content)
            .unwrap();
        assert!(
            messages[hint_idx + 1..]
                .iter()
                .any(|m| m.role == MessageRole::User),
            "会话定位提示之后应有历史或当前输入"
        );
        assert!(
            messages[..hint_idx]
                .iter()
                .all(|m| m.role == MessageRole::System),
            "会话定位提示之前只应有 system 前缀（soul/rules）"
        );
    }

    #[test]
    fn test_clarification_answer_injected_once() {
        // 回归保护：handle_clarification_response 链路中，用户对追问的回答
        // 通过 prepare_context 仅注入内存状态一次；持久化（persist_user_message）
        // 走 VFS，与上下文组装是两条独立存储，组装结果不得出现重复回答。
        let mut state = SessionState::new("session-1");
        state.add_user_message("原始问题");
        state.add_structured_message(ContextAssembler::message_to_structured(
            &Message::assistant("为了继续，需要确认：是否允许执行操作？"),
            "session-1",
            None,
            None,
        ));
        // prepare_context 对追问回答的注入（每次调用仅一次）
        state.add_user_message("允许");

        let messages = ContextAssembler::assemble(
            &state.structured_messages,
            &InjectableContext::default(),
            "",
        );

        let answer_count = messages
            .iter()
            .filter(|m| m.role == MessageRole::User && m.content.contains("允许"))
            .count();
        assert_eq!(answer_count, 1, "用户回答在组装上下文中只能出现一次");
    }
}
