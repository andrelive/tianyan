//! 会话状态管理模块。
//!
//! 本模块提供会话状态容器，统一管理会话的所有状态：
//! - 对话历史（用户和助手的问答）
//! - 执行历史（Planner-Executor 循环的结果）
//! - 当前目标
//! - 待处理的追问
//! - 最后活动时间
//!
//! # 架构设计
//!
//! SessionState 是 Planner-Executor 架构的核心状态容器，于 2026-03 引入。
//! 它统一管理会话的运行时状态，支持多轮迭代和上下文管理。
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    SessionState                              │
//! ├─────────────────────────────────────────────────────────────┤
//! │  conversation: Vec<Message>          (对话历史)              │
//! │  execution_context: ContextManager   (执行历史)              │
//! │  current_goal: Option<String>        (当前目标)              │
//! │  pending_clarification: Option<...>  (待处理追问)            │
//! │  last_activity: Instant              (最后活动时间)          │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//!
//! ## 基本使用
//!
//! ```rust,no_run
//! use tianyan::agent::session_state::SessionState;
//!
//! // 创建会话状态
//! let mut state = SessionState::new("session-001");
//!
//! // 添加对话历史
//! state.add_user_message("帮我分析项目性能问题");
//! state.add_assistant_message("好的，让我先查看项目结构");
//!
//! // 构建 Prompt 上下文
//! let prompt = state.build_prompt_context("继续分析");
//! ```
//!
//! ## Planner-Executor 循环
//!
//! ```rust,no_run
//! use tianyan::agent::session_state::SessionState;
//! use tianyan::planner::types::{Plan, Step, StepResult, Action, FailureHandling};
//! use serde_json::json;
//!
//! let mut state = SessionState::new("session-001");
//!
//! // 第 1 轮：Planner 生成计划
//! let plan = Plan::Steps(vec![
//!     Step {
//!         step_id: 1,
//!         description: "读取 Cargo.toml".to_string(),
//!         action: Action::ReadFile { path: "Cargo.toml".to_string() },
//!         expected_importance: 0.8,
//!         on_failure: FailureHandling::Ignore,
//!     },
//!     Step {
//!         step_id: 2,
//!         description: "运行性能测试".to_string(),
//!         action: Action::ExecuteCommand {
//!             command: "cargo bench".to_string(),
//!             cwd: None,
//!             timeout_secs: None,
//!         },
//!         expected_importance: 0.9,
//!         on_failure: FailureHandling::Abort {
//!             error_message: "无法运行 benchmark".to_string(),
//!         },
//!     },
//! ]);
//!
//! // Executor 执行步骤（伪代码）
//! let results = vec![
//!     StepResult {
//!         step_id: 1,
//!         success: true,
//!         output: json!({"content": "[package]\nname = \"my-project\""}),
//!         error: None,
//!         actual_importance: Some(0.85),
//!     },
//!     StepResult {
//!         step_id: 2,
//!         success: true,
//!         output: json!({"stdout": "bench result...", "exit_code": 0}),
//!         error: None,
//!         actual_importance: Some(0.95),
//!     },
//! ];
//!
//! // 更新执行历史
//! state.add_turn(plan, results);
//!
//! // 第 2 轮：基于执行结果继续规划
//! let prompt = state.build_prompt_context("根据测试结果分析性能瓶颈");
//! // prompt 包含：
//! // - 对话历史（2 条消息）
//! // - 执行历史（1 轮，包含 2 个步骤的执行结果）
//! // - 当前输入（"根据测试结果分析性能瓶颈"）
//! ```
//!
//! ## 使用 SessionStateManager
//!
//! ```rust,ignore
//! use tianyan::agent::session_state::SessionStateManager;
//!
//! let manager = SessionStateManager::new();
//!
//! // 创建或获取会话状态
//! let session_id = manager
//!     .with_state("session-001", |state| state.session_id.clone())
//!     .await;
//!
//! // 清理过期会话（超过 30 分钟不活动）
//! manager.cleanup_expired(1800).await;
//! ```
//!
//! # 与 SessionManager 的关系
//!
//! SessionState 与 SessionManager 是两个不同层次的组件：
//!
//! - **SessionState**：内存中的运行时状态，包含执行上下文，支持 Planner-Executor 循环
//! - **SessionManager**：VFS 持久化的会话管理器，负责长期存储和检索
//!
//! ```text
//! SessionStateManager (运行时，内存中)
//!     └─ SessionState
//!         ├─ conversation: Vec<Message>
//!         ├─ execution_context: ContextManager
//!         └─ current_goal: Option<String>
//!
//! PersistentSessionManager (持久化，VFS)
//!     └─ Session
//!         └─ messages: Vec<Message>
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::common::types::{Message, MessageRole};
use crate::context::types::ContextWindow;
use crate::planner::types::ClarificationQuestion;
use crate::session::Session;

