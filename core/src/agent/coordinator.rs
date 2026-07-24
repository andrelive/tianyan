//! 智能体协调器实现。
//!
//! 本模块提供集成所有组件的主要智能体协调器。
//!
//! # 架构设计
//!
//! AgentCoordinator 是 AgentLoop 架构的对外接口，负责：
//! - 管理会话状态（SessionState）
//! - 调用 AgentLoop 进行推理与工具执行
//! - 处理追问（Clarification）
//! - 集成 VFS、检索器、技能执行器等组件
//!
//! # 使用示例
//!
//! ## 基本使用
//!
//! ```rust,ignore
//! use tianyan::agent::{Agent, AgentCoordinator, SessionState};
//!
//! async fn example(agent: &Agent) -> Result<(), Box<dyn std::error::Error>> {
//!     let mut state = SessionState::new("session-001");
//!     let response = agent.process_message(&mut state, "帮我分析项目性能问题").await?;
//!     println!("响应：{}", response.content);
//!     Ok(())
//! }
//! ```
//!
//! ## 处理追问流程
//!
//! 当 AgentLoop 信息不足时，会返回追问：
//!
//! ```rust,ignore
//! use tianyan::agent::{Agent, AgentCoordinator, SessionState};
//!
//! async fn chat_with_clarification(
//!     agent: &Agent,
//!     user_input: &str,
//! ) -> Result<String, Box<dyn std::error::Error>> {
//!     let mut state = SessionState::new("session-001");
//!
//!     let response = agent.process_message(&mut state, user_input).await?;
//!
//!     if response.needs_clarification {
//!         println!("需要追问：");
//!         for q in &response.clarification_questions {
//!             println!("  - {}", q.question);
//!         }
//!
//!         let user_answer = "我想优化 src/parser.rs，关注执行速度";
//!         let final_response = agent.handle_clarification(&mut state, user_answer).await?;
//!         Ok(final_response.content)
//!     } else {
//!         Ok(response.content)
//!     }
//! }
//! ```
//!
//! ## AgentLoop 迭代循环
//!
//! Coordinator 自动管理 AgentLoop 迭代循环：
//!
//! ```text
//! 用户输入 → Coordinator.process_message()
//!              │
//!              ├─→ SessionState.add_user_message()
//!              │
//!              ├─→ context_pipeline.run()
//!              │
//!              └─→ AgentLoop.run(messages)
//!                      │
//!                      ├─→ LLM 调用 → 工具调用 → 执行 → 循环
//!                      │
//!                      └─→ Answer / NeedsClarification
//! ```
//!
//! ## 会话状态管理
//!
//! Coordinator 使用 SessionStateManager 管理所有会话的运行时状态：
//!
//! ```rust,no_run
//! use tianyan::agent::session_state::SessionStateManager;
//!
//! #[tokio::main]
//! async fn main() {
//!     let manager = SessionStateManager::new();
//!     
//!     // 创建会话
//!     manager.with_state("session-001", |state| {
//!         state.add_user_message("Hello");
//!         state.add_assistant_message("Hi there!");
//!     }).await;
//!     
//!     // 读取会话
//!     let msg_count = manager.with_state_read("session-001", |state| {
//!         state.get_conversation().len()
//!     }).await;
//!     
//!     println!("会话消息数：{}", msg_count.unwrap_or(0));
//!     
//!     // 清理过期会话（30 分钟不活动）
//!     manager.cleanup_expired(1800).await;
//! }
//! ```
//!
//! # 错误处理
//!
//! Coordinator 统一处理各种错误情况：
//!
//! - **AgentLoop 错误**：转换为 AgentResponse 错误消息
//! - **追问处理**：返回 `needs_clarification: true` 的响应
//! - **执行失败**：记录日志并返回友好的错误消息
//! - **资源限制**：当达到最大迭代次数时生成错误响应

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tracing::info;

use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    StreamChunkType, StreamEventSender,
};
use crate::common::error::Result;
use crate::common::types::{Message, StructuredMessage, TokenUsage};
use crate::config::AgentConfig;
use crate::context::{ContextAssembler, ContextPipeline};
use crate::model::ChatService;
use crate::observability::AgentMetrics;
use crate::observability::TokenRecord;
use crate::session::{Session, SessionManager};
use crate::skills::{SkillExecutor, SkillLearningEngine, SkillRegistry};
use crate::vfs::VirtualFileSystem;

