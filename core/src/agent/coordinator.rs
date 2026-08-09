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

use std::sync::atomic::AtomicBool;
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
use crate::common::types::Message;

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 处理用户消息。
    /// - `message` — 完整消息（含可选的多模态图片片段，`content` 为纯文本）。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    async fn process_message(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
    ) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    /// - `message` — 完整消息（含可选的多模态图片片段）。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    /// - `cancel` — 取消标志（客户端断开/服务关停时置位）；`None` 表示不可取消。
    async fn process_message_stream(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>>;

    /// 处理用户对追问的回答。
    async fn handle_clarification(&self, session_id: &str, answers: &str) -> Result<AgentResponse>;

    /// 初始化智能体。
    async fn initialize(&self) -> Result<()>;

    /// 获取审批状态快照（工作流配置 + 待处理请求 + 最近审计记录）。
    ///
    /// 用于状态查询接口；审批工作流未装配时返回默认配置与空队列。
    async fn approval_status(&self) -> Result<crate::executor::approval::ApprovalStatusSnapshot>;

    /// 响应待处理审批请求（GUI 审批面板调用）。
    ///
    /// 请求不存在或已超时返回错误（调用方映射为 404/409）。
    async fn respond_approval(
        &self,
        request_id: &str,
        decision: crate::executor::approval::ApprovalDecision,
        reason: Option<String>,
    ) -> Result<()>;

    /// 获取智能体状态。
    async fn get_state(&self) -> AgentState;

    /// 获取后台任务列表快照（delegate_to_agent(background) 的任务）。
    async fn background_tasks(&self) -> Vec<crate::agent::background::BackgroundTask>;

    /// 取消后台任务（终态任务为幂等空操作）。
    ///
    /// 返回是否取消成功（任务不存在时返回 false，不报错）。
    async fn cancel_background_task(&self, task_id: &str) -> Result<bool>;

    /// 手动压缩会话（前端按钮触发）。
    ///
    /// 与自动压缩共用 `maybe_compress_and_persist` 逻辑（含压缩点刷新：
    /// learned rules 缓存清空 + 技能注册表增量注册），仅触发点不同。
    /// 返回是否实际发生了压缩（消息不足 / token 未超阈值时为 false）。
    async fn compress_session(&self, session_id: &str) -> Result<bool>;

    /// 关闭智能体。
    async fn shutdown(&self) -> Result<()>;
}

