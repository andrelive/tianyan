//! 智能体核心实现。
//!
//! 包含 Agent 结构体定义、构造函数和所有辅助方法。
//! 将 Agent 与 AgentCoordinator trait 分离，消除循环依赖。

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::agent::r#loop::{AgentLoop, AgentLoopResult};
use crate::agent::session_state::SessionState;
use crate::agent::tool_registry::DynamicToolExecutor;
use crate::agent::types::{AgentResponse, AgentState, ClarificationQuestion, QuestionType};
use crate::common::error::Result;
use crate::common::types::{Message, StructuredMessage, TokenUsage};
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
    pub(crate) async fn prepare_context(
        &self,
        state: &Arc<RwLock<SessionState>>,
        query: &str,
    ) -> Vec<Message> {
        state.write().await.add_user_message(query);

        // 会话级缓存：soul 非空表示已加载过，跳过 I/O
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

    /// 处理用户对追问的回答。
    pub(crate) async fn handle_clarification_response(
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

        // 持久化用户对追问的回答，保持会话历史完整（与 process_message 路径对齐）
        let (session_id, parent_id) = {
            let s = state.read().await;
            (
                s.session_id.clone(),
                s.structured_messages.last().map(|m| m.id.clone()),
            )
        };
        self.persist_user_message(&session_id, state, clarification_answers)
            .await;

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

        let messages = self.prepare_context(state, clarification_answers).await;

        let loop_result = self
            .agent_loop
            .run(
                &mut messages.clone(),
                None,
                &session_id,
                parent_id.as_deref(),
                &self.default_model,
            )
            .await;

        let mut clarification_tokens: Option<TokenUsage> = None;
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
                let formatted = format_clarification_questions(std::slice::from_ref(&question_obj));
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
        message: &str,
    ) {
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
