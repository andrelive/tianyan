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
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, instrument};

use crate::agent::harness::AgentHarness;
use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::skill_subsystem::AgentSkills;
use crate::agent::types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    StreamChunkType, StreamEventSender,
};
use crate::common::error::Result;
use crate::common::types::{Message, MessageRole};
use crate::config::AgentConfig;
use crate::context::{ContextPipeline, FailureKind};
use crate::executor::{LlmJudge, VerificationGate};
use crate::model::ChatService;
use crate::storage::{MemoryExtractionTrait, VirtualFileSystem};

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    async fn process_message(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    async fn process_message_stream(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    /// 处理用户对追问的回答。
    async fn handle_clarification(
        &self,
        state: &mut SessionState,
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
    memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
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
        memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
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
            memory_extractor,
            background_tasks: Arc::new(Mutex::new(Vec::new())),
            verification_gate,
            llm_judge,
            agent_loop,
        }
    }

    /// 处理用户对追问的回答。
    async fn handle_clarification_response(
        &self,
        state: &mut SessionState,
        clarification_answers: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();

        if state.pending_clarification.is_none() {
            return Ok(AgentResponse::simple("当前没有待处理的追问".to_string()));
        }

        state.add_user_message(clarification_answers);
        state.pending_clarification.take();

        // 运行上下文管线
        match self
            .context_pipeline
            .run(clarification_answers, &mut state.conversation)
            .await
        {
            Ok(window) => {
                let injected_rules = window
                    .system_prompt
                    .lines()
                    .filter(|l| l.starts_with("- [") && l.contains("来源会话"))
                    .count();
                if injected_rules > 0 {
                    self.harness.metrics.record_rule_hit(injected_rules).await;
                }
                state.context_window = Some(window);
            }
            Err(e) => {
                tracing::warn!(error = %e, "追问上下文管线执行失败");
                self.harness.metrics.record_pipeline_failure().await;
            }
        }

        // 注入系统提示词
        let system_prompt = state
            .context_window
            .as_ref()
            .map(|w| w.system_prompt.clone());
        if let Some(prompt) = system_prompt {
            if let Some(first) = state.conversation.first_mut() {
                if first.role == MessageRole::System {
                    first.content = prompt;
                } else {
                    state.conversation.insert(0, Message::system(&prompt));
                }
            } else {
                state.conversation.push(Message::system(&prompt));
            }
        }

        let loop_result = self.agent_loop.run(&mut state.conversation, None).await;

        let response = match loop_result {
            Ok(AgentLoopResult::Answer(content)) => {
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
                state.pending_clarification = Some(vec![question_obj.clone()]);
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

        if response.is_complete && !response.needs_clarification {
            let agent_clone = self.clone();
            let state_clone = state.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = agent_clone
                    .extract_memories_from_session(&state_clone)
                    .await
                {
                    tracing::warn!(error = %e, "记忆提取后台任务失败");
                }
                agent_clone
                    .scan_and_promote_rules(&state_clone.session_id)
                    .await;
                if let Err(e) = agent_clone.learn_skills_from_session(&state_clone).await {
                    tracing::warn!(error = %e, "技能学习后台任务失败");
                }
            });
            self.background_tasks.lock().await.push(handle);
        }

        Ok(response)
    }

    /// 在会话结束后提取和保存记忆。
    async fn extract_memories_from_session(&self, state: &SessionState) -> Result<()> {
        if let Some(ref extractor) = self.memory_extractor {
            let turn_threshold = 3;
            if state.conversation.len() >= turn_threshold * 2 {
                let conversation: String = state
                    .conversation
                    .iter()
                    .map(|m| format!("{}: {}", m.role, m.content))
                    .collect::<Vec<_>>()
                    .join("\n");

                match extractor
                    .extract_and_store(&conversation, &state.session_id)
                    .await
                {
                    Ok(memories) => {
                        tracing::info!(count = memories.len(), session_id = %state.session_id, "记忆提取完成");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, session_id = %state.session_id, "记忆提取失败");
                    }
                }
            }
        }
        Ok(())
    }

    /// 从会话执行历史中自动学习新技能（GEPA 进化引擎）。
    #[allow(dead_code)]
    async fn learn_skills_from_session(&self, _state: &SessionState) -> Result<()> {
        // TODO: 重构后恢复
        Ok(())
    }

    /// 扫描记忆聚类并自动提炼为规则（后台任务）。
    async fn scan_and_promote_rules(&self, session_id: &str) {
        if let Some(ref suggester) = self.harness.rule_suggester {
            match suggester.scan().await {
                Ok(suggestions) => {
                    for suggestion in suggestions {
                        if suggestion.source_count >= 2 {
                            tracing::info!(
                                category = %suggestion.source_category,
                                count = suggestion.source_count,
                                "检测到记忆聚类，尝试提炼规则"
                            );
                            if let Err(e) = suggester.promote_to_rule(&suggestion, session_id).await
                            {
                                tracing::warn!(error = %e, "规则提炼失败");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "记忆聚类扫描失败");
                }
            }
        }
    }
}

#[async_trait]
impl AgentCoordinator for Agent {
    #[instrument(skip(self, state), fields(session_id = %state.session_id))]
    async fn process_message(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<AgentResponse> {
        let start = Instant::now();
        let message = message.to_string();

        state.add_user_message(&message);

        // 运行上下文管线
        match self
            .context_pipeline
            .run(&message, &mut state.conversation)
            .await
        {
            Ok(window) => {
                let injected_rules = window
                    .system_prompt
                    .lines()
                    .filter(|l| l.starts_with("- [") && l.contains("来源会话"))
                    .count();
                if injected_rules > 0 {
                    self.harness.metrics.record_rule_hit(injected_rules).await;
                }
                state.context_window = Some(window);
            }
            Err(e) => {
                tracing::warn!(error = %e, "上下文管线执行失败");
                self.harness.metrics.record_pipeline_failure().await;
                let session_id = state.session_id.clone();
                let error_msg = format!("{}", e);
                let _ = self
                    .harness
                    .rule_recorder
                    .record_with_kind(
                        &format!("上下文管线失败：{}", error_msg),
                        &format!(
                            "# Pipeline 失败\n\n会话: {}\n错误: {}\n建议: 检查 VFS / 检索 / 压缩子系统的可用性",
                            session_id, error_msg
                        ),
                        &session_id,
                        FailureKind::System,
                    )
                    .await;
            }
        }

        // 注入系统提示词
        let system_prompt = state
            .context_window
            .as_ref()
            .map(|w| w.system_prompt.clone());
        if let Some(prompt) = system_prompt {
            if let Some(first) = state.conversation.first_mut() {
                if first.role == MessageRole::System {
                    first.content = prompt;
                } else {
                    state.conversation.insert(0, Message::system(&prompt));
                }
            } else {
                state.conversation.push(Message::system(&prompt));
            }
        }

        let loop_result = self.agent_loop.run(&mut state.conversation, None).await;

        let response = match loop_result {
            Ok(AgentLoopResult::Answer(content)) => {
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
                state.pending_clarification = Some(vec![question_obj.clone()]);
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

        if response.is_complete && !response.needs_clarification {
            let agent_clone = self.clone();
            let state_clone = state.clone();
            let handle = tokio::spawn(async move {
                if let Err(e) = agent_clone
                    .extract_memories_from_session(&state_clone)
                    .await
                {
                    tracing::warn!(error = %e, "记忆提取后台任务失败");
                }
                agent_clone
                    .scan_and_promote_rules(&state_clone.session_id)
                    .await;
                if let Err(e) = agent_clone.learn_skills_from_session(&state_clone).await {
                    tracing::warn!(error = %e, "技能学习后台任务失败");
                }
            });
            self.background_tasks.lock().await.push(handle);
        }

        Ok(response)
    }

    #[instrument(skip(self, state), fields(session_id = %state.session_id))]
    async fn process_message_stream(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>> {
        let (tx, rx) = mpsc::channel(100);
        let self_clone = Arc::new(self.clone());
        let mut state_clone = state.clone();
        let message = message.to_string();
        let stream_sender = StreamEventSender::new(tx.clone());

        tokio::spawn(async move {
            state_clone.add_user_message(&message);

            // 运行上下文管线
            match self_clone
                .context_pipeline
                .run(&message, &mut state_clone.conversation)
                .await
            {
                Ok(window) => {
                    let injected_rules = window
                        .system_prompt
                        .lines()
                        .filter(|l| l.starts_with("- [") && l.contains("来源会话"))
                        .count();
                    if injected_rules > 0 {
                        self_clone
                            .harness
                            .metrics
                            .record_rule_hit(injected_rules)
                            .await;
                    }
                    state_clone.context_window = Some(window);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "流式上下文管线执行失败");
                    self_clone.harness.metrics.record_pipeline_failure().await;
                }
            }

            // 注入系统提示词
            let system_prompt = state_clone
                .context_window
                .as_ref()
                .map(|w| w.system_prompt.clone());
            if let Some(prompt) = system_prompt {
                if let Some(first) = state_clone.conversation.first_mut() {
                    if first.role == MessageRole::System {
                        first.content = prompt;
                    } else {
                        state_clone.conversation.insert(0, Message::system(&prompt));
                    }
                } else {
                    state_clone.conversation.push(Message::system(&prompt));
                }
            }

            let loop_result = self_clone
                .agent_loop
                .run(&mut state_clone.conversation, Some(stream_sender.clone()))
                .await;

            match loop_result {
                Ok(AgentLoopResult::Answer(content)) => {
                    stream_sender
                        .send_complete(&content, StreamChunkType::Answer, None)
                        .await;
                    {
                        let mut st = self_clone.state.write().await;
                        st.conversations_processed += 1;
                    }
                    self_clone.harness.metrics.record_execution(true).await;

                    let agent_clone2 = self_clone.clone();
                    let state_clone2 = state_clone.clone();
                    let handle = tokio::spawn(async move {
                        if let Err(e) = agent_clone2
                            .extract_memories_from_session(&state_clone2)
                            .await
                        {
                            tracing::warn!(error = %e, "记忆提取后台任务失败");
                        }
                        agent_clone2
                            .scan_and_promote_rules(&state_clone2.session_id)
                            .await;
                        if let Err(e) = agent_clone2.learn_skills_from_session(&state_clone2).await
                        {
                            tracing::warn!(error = %e, "技能学习后台任务失败");
                        }
                    });
                    self_clone.background_tasks.lock().await.push(handle);
                }
                Ok(AgentLoopResult::NeedsClarification { question }) => {
                    let question_obj = ClarificationQuestion {
                        question,
                        question_type: QuestionType::OpenEnded,
                        options: None,
                        required: true,
                    };
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

    #[instrument(skip(self, state), fields(session_id = %state.session_id))]
    async fn handle_clarification(
        &self,
        state: &mut SessionState,
        answers: &str,
    ) -> Result<AgentResponse> {
        self.handle_clarification_response(state, answers).await
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

        let mut tasks = self.background_tasks.lock().await;
        let mut pending = Vec::new();
        std::mem::swap(&mut pending, &mut tasks);
        drop(tasks);

        if !pending.is_empty() {
            tracing::info!(count = pending.len(), "等待后台任务完成");
            let timeout = tokio::time::Duration::from_secs(5);
            for handle in pending {
                let _ = tokio::time::timeout(timeout, handle).await;
            }
        }

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
            memory_extractor: self.memory_extractor.clone(),
            background_tasks: self.background_tasks.clone(),
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