const MAX_CONVERSATION_MESSAGES: usize = 100;
const KEEP_RECENT_MESSAGES: usize = 50;

/// 会话状态容器。
///
/// 统一管理会话的所有状态，包括对话历史、执行历史、当前目标等。
/// 这是 Planner-Executor 架构中唯一的状态容器，替代了之前的 ConversationContext。
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 会话 ID。
    pub session_id: String,

    /// 对话历史（用户和助手的问答）。
    pub conversation: Vec<Message>,

    /// 执行历史（Planner-Executor 循环的结果）。
    pub execution_history: Vec<String>,

    /// 当前目标。
    pub current_goal: Option<String>,

    /// 待处理的追问（如果有）。
    pub pending_clarification: Option<Vec<ClarificationQuestion>>,

    /// 最后活动时间。
    pub last_activity: Instant,

    /// 当前会话的上下文窗口。
    pub context_window: Option<ContextWindow>,

    /// 待持久化的记忆条目
    pub pending_memories: Vec<crate::common::types::MemoryEntry>,

    /// Token 使用量。
    pub total_tokens: usize,

    /// 会话开始时间。
    pub start_time: Instant,
}

impl SessionState {
    /// 创建新的会话状态。
    ///
    /// - `session_id` - 会话 ID
    /// - returns: 新的会话状态实例
    pub fn new(session_id: &str) -> Self {
        let now = Instant::now();
        Self {
            session_id: session_id.to_string(),
            conversation: Vec::new(),
            execution_history: Vec::new(),
            current_goal: None,
            pending_clarification: None,
            last_activity: now,
            context_window: None,
            pending_memories: Vec::new(),
            total_tokens: 0,
            start_time: now,
        }
    }