/// compression_marker 之后至少积累多少条消息才触发压缩。
const MIN_MESSAGES_BEFORE_COMPRESSION: usize = 6;

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    async fn process_message(&self, session_id: &str, message: &str, model: Option<&str>) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    async fn process_message_stream(
        &self,
        session_id: &str,
        message: &str,
        model: Option<&str>,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    /// 处理用户对追问的回答。
    async fn handle_clarification(&self, session_id: &str, answers: &str) -> Result<AgentResponse>;

    /// 初始化智能体。
    async fn initialize(&self) -> Result<()>;
    /// 获取智能体状态。
    async fn get_state(&self) -> AgentState;
    /// 关闭智能体。
    async fn shutdown(&self) -> Result<()>;
}

/// 智能体协调器的默认实现。
#[derive(Clone)]
pub struct Agent {
    config: AgentConfig,
    /// 默认对话模型名（配置中指定，未被请求级 model 覆盖时使用）。
    default_model: String,
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    context_pipeline: ContextPipeline,
    metrics: Arc<AgentMetrics>,
    skill_executor: Option<Arc<SkillExecutor>>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    skill_learning_engine: Option<SkillLearningEngine>,
    state: Arc<RwLock<AgentState>>,
    agent_loop: AgentLoop,
    session_manager: Arc<dyn SessionManager>,
}

