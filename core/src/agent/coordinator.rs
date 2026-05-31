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

use crate::agent::harness::AgentHarness;
use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::skill_subsystem::AgentSkills;
use crate::agent::types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    StreamChunkType, StreamEventSender,
};
use crate::common::error::Result;
use crate::common::types::{Message, StructuredMessage};
use crate::config::AgentConfig;
use crate::context::{ContextAssembler, ContextPipeline};
use crate::executor::{LlmJudge, VerificationGate};
use crate::model::ChatService;
use crate::vfs::VirtualFileSystem;

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    async fn process_message(
        &self,
        state: Arc<RwLock<SessionState>>,
        message: &str,
    ) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    async fn process_message_stream(
        &self,
        state: Arc<RwLock<SessionState>>,
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    /// 处理用户对追问的回答。
    async fn handle_clarification(
        &self,
        state: Arc<RwLock<SessionState>>,
        answers: &str,
    ) -> Result<AgentResponse>;

    async fn initialize(&self) -> Result<()>;
    async fn get_state(&self) -> AgentState;
    async fn shutdown(&self) -> Result<()>;
}

/// 智能体协调器的默认实现。
pub struct Agent {
    config: AgentConfig,
    model_service: Arc<dyn ChatService>,
    vfs: Arc<dyn VirtualFileSystem>,
    context_pipeline: ContextPipeline,
    harness: AgentHarness,
    skills: AgentSkills,
    state: Arc<RwLock<AgentState>>,
    verification_gate: VerificationGate,
    llm_judge: Option<LlmJudge>,
    agent_loop: AgentLoop,
}

impl Agent {
    /// 创建新的 Agent 实例。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: AgentConfig,
        model_service: Arc<dyn ChatService>,
        vfs: Arc<dyn VirtualFileSystem>,
        context_pipeline: ContextPipeline,
        harness: AgentHarness,
        skills: AgentSkills,
        verification_gate: VerificationGate,
        llm_judge: Option<LlmJudge>,
        agent_loop: AgentLoop,
    ) -> Self {
        Self {
            config,
            model_service,
            vfs,
            context_pipeline,
            harness,
            skills,
            state: Arc::new(RwLock::new(AgentState::default())),
            verification_gate,
            llm_judge,
            agent_loop,
        }
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
        let cached = {
            let s = state.read().await;
            s.injectable_context.clone()
        };

        let injectable = if cached.soul.is_empty() {
            match self.context_pipeline.load_injectable(query).await {
                Ok(ctx) => {
                    let injected_rules = ctx.rules_and_experiences.len();
                    if injected_rules > 0 {
                        self.harness.metrics.record_rule_hit(injected_rules).await;
                    }
                    state.write().await.injectable_context = ctx.clone();
                    ctx
                }
                Err(e) => {
                    tracing::warn!(error = %e, "加载可注入上下文失败");
                    self.harness.metrics.record_pipeline_failure().await;
                    cached
                }
            }
        } else {
            cached
        };

        // 压缩对话历史（每轮独立执行，不影响前缀稳定性）
        {
            let s = state.read().await;
            let mut conversation: Vec<Message> = s
                .structured_messages
                .iter()
                .flat_map(|sm| ContextAssembler::structured_to_messages(sm))
                .collect();

            if let Ok(summary) = self.context_pipeline.compress_if_needed(&mut conversation).await {
                if summary.is_some() && conversation.len() < s.structured_messages.len() {
                    drop(s);
                    let (session_id, parent_id) = {
                        let s = state.read().await;
                        (s.session_id.clone(), s.structured_messages.last().map(|m| m.id.clone()))
                    };
                    let new_structured: Vec<StructuredMessage> = conversation
                        .iter()
                        .map(|m| {
                            ContextAssembler::message_to_structured(
                                m,
                                &session_id,
                                parent_id.as_deref(),
                            )
                        })
                        .collect();
                    state.write().await.structured_messages = new_structured;
                }
            }
        }

        let s = state.read().await;
        ContextAssembler::assemble(&s.structured_messages, &injectable, "")
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

        let loop_result = self.agent_loop.run(&mut messages, None).await;

        let response = match loop_result {
            Ok(AgentLoopResult::Answer(content)) => {
                let parent_id = {
                    let s = state.read().await;
                    s.structured_messages.last().map(|m| m.id.clone())
                };
                let sm = ContextAssembler::message_to_structured(
                    &Message::assistant(&content),
                    &state.read().await.session_id,
                    parent_id.as_deref(),
                );
                state.write().await.add_structured_message(sm);

                let mut resp = AgentResponse::simple(content);
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                self.harness.metrics.record_execution(true).await;
                resp
            }
            Ok(AgentLoopResult::NeedsClarification { question }) => {
                let question_obj = ClarificationQuestion {
                    question,
                    question_type: QuestionType::OpenEnded,
                    options: None,
                    required: true,
                };
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                let formatted = format_clarification_questions(&[question_obj.clone()]);
                let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.harness.metrics.record_execution(false).await;
                let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
        };

        {
            let mut st = self.state.write().await;
            st.conversations_processed += 1;
        }

        Ok(response)
    }

    /// 从会话执行历史中自动学习新技能（GEPA 进化引擎）。
    #[allow(dead_code)]
    async fn learn_skills_from_session(&self, _state: &SessionState) -> Result<()> {
        // TODO: 重构后恢复
        Ok(())
    }
}

