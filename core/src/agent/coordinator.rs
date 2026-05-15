//! 智能体协调器实现。
//!
//! 本模块提供集成所有组件的主要智能体协调器。
//!
//! # 架构设计
//!
//! AgentCoordinator 是 Planner-Executor 架构的对外接口，负责：
//! - 管理会话状态（SessionState）
//! - 调用 Planner 进行任务规划
//! - 处理追问（Clarification）
//! - 集成 VFS、检索器、技能执行器等组件
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    AgentCoordinator                          │
//! │  (对外接口，管理会话状态)                                      │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!                             ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Planner                                 │
//! │  (LLM 驱动，负责意图理解、信息收集、计划生成)                   │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!                             ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     Executor                                 │
//! │  (纯机械执行，无思考能力)                                      │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!         ┌───────────────────┼───────────────────┐
//!         ▼                   ▼                   ▼
//!   ┌──────────┐       ┌──────────┐       ┌──────────┐
//!   │   VFS    │       │Retriever │       │  Skills  │
//!   └──────────┘       └──────────┘       └──────────┘
//! ```
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
//! 当 Planner 信息不足时，会返回追问：
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
//! ## Planner-Executor 迭代循环
//!
//! Coordinator 自动管理 Planner-Executor 迭代循环：
//!
//! ```text
//! 用户输入 → Coordinator.process_message()
//!              │
//!              ├─→ SessionState.add_user_message()
//!              │
//!              ├─→ Planner.run(input, state)
//!              │       │
//!              │       ├─→ 第 1 轮：Plan::Steps([...])
//!              │       │       │
//!              │       │       └─→ Executor.execute_steps()
//!              │       │               │
//!              │       │               └─→ SessionState.add_turn()
//!              │       │
//!              │       ├─→ 第 2 轮：Plan::Steps([...])
//!              │       │       │
//!              │       │       └─→ Executor.execute_steps()
//!              │       │
//!              │       └─→ 第 3 轮：Plan::DirectAnswer(...)
//!              │
//!              └─→ SessionState.add_assistant_message()
//!                      │
//!                      └─→ 返回给用户
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
//! - **Planner 错误**：转换为 AgentResponse 错误消息
//! - **追问处理**：返回 `needs_clarification: true` 的响应
//! - **执行失败**：记录日志并返回友好的错误消息
//! - **资源限制**：当达到最大迭代次数时生成部分总结

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, instrument};

use crate::agent::harness::AgentHarness;
use crate::agent::session_state::SessionState;
use crate::agent::skill_subsystem::AgentSkills;
use crate::agent::types::{
    AgentResponse, AgentState, AgentStreamChunk, SkillCallInfo, StreamChunkType, StreamEventSender,
};
use crate::common::error::Result;
use crate::config::AgentConfig;
use crate::context::{ContextPipeline, FailureKind};
use crate::executor::{LlmJudge, VerificationGate};
use crate::model::ModelService;
use crate::skills::learning::{ExecutionHistory, ExecutionStep};
use crate::skills::Skill;
use crate::storage::{MemoryExtractionTrait, VirtualFileSystem};

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    ///
    /// - `state` - 会话状态
    /// - `message` - 用户消息
    /// - returns: 智能体响应
    async fn process_message(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    ///
    /// 支持真正的阶段性流式响应：
    /// - Thought: 思考过程
    /// - ToolCall: 工具/技能调用
    /// - Observation: 执行结果观察
    /// - Answer: 最终回答
    ///
    /// - `state` - 会话状态
    /// - `message` - 用户消息
    /// - returns: 流式响应接收端
    async fn process_message_stream(
        &self,
        state: &mut SessionState,
        message: &str,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    /// 处理用户对追问的回答。
    ///
    /// - `state` - 会话状态
    /// - `answers` - 用户回答
    /// - returns: 智能体响应
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
    model_service: Arc<dyn ModelService>,
    vfs: Arc<dyn VirtualFileSystem>,
    context_pipeline: ContextPipeline,
    harness: AgentHarness,
    skills: AgentSkills,
    state: Arc<RwLock<AgentState>>,
    memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    verification_gate: VerificationGate,
    llm_judge: Option<LlmJudge>,
}

impl Agent {
    /// 创建新的 Agent 实例。
    ///
    /// 所有子组件应由 AgentBuilder 预构建后传入。
    /// 不建议直接调用此构造函数，请使用 AgentBuilder::build()。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: AgentConfig,
        model_service: Arc<dyn ModelService>,
        vfs: Arc<dyn VirtualFileSystem>,
        context_pipeline: ContextPipeline,
        harness: AgentHarness,
        skills: AgentSkills,
        memory_extractor: Option<Arc<dyn MemoryExtractionTrait + Send + Sync>>,
        verification_gate: VerificationGate,
        llm_judge: Option<LlmJudge>,
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
        }
    }

    /// 处理用户对追问的回答。
    ///
    /// - `state` - 会话状态
    /// - `clarification_answers` - 用户对追问的回答
    /// - returns: 处理后的响应
    ///
    /// # Errors
    /// 返回处理过程中的错误
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
                // 统计本次注入的 learned rules 数量
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

        // TODO: 重构后恢复 Planner 调用
        let mut response = AgentResponse::simple("追问回答已收到，继续处理中...".to_string());

        response.processing_time_ms = start.elapsed().as_millis() as u64;
        Ok(response)
    }

    /// 在会话结束后提取和保存记忆。
    async fn extract_memories_from_session(&self, state: &SessionState) -> Result<()> {
        if let Some(ref extractor) = self.memory_extractor {
            let turn_threshold = 3; // 最小对话轮数
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
                // 统计本次注入的 learned rules 数量
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
                // 管线失败接入 RuleRecorder（系统级失败）
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

        // TODO: 重构后恢复 Planner 调用
        let mut response = AgentResponse::simple(message);
        response.processing_time_ms = start.elapsed().as_millis() as u64;
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
            // TODO: 重构后恢复流式 Planner 调用
            stream_sender
                .send_complete(&message, StreamChunkType::Answer, None)
                .await;
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
        info!("Agent initialized with Planner architecture");
        Ok(())
    }

    async fn get_state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down agent");

        // 等待后台任务完成（最多 5 秒）
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
        }
    }
}

/// 格式化追问问题为用户友好的文本
fn format_clarification_questions(
    questions: &[crate::agent::types::ClarificationQuestion],
) -> String {
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

/// 从执行历史轮次中提取 SkillCall 信息。
#[allow(dead_code)]
fn extract_skill_calls_from_turns(_turns: &[String]) -> Vec<SkillCallInfo> {
    // TODO: 重构后恢复
    Vec::new()
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_format_clarification_questions() {
        use crate::agent::types::{ClarificationQuestion, QuestionType};

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