    /// 添加用户消息。
    ///
    /// - `content` - 消息内容
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.conversation.push(Message::user(content));
        self.last_activity = Instant::now();
    }

    /// 添加助手回复。
    ///
    /// - `content` - 回复内容
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.conversation.push(Message::assistant(content));
        self.last_activity = Instant::now();
    }

    /// 添加消息到历史记录。
    ///
    /// - `message` - 消息
    pub fn add_message(&mut self, message: Message) {
        self.conversation.push(message);
        self.trim_conversation();
        self.last_activity = Instant::now();
    }

    /// 获取最近 N 条消息。
    ///
    /// - `n` - 要获取的消息数量
    /// - returns: 最近 N 条消息
    pub fn recent_messages(&self, n: usize) -> Vec<Message> {
        self.conversation
            .iter()
            .rev()
            .take(n)
            .rev()
            .cloned()
            .collect()
    }

    /// 获取消息总数。
    ///
    /// - returns: 消息数量
    pub fn message_count(&self) -> usize {
        self.conversation.len()
    }

    /// 转换为 Session 用于记忆持久化。
    ///
    /// - returns: Session 实例
    pub fn to_session(&self) -> Session {
        let mut session = Session::new(&self.session_id);
        for msg in &self.conversation {
            match msg.role {
                MessageRole::User => session.add_user_message(&msg.content),
                MessageRole::Assistant => session.add_assistant_message(&msg.content),
                MessageRole::System => session.add_system_message(&msg.content),
                MessageRole::Tool => {}
            }
        }
        session
    }

    /// 添加执行轮次。
    ///
    /// # Arguments
    /// * `plan` - 该轮的计划
    /// * `results` - 执行结果
    #[allow(dead_code)]
    pub fn add_turn(&mut self, _plan: &str, _results: &[&str]) {
        // TODO: 重构后恢复
    }

    /// 构建完整的 Prompt 上下文。
    ///
    /// # Arguments
    /// * `current_input` - 当前用户输入
    ///
    /// # Returns
    /// 完整的 Prompt 上下文字符串
    pub fn build_prompt_context(&self, current_input: &str) -> String {
        if let Some(ref window) = self.context_window {
            crate::context::assembly::assemble_prompt(
                window,
                &self.conversation,
                current_input,
            )
        } else {
            format!("## 当前输入\n\n{}\n\n## 你的执行计划\n", current_input)
        }
    }

    /// 清理过期状态（防止内存泄漏）。
    ///
    /// 对话历史保留最近 MAX_CONVERSATION_MESSAGES 条，执行历史按权重清理。
    pub fn cleanup(&mut self) {
        self.trim_conversation();
    }

    /// 裁剪对话历史，保留系统消息和最近的消息。
    ///
    /// 始终保留第一条系统消息（如有），然后保留最近的 KEEP_RECENT_MESSAGES 条消息。
    fn trim_conversation(&mut self) {
        if self.conversation.len() <= MAX_CONVERSATION_MESSAGES {
            return;
        }

        let has_system_msg =
            !self.conversation.is_empty() && self.conversation[0].role == MessageRole::System;

        let start = if has_system_msg {
            let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
            if keep_from <= 1 {
                return;
            }
            1
        } else {
            let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
            if keep_from == 0 {
                return;
            }
            0
        };

        let keep_from = self.conversation.len().saturating_sub(KEEP_RECENT_MESSAGES);
        if keep_from > start {
            self.conversation.drain(start..keep_from);
        }
    }

    /// 获取对话历史。
    ///
    /// # Returns
    /// 对话历史引用
    pub fn get_conversation(&self) -> &[Message] {
        &self.conversation
    }

    /// 获取执行历史。
    ///
    /// # Returns
    /// 执行历史引用
    pub fn get_execution_history(&self) -> &[String] {
        &self.execution_history
    }

    /// 创建 Planner 运行时上下文（只读快照）。
    ///
    /// 将 SessionState 转换为 PlannerContext，使 Planner 不直接依赖 SessionState。
    ///
    /// - returns: Planner 运行时上下文
    #[allow(dead_code)]
    pub fn to_planner_context(&self) -> String {
        // TODO: 重构后恢复
        String::new()
    }

    /// 应用 Planner 返回的状态变更。
    ///
    /// Planner 不直接修改 SessionState，而是返回变更列表。
    /// 此方法将这些变更应用到当前会话状态。
    ///
    /// - `mutations` - Planner 产生的状态变更列表
    #[allow(dead_code)]
    pub fn apply_mutations(&mut self, _mutations: Vec<String>) {
        // TODO: 重构后恢复
    }
}

/// 会话状态管理器。
///
/// 管理多个会话的状态，提供线程安全的访问。
#[derive(Debug, Clone)]
pub struct SessionStateManager {
    /// 所有会话状态。
    states: Arc<RwLock<HashMap<String, SessionState>>>,
}

impl SessionStateManager {
    /// 创建新的会话状态管理器。
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取或创建会话状态（通过闭包操作）。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    /// * `f` - 操作函数
    ///
    /// # Returns
    /// 操作结果
    pub async fn with_state<F, R>(&self, session_id: &str, f: F) -> R
    where
        F: FnOnce(&mut SessionState) -> R,
    {
        let mut states = self.states.write().await;
        let state = states
            .entry(session_id.to_string())
            .or_insert_with(|| SessionState::new(session_id));
        f(state)
    }

    /// 读取会话状态（通过闭包操作）。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    /// * `f` - 读取函数
    ///
    /// # Returns
    /// 读取结果
    pub async fn with_state_read<F, R>(&self, session_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&SessionState) -> R,
    {
        let states = self.states.read().await;
        states.get(session_id).map(f)
    }

    /// 删除会话状态。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    pub async fn remove(&self, session_id: &str) {
        let mut states = self.states.write().await;
        states.remove(session_id);
    }

    /// 获取所有会话 ID。
    ///
    /// # Returns
    /// 会话 ID 列表
    pub async fn get_all_session_ids(&self) -> Vec<String> {
        let states = self.states.read().await;
        states.keys().cloned().collect()
    }

    /// 清理所有过期会话。
    ///
    /// # Arguments
    /// * `max_inactive_duration` - 最大不活动时间（秒）
    pub async fn cleanup_expired(&self, max_inactive_duration_secs: u64) {
        let mut states = self.states.write().await;
        let duration = std::time::Duration::from_secs(max_inactive_duration_secs);

        states.retain(|_, state| state.last_activity.elapsed() < duration);
    }
}