#[async_trait]
impl AgentCoordinator for Agent {
    async fn process_message(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
    ) -> Result<AgentResponse> {
        let start = Instant::now();

        // ADR-013 串行化：单会话同一时刻只有一个活动轮（用户轮 / 唤醒轮互斥）
        let _turn_guard = self.turn_guard(session_id).await;

        // Resolve model: caller-specified > Agent default config
        let model = model.unwrap_or(&self.default_model);

        // 1. Load session and build state
        let state = self.load_and_build_state(session_id).await?;

        // 1.5 捕获工作区快照（消息处理前，供会话回退恢复文件）
        {
            let s = state.read().await;
            self.capture_workspace_snapshot(session_id, &s).await;
        }

        // 2-6. 共享编排骨架：持久化 → 上下文 → AgentLoop → 结果组装 → 指标更新
        // 非流式路径无可取消源（HTTP 请求生命周期内），传 None
        let response = self
            .run_agent_turn(&state, session_id, message, model, start, None)
            .await?;

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
        message: &Message,
        model: Option<&str>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<mpsc::Receiver<Result<AgentStreamChunk>>> {
        let state = self.load_and_build_state(session_id).await?;
        let model = model.unwrap_or(&self.default_model).to_string();

        let (tx, rx) = mpsc::channel(100);
        let self_clone = Arc::new(self.clone());
        let message = message.clone();
        let stream_sender = StreamEventSender::new(tx.clone());
        let state_clone = state.clone();

        tokio::spawn(async move {
            // ADR-013 串行化：单会话同一时刻只有一个活动轮（用户轮 / 唤醒轮互斥）；
            // 锁覆盖 prepare_context + AgentLoop 全程（其他轮排队等待）
            let (session_id_str, _) = {
                let s = state_clone.read().await;
                (s.session_id.clone(), ())
            };
            let _turn_guard = self_clone.turn_guard(&session_id_str).await;

            let messages = self_clone.prepare_context(&state_clone, &message).await;

            // 捕获工作区快照（消息处理前，供会话回退恢复文件）
            {
                let s = state_clone.read().await;
                self_clone
                    .capture_workspace_snapshot(&s.session_id, &s)
                    .await;
            }

            let parent_id = {
                let s = state_clone.read().await;
                s.structured_messages.last().map(|m| m.id.clone())
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
                    cancel.as_deref(),
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
                Ok(AgentLoopResult::Cancelled {
                    total_tokens,
                    turns,
                }) => {
                    tracing::info!(turns, "AgentLoop 流式执行被取消");
                    self_clone.metrics.record_execution(false).await;
                    stream_sender
                        .send_complete("任务已取消", StreamChunkType::Answer, None)
                        .await;
                    self_clone
                        .update_agent_metrics(&session_id_str, Some(&total_tokens), false)
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
        // ADR-013 串行化：澄清回答是用户轮的延续，同样占用会话轮次锁
        let _turn_guard = self.turn_guard(session_id).await;
        let state = self.load_and_build_state(session_id).await?;
        self.handle_clarification_response(&state, answers).await
    }

    async fn compress_session(&self, session_id: &str) -> Result<bool> {
        let state = self.load_and_build_state(session_id).await?;
        Ok(self.maybe_compress_and_persist(&state, session_id).await)
    }

    async fn initialize(&self) -> Result<()> {
        tracing::info!("Agent initialized with AgentLoop architecture");
        Ok(())
    }

    async fn approval_status(&self) -> Result<crate::executor::approval::ApprovalStatusSnapshot> {
        use crate::executor::approval::{ApprovalStatusSnapshot, ApprovalWorkflowConfig};

        let registry = self.agent_loop.tool_registry();
        let pending_confirmations = registry.pending_approval_fingerprints().await;
        let Some(workflow) = registry.approval_workflow() else {
            return Ok(ApprovalStatusSnapshot {
                config: ApprovalWorkflowConfig::default(),
                pending_approvals: Vec::new(),
                pending_confirmations,
                recent_records: Vec::new(),
                confirmed_action_count: 0,
            });
        };
        let mut records = workflow.get_approval_records().await;
        records.reverse();
        records.truncate(50);
        Ok(ApprovalStatusSnapshot {
            config: workflow.config(),
            pending_approvals: workflow.get_pending_approvals().await,
            pending_confirmations,
            recent_records: records,
            confirmed_action_count: workflow.confirmed_action_count().await,
        })
    }

    async fn respond_approval(
        &self,
        request_id: &str,
        decision: crate::executor::approval::ApprovalDecision,
        reason: Option<String>,
    ) -> Result<()> {
        let registry = self.agent_loop.tool_registry();
        let Some(workflow) = registry.approval_workflow() else {
            return Err(crate::TianyanError::Custom(
                "审批流程未装配（审批工作流不可用）".to_string(),
            ));
        };
        // GUI 人工响应：approved_by 固定为 "user"
        workflow
            .respond_to_approval(request_id, decision, reason, "user")
            .await
    }

    async fn get_state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    async fn background_tasks(&self) -> Vec<crate::agent::background::BackgroundTask> {
        self.agent_loop
            .tool_registry()
            .background_tasks
            .snapshot()
            .await
    }

    async fn cancel_background_task(&self, task_id: &str) -> Result<bool> {
        let manager = &self.agent_loop.tool_registry().background_tasks;
        if manager.get(task_id).await.is_none() {
            return Ok(false);
        }
        manager.cancel(task_id).await?;
        Ok(true)
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
