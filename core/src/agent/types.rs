//! 智能体类型定义。
//!
//! 本模块包含 Agent 使用的核心数据类型和流式事件发送器。

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::common::error::Result;
use crate::common::types::TokenUsage;
use crate::context::RetrievalTrace;

/// 追问问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    pub question: String,
    pub question_type: QuestionType,
    pub options: Option<Vec<String>>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionType {
    #[serde(rename = "OpenEnded")]
    OpenEnded,
    #[serde(rename = "Choice")]
    Choice,
    #[serde(rename = "Confirmation")]
    Confirmation,
}

/// 用于跟踪执行状态的智能体状态。
#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub initialized: bool,
    pub conversations_processed: usize,
    pub total_tokens: usize,
    pub retrievals_performed: usize,
    pub skills_executed: usize,
}

/// 智能体的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub content: String,
    pub is_complete: bool,
    pub retrieval_trace: Option<RetrievalTrace>,
    pub context_uris: Vec<String>,
    pub token_usage: TokenUsage,
    pub skill_calls: Vec<SkillCallInfo>,
    pub processing_time_ms: u64,
    pub needs_clarification: bool,
    pub clarification_questions: Vec<ClarificationQuestion>,
}

impl AgentResponse {
    /// 创建简单回答响应。
    ///
    /// - `content` - 回答内容
    /// - returns: AgentResponse 实例
    pub fn simple(content: String) -> Self {
        Self {
            content,
            is_complete: true,
            retrieval_trace: None,
            context_uris: vec![],
            token_usage: TokenUsage::default(),
            skill_calls: vec![],
            processing_time_ms: 0,
            needs_clarification: false,
            clarification_questions: vec![],
        }
    }

    /// 创建追问响应。
    ///
    /// - `questions` - 追问问题列表
    /// - `content` - 格式化后的追问内容
    /// - returns: AgentResponse 实例
    pub fn clarification(questions: Vec<ClarificationQuestion>, content: String) -> Self {
        Self {
            content,
            is_complete: true,
            retrieval_trace: None,
            context_uris: vec![],
            token_usage: TokenUsage::default(),
            skill_calls: vec![],
            processing_time_ms: 0,
            needs_clarification: true,
            clarification_questions: questions,
        }
    }

    /// 创建错误响应。
    ///
    /// - `message` - 错误消息
    /// - returns: AgentResponse 实例
    pub fn error(message: String) -> Self {
        Self {
            content: message,
            is_complete: true,
            retrieval_trace: None,
            context_uris: vec![],
            token_usage: TokenUsage::default(),
            skill_calls: vec![],
            processing_time_ms: 0,
            needs_clarification: false,
            clarification_questions: vec![],
        }
    }
}

/// 技能调用信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCallInfo {
    pub skill_id: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

/// 流式响应块的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StreamChunkType {
    Thought,
    ToolCall,
    Observation,
    #[default]
    Answer,
    Error,
    Clarification,
}

/// 流式响应块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStreamChunk {
    pub delta: String,
    pub is_complete: bool,
    pub token_usage: Option<TokenUsage>,
    #[serde(default)]
    pub chunk_type: StreamChunkType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_calls: Option<Vec<SkillCallInfo>>,
}

/// 流式事件发送器（用于在Planner-Executor循环中实时推送事件）。
#[derive(Clone)]
pub struct StreamEventSender {
    tx: mpsc::Sender<Result<AgentStreamChunk>>,
}

impl StreamEventSender {
    pub fn new(tx: mpsc::Sender<Result<AgentStreamChunk>>) -> Self {
        Self { tx }
    }

    /// 发送思考开始事件。
    pub async fn send_thought(&self, content: &str) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: content.to_string(),
                is_complete: false,
                token_usage: None,
                chunk_type: StreamChunkType::Thought,
                skill_calls: None,
            }))
            .await;
    }

    /// 发送工具调用事件。
    pub async fn send_tool_call(&self, description: &str) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: description.to_string(),
                is_complete: false,
                token_usage: None,
                chunk_type: StreamChunkType::ToolCall,
                skill_calls: None,
            }))
            .await;
    }

    /// 发送观察结果事件。
    pub async fn send_observation(&self, content: &str) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: content.to_string(),
                is_complete: false,
                token_usage: None,
                chunk_type: StreamChunkType::Observation,
                skill_calls: None,
            }))
            .await;
    }

    /// 发送回答片段事件。
    pub async fn send_answer_delta(&self, delta: &str) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: delta.to_string(),
                is_complete: false,
                token_usage: None,
                chunk_type: StreamChunkType::Answer,
                skill_calls: None,
            }))
            .await;
    }

    /// 发送最终完成事件。
    pub async fn send_complete(
        &self,
        content: &str,
        chunk_type: StreamChunkType,
        skill_calls: Option<Vec<SkillCallInfo>>,
    ) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: content.to_string(),
                is_complete: true,
                token_usage: None,
                chunk_type,
                skill_calls,
            }))
            .await;
    }

    /// 发送错误事件。
    pub async fn send_error(&self, error: &str) {
        let _ = self
            .tx
            .send(Ok(AgentStreamChunk {
                delta: error.to_string(),
                is_complete: true,
                token_usage: None,
                chunk_type: StreamChunkType::Error,
                skill_calls: None,
            }))
            .await;
    }
}