impl Default for SessionStateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state_new() {
        let state = SessionState::new("test-session");
        assert_eq!(state.session_id, "test-session");
        assert!(state.conversation.is_empty());
        assert!(state.current_goal.is_none());
        assert!(state.pending_clarification.is_none());
    }

    #[test]
    fn test_session_state_add_messages() {
        let mut state = SessionState::new("test-session");

        state.add_user_message("Hello");
        assert_eq!(state.conversation.len(), 1);
        assert_eq!(
            state.conversation[0].role,
            crate::common::types::MessageRole::User
        );

        state.add_assistant_message("Hi there!");
        assert_eq!(state.conversation.len(), 2);
        assert_eq!(
            state.conversation[1].role,
            crate::common::types::MessageRole::Assistant
        );
    }

    #[test]
    fn test_session_state_add_turn() {
        let mut state = SessionState::new("test-session");

        state.add_turn("test-plan", &["result1"]);
        assert_eq!(state.execution_history.len(), 0);
    }

    #[test]
    fn test_session_state_cleanup() {
        let mut state = SessionState::new("test-session");

        for i in 0..101 {
            state.add_user_message(format!("Message {}", i));
        }

        state.cleanup();
        assert!(state.conversation.len() <= KEEP_RECENT_MESSAGES);
        assert!(!state.conversation.is_empty());
    }

    #[test]
    fn test_session_state_add_message() {
        let mut state = SessionState::new("test-session");
        state.add_message(Message::user("Hello"));
        assert_eq!(state.conversation.len(), 1);
        assert_eq!(state.message_count(), 1);
    }

    #[test]
    fn test_session_state_recent_messages() {
        let mut state = SessionState::new("test-session");
        for i in 0..5 {
            state.add_user_message(format!("Message {}", i));
        }
        let recent = state.recent_messages(3);
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_session_state_to_session() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        state.add_assistant_message("Hi");

        let session = state.to_session();
        assert_eq!(session.session_id, "test-session");
    }

    #[tokio::test]
    async fn test_session_state_manager() {
        let manager = SessionStateManager::new();

        // 测试 with_state（创建）
        let session_id = manager
            .with_state("session1", |state| state.session_id.clone())
            .await;
        assert_eq!(session_id, "session1");

        // 测试 with_state_read（读取）
        let exists = manager.with_state_read("session1", |_| true).await;
        assert!(exists.is_some());

        // 测试 remove
        manager.remove("session1").await;

        let exists_after_remove = manager.with_state_read("session1", |_| true).await;
        assert!(exists_after_remove.is_none());
    }

    #[test]
    fn test_session_state_build_prompt_context() {
        let mut state = SessionState::new("test-session");
        state.add_user_message("Hello");
        state.add_assistant_message("Hi there!");

        // 无 ContextWindow 时回退为最小化 prompt
        let prompt = state.build_prompt_context("What's next?");
        assert!(prompt.contains("## 当前输入"), "应包含当前输入部分");
        assert!(prompt.contains("What's next"), "应包含当前输入内容");

        // 设置 ContextWindow 后包含完整对话历史
        use crate::context::types::ContextWindow;
        state.context_window = Some(ContextWindow::new("测试系统提示".to_string()));
        let prompt = state.build_prompt_context("What's next?");
        assert!(prompt.contains("测试系统提示"), "应包含系统提示词");
        assert!(prompt.contains("## 对话历史"), "应包含对话历史部分");
        assert!(prompt.contains("Hello"), "应包含用户消息");
        assert!(prompt.contains("Hi there"), "应包含助手消息");
    }

    #[tokio::test]
    async fn test_session_state_manager_cleanup() {
        let manager = SessionStateManager::new();

        // 创建会话
        manager.with_state("session1", |_| {}).await;

        // 清理过期会话（0 秒，即立即清理）
        manager.cleanup_expired(0).await;

        // 验证会话已被清理
        let session_ids = manager.get_all_session_ids().await;
        assert!(session_ids.is_empty());
    }
}