#[async_trait]
impl AgentCoordinator for Agent {
    async fn process_message(
        &self,
        state: Arc<RwLock<SessionState>>,
        message: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();
        let message = message.to_string();

        let mut messages = self.prepare_context(&state, &message).await;

        let loop_result = self.agent_loop.run(&mut messages, None).await;

        let response = match loop_result {
            Ok(AgentLoopResult::Answer(content)) => {
                let parent_id = {
                    let s = state.read().await;
                    s.structured_messages.last().map(|m| m.id.clone())
                };
                let sm = ContextAssembler::message_to_structured(
                    &Message::assistant(&content),
                    &state.read().await.session_id,
                    parent_id.as_deref(),
                );
                state.write().await.add_structured_message(sm);

                let mut resp = AgentResponse::simple(content);
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                self.harness.metrics.record_execution(true).await;
                resp
            }
            Ok(AgentLoopResult::NeedsClarification { question }) => {
                let question_obj = ClarificationQuestion {
                    question,
                    question_type: QuestionType::OpenEnded,
                    options: None,
                    required: true,
                };
                state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                let formatted = format_clarification_questions(&[question_obj.clone()]);
                let mut resp = AgentResponse::clarification(vec![question_obj], formatted);
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.harness.metrics.record_execution(false).await;
                let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
        };

        {
            let mut st = self.state.write().await;
            st.conversations_processed += 1;
        }

        Ok(response)
    }
    async fn process_message_stream(
        &self,
        state: Arc<RwLock<SessionState>>,
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>> {
        let (tx, rx) = mpsc::channel(100);
        let self_clone = Arc::new(self.clone());
        let message = message.to_string();
        let stream_sender = StreamEventSender::new(tx.clone());

        tokio::spawn(async move {
            let messages = self_clone.prepare_context(&state, &message).await;

            let loop_result = self_clone
                .agent_loop
                .run(&mut messages.clone(), Some(stream_sender.clone()))
                .await;

            match loop_result {
                Ok(AgentLoopResult::Answer(content)) => {
                    let parent_id = {
                        let s = state.read().await;
                        s.structured_messages.last().map(|m| m.id.clone())
                    };
                    let sm = ContextAssembler::message_to_structured(
                        &Message::assistant(&content),
                        &state.read().await.session_id,
                        parent_id.as_deref(),
                    );
                    state.write().await.add_structured_message(sm);

                    stream_sender
                        .send_complete(&content, StreamChunkType::Answer, None)
                        .await;
                    {
                        let mut st = self_clone.state.write().await;
                        st.conversations_processed += 1;
                    }
                    self_clone.harness.metrics.record_execution(true).await;
                }
                Ok(AgentLoopResult::NeedsClarification { question }) => {
                    let question_obj = ClarificationQuestion {
                        question,
                        question_type: QuestionType::OpenEnded,
                        options: None,
                        required: true,
                    };
                    state.write().await.pending_clarification = Some(vec![question_obj.clone()]);
                    let formatted = format_clarification_questions(&[question_obj.clone()]);
                    stream_sender
                        .send_complete(&formatted, StreamChunkType::Clarification, None)
                        .await;
                    {
                        let mut st = self_clone.state.write().await;
                        st.conversations_processed += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "AgentLoop 流式执行失败");
                    self_clone.harness.metrics.record_execution(false).await;
                    stream_sender.send_error(&format!("处理失败：{}", e)).await;
                    {
                        let mut st = self_clone.state.write().await;
                        st.conversations_processed += 1;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn handle_clarification(
        &self,
        state: Arc<RwLock<SessionState>>,
        answers: &str,
    ) -> Result<AgentResponse> {
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

impl Clone for Agent {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            model_service: self.model_service.clone(),
            vfs: self.vfs.clone(),
            context_pipeline: self.context_pipeline.clone(),
            harness: self.harness.clone(),
            skills: self.skills.clone(),
            state: self.state.clone(),
            verification_gate: self.verification_gate.clone(),
            llm_judge: self.llm_judge.clone(),
            agent_loop: self.agent_loop.clone(),
        }
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