impl Agent {
    /// 创建新的 Agent 实例。
    ///
    /// 此构造函数由 AgentBuilder::build() 调用，外部应通过 Builder 创建 Agent。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: AgentConfig,
        default_model: String,
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        context_pipeline: ContextPipeline,
        metrics: Arc<AgentMetrics>,
        skill_executor: Option<Arc<SkillExecutor>>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        skill_learning_engine: Option<SkillLearningEngine>,
        agent_loop: AgentLoop,
        session_manager: Arc<dyn SessionManager>,
    ) -> Self {
        Self {
            config,
            default_model,
            model_service,
            vfs,
            context_pipeline,
            metrics,
            skill_executor,
            skill_registry,
            skill_learning_engine,
            state: Arc::new(RwLock::new(AgentState::default())),
            agent_loop,
            session_manager,
        }
    }

    /// 从持久化存储加载会话并构建 SessionState。
    async fn load_and_build_state(
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
    async fn prepare_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        query: &str,
    ) -> Vec<Message> {
        state.write().await.add_user_message(query);

        // 会话级缓存：soul 非空表示已加载过，跳过 I/O
        // 先检查 soul 是否已加载（避免克隆整个 InjectableContext）
        let soul_loaded = {
            let s = state.read().await;
            !s.injectable_context.soul.is_empty()
        };

        let injectable = if !soul_loaded {
            match self.context_pipeline.load_injectable(query).await {
                Ok(ctx) => {
                    let injected_rules = ctx.rules_and_experiences.len();
                    if injected_rules > 0 {
                        self.metrics.record_rule_hit(injected_rules).await;
                    }
                    state.write().await.injectable_context = ctx;
                    // 加载后只需 clone 一次供 assemble 使用
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
        ContextAssembler::assemble(&s.structured_messages, &injectable, "")
    }

    /// 判断是否需要压缩，如果需要则生成摘要 StructuredMessage 并持久化。
    async fn maybe_compress_and_persist(
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

    /// 处理用户对追问的回答。
    async fn handle_clarification_response(
        &self,
        state: &Arc<RwLock<SessionState>>,
        clarification_answers: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();

        {
            let s = state.read().await;
            if s.pending_clarification.is_none() {
                return Ok(AgentResponse::simple("当前没有待处理的追问".to_string()));
            }
        }
        state.write().await.pending_clarification = None;

        let mut messages = self.prepare_context(state, clarification_answers).await;

        let (session_id, parent_id) = {
            let s = state.read().await;
            (
                s.session_id.clone(),
                s.structured_messages.last().map(|m| m.id.clone()),
            )
        };
        let loop_result = self
            .agent_loop
            .run(&mut messages, None, &session_id, parent_id.as_deref(), &self.default_model)
            .await;

        let mut clarification_tokens: Option<TokenUsage> = None;
        let response = match loop_result {
            Ok(AgentLoopResult::Answer {
                content,
                total_tokens,
                persisted_message,
                ..
            }) => {
                // 复用 AgentLoop 已持久化的消息，避免重复创建
                state
                    .write()
                    .await
                    .add_structured_message(persisted_message);

                let mut resp = AgentResponse::simple(content);
                resp.token_usage = total_tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                self.metrics.record_execution(true).await;
                clarification_tokens = Some(total_tokens);
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
                let formatted = format_clarification_questions(&[question_obj.clone()]);
                let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                resp.token_usage = total_tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                clarification_tokens = Some(total_tokens);
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

        self.update_agent_metrics(
            &session_id,
            clarification_tokens.as_ref(),
            clarification_tokens.is_some(),
        )
        .await;

        Ok(response)
    }

    /// 从会话执行历史中自动学习新技能（GEPA 进化引擎）。
    async fn learn_skills_from_session(&self, session_id: &str) {
        if let Some(ref engine) = self.skill_learning_engine {
            // 从 ToolRegistry 排空执行轨迹
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
    async fn update_agent_metrics(
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
            // Record per-session token usage for self_check tool / observability.
            // system_prompt_tokens and retrieved_tokens would require pipeline
            // instrumentation for accurate values; recorded as 0 for now.
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
    async fn persist_user_message(&self, session_id: &str, state: &Arc<RwLock<SessionState>>, message: &str) {
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let user_sm = ContextAssembler::message_to_structured(
            &Message::user(message),
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

#[async_trait]
impl AgentCoordinator for Agent {
    async fn process_message(&self, session_id: &str, message: &str, model: Option<&str>) -> Result<AgentResponse> {
        let start = Instant::now();
        let message = message.to_string();

        // Resolve model: caller-specified > Agent default config
        let model = model.unwrap_or(&self.default_model);

        // 1. Load session and build state
        let state = self.load_and_build_state(session_id).await?;

        // 2. Persist current user message
        self.persist_user_message(session_id, &state, &message).await;

        // 3. Prepare context
        let messages = self.prepare_context(&state, &message).await;

        // 4. Run agent loop (internal persistence in AgentLoop)
        let parent_id = {
            let s = state.read().await;
            s.structured_messages.last().map(|m| m.id.clone())
        };
        let mut messages = messages;
        let loop_result = self
            .agent_loop
            .run(&mut messages, None, session_id, parent_id.as_deref(), model)
            .await;

        // 5. Handle loop result
        let mut loop_tokens: Option<TokenUsage> = None;
        let response = match loop_result {
            Ok(AgentLoopResult::Answer {
                content,
                total_tokens: tokens,
                persisted_message,
                ..
            }) => {
                state
                    .write()
                    .await
                    .add_structured_message(persisted_message);

                let mut resp = AgentResponse::simple(content);
                resp.token_usage = tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                self.metrics.record_execution(true).await;
                loop_tokens = Some(tokens);
                resp
            }
            Ok(AgentLoopResult::NeedsClarification {
                question,
                total_tokens: tokens,
                ..
            }) => {
                let question_obj = ClarificationQuestion {
                    question,
                    question_type: QuestionType::OpenEnded,
                    options: None,
                    required: true,
                };
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                let formatted = format_clarification_questions(&[question_obj.clone()]);
                let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                resp.token_usage = tokens.clone();
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                loop_tokens = Some(tokens);
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

        // 6. Compression check and persist
        self.maybe_compress_and_persist(&state, session_id).await;

        // 7. Update agent metrics
        self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
            .await;

        // 8. Background: GEPA skill learning
        {
            let agent = self.clone();
            let sid = session_id.to_string();
            tokio::spawn(async move {
                agent.learn_skills_from_session(&sid).await;
            });
        }

        Ok(response)
    }

    async fn process_message_stream(
        &self,
        session_id: &str,
        message: &str,
        model: Option<&str>,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>> {
        let state = self.load_and_build_state(session_id).await?;
        let model = model.unwrap_or(&self.default_model).to_string();

        let (tx, rx) = mpsc::channel(100);
        let self_clone = Arc::new(self.clone());
        let message = message.to_string();
        let stream_sender = StreamEventSender::new(tx.clone());
        let state_clone = state.clone();

        tokio::spawn(async move {
            let messages = self_clone.prepare_context(&state_clone, &message).await;

            let (session_id_str, parent_id) = {
                let s = state_clone.read().await;
                (
                    s.session_id.clone(),
                    s.structured_messages.last().map(|m| m.id.clone()),
                )
            };

            // 持久化用户消息（和非流式路径对齐）
            let user_sm = ContextAssembler::message_to_structured(
                &Message::user(&message),
                &session_id_str,
                parent_id.as_deref(),
                None,
            );
            if let Err(e) = self_clone
                .session_manager
                .add_structured_message(&session_id_str, user_sm)
                .await
            {
                tracing::warn!(error = %e, "持久化用户消息失败");
            }

            let loop_result = self_clone
                .agent_loop
                .run_stream(
                    &mut messages.clone(),
                    stream_sender.clone(),
                    &session_id_str,
                    parent_id.as_deref(),
                    &model,
                )
                .await;

            match loop_result {
                Ok(AgentLoopResult::Answer {
                    total_tokens,
                    persisted_message,
                    ..
                }) => {
                    state_clone
                        .write()
                        .await
                        .add_structured_message(persisted_message);

                    // run_stream 已逐 chunk 流式推完内容，这里只发结束标记
                    stream_sender
                        .send_complete("", StreamChunkType::Answer, None)
                        .await;
                    self_clone
                        .update_agent_metrics(&session_id_str, Some(&total_tokens), true)
                        .await;
                    self_clone.metrics.record_execution(true).await;
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
                    state_clone.write().await.pending_clarification =
                        Some(vec![question_obj.clone()]);
                    let formatted = format_clarification_questions(&[question_obj.clone()]);
                    stream_sender
                        .send_complete(&formatted, StreamChunkType::Clarification, None)
                        .await;
                    self_clone
                        .update_agent_metrics(&session_id_str, Some(&total_tokens), true)
                        .await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "AgentLoop 流式执行失败");
                    self_clone.metrics.record_execution(false).await;
                    stream_sender.send_error(&format!("处理失败：{}", e)).await;
                    self_clone
                        .update_agent_metrics(&session_id_str, None, false)
                        .await;
                }
            }

            // Background: GEPA skill learning
            self_clone.learn_skills_from_session(&session_id_str).await;
        });

        Ok(rx)
    }

    async fn handle_clarification(&self, session_id: &str, answers: &str) -> Result<AgentResponse> {
        let state = self.load_and_build_state(session_id).await?;
        self.handle_clarification_response(&state, answers).await
    }

    async fn initialize(&self) -> Result<()> {
        info!("Agent initialized with AgentLoop architecture");
        Ok(())
    }

    async fn get_state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down agent");
        Ok(())
    }
}

/// 格式化追问问题为用户友好的文本
fn format_clarification_questions(questions: &[ClarificationQuestion]) -> String {
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
    use crate::agent::types::{ClarificationQuestion, QuestionType};

    #[test]
    fn test_format_clarification_questions() {
        let questions = vec![
            ClarificationQuestion {
                question: "您想创建什么类型的项目？".to_string(),
                question_type: QuestionType::Choice,
                options: Some(vec!["Rust 项目".to_string(), "TypeScript 项目".to_string()]),
                required: true,
            },
            ClarificationQuestion {
                question: "项目名称是什么？".to_string(),
                question_type: QuestionType::OpenEnded,
                options: None,
                required: true,
            },
        ];

        let result = super::format_clarification_questions(&questions);

        assert!(result.contains("为了更好帮助您，需要以下信息"));
        assert!(result.contains("1. 您想创建什么类型的项目？"));
        assert!(result.contains("2. 项目名称是什么？"));
        assert!(result.contains("可选答案："));
        assert!(result.contains("Rust 项目"));
        assert!(result.contains("TypeScript 项目"));
        assert!(result.contains("(必填)"));
        assert!(result.contains("请提供以上信息，我会根据您的回答继续处理。"));
    }
}
