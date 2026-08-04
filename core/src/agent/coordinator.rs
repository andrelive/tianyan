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
//! Coordinator 每次处理消息时从 SessionManager 加载会话状态，
//! 运行期间持有单个 SessionState（不跨请求缓存多会话状态）。
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
use tokio::sync::mpsc;

use crate::agent::agent_core::{format_clarification_questions, Agent};
use crate::agent::r#loop::AgentLoopResult;
use crate::agent::types::{
    AgentResponse, AgentState, AgentStreamChunk, ClarificationQuestion, QuestionType,
    StreamChunkType, StreamEventSender,
};
use crate::common::error::Result;
use crate::common::types::TokenUsage;

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    async fn process_message(
        &self,
        session_id: &str,
        message: &str,
        model: Option<&str>,
    ) -> Result<AgentResponse>;

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

#[async_trait]
impl AgentCoordinator for Agent {
    async fn process_message(
        &self,
        session_id: &str,
        message: &str,
        model: Option<&str>,
    ) -> Result<AgentResponse> {
        let start = Instant::now();
        let message = message.to_string();

        // Resolve model: caller-specified > Agent default config
        let model = model.unwrap_or(&self.default_model);

        // 1. Load session and build state
        let state = self.load_and_build_state(session_id).await?;

        // 1.5 捕获工作区快照（消息处理前，供会话回退恢复文件）
        {
            let s = state.read().await;
            self.capture_workspace_snapshot(session_id, &s).await;
        }

        // 2. Persist current user message
        self.persist_user_message(session_id, &state, &message)
            .await;

        // 3. Prepare context
        let messages = self.prepare_context(&state, &message).await;

        // 4. Run agent loop (internal persistence in AgentLoop)
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
            )
            .await;

        // 5. Handle loop result
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
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop 执行失败");
                self.metrics.record_execution(false).await;
                let mut resp = AgentResponse::error(format!("处理失败：{}", e));
                resp.processing_time_ms = start.elapsed().as_millis() as u64;
                resp
            }
        };

        // 6. Update agent metrics
        self.update_agent_metrics(session_id, loop_tokens.as_ref(), loop_tokens.is_some())
            .await;

        // 7. Compression check and persist
        self.maybe_compress_and_persist(&state, session_id).await;

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

            // 捕获工作区快照（消息处理前，供会话回退恢复文件）
            {
                let s = state_clone.read().await;
                self_clone
                    .capture_workspace_snapshot(&s.session_id, &s)
                    .await;
            }

            let (session_id_str, parent_id) = {
                let s = state_clone.read().await;
                (
                    s.session_id.clone(),
                    s.structured_messages.last().map(|m| m.id.clone()),
                )
            };

            // 持久化用户消息（和非流式路径对齐）
            self_clone
                .persist_user_message(&session_id_str, &state_clone, &message)
                .await;

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
                        .add_structured_message(*persisted_message);

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
                    let formatted =
                        format_clarification_questions(std::slice::from_ref(&question_obj));
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
        tracing::info!("Agent initialized with AgentLoop architecture");
        Ok(())
    }

    async fn get_state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    async fn shutdown(&self) -> Result<()> {
        tracing::info!("Shutting down agent");
        Ok(())
    }
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
