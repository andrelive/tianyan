use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;

use crate::common::types::Message;
use crate::context::types::ContextWindow;

// 从 executor 层导入执行契约类型（依赖反转）
pub use crate::executor::Action;

/// 执行步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// 步骤 ID（在同轮计划中唯一）
    pub step_id: usize,
    /// 步骤描述（给 LLM 看的，帮助理解这个步骤的目的）
    pub description: String,
    /// 具体执行的动作
    pub action: Action,
    /// 失败处理策略
    pub on_failure: FailureHandling,
    /// Planner 预判的重要性（0-1）
    pub expected_importance: f32,
}

/// 失败处理策略
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FailureHandling {
    /// 忽略，继续执行
    #[serde(rename = "Ignore")]
    Ignore,
    /// 报错，终止执行
    #[serde(rename = "Abort")]
    Abort {
        #[serde(rename = "error_message")]
        error_message: String,
    },
    /// 重试
    #[serde(rename = "Retry")]
    Retry {
        #[serde(rename = "max_retries")]
        max_retries: usize,
    },
}

/// 步骤执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// 对应的步骤 ID
    pub step_id: usize,
    /// 是否执行成功
    pub success: bool,
    /// 执行结果（JSON 格式）
    pub output: Value,
    /// 错误信息（如果执行失败）
    pub error: Option<String>,
    /// 实际重要性（可选，用于 Executor 修正）
    pub actual_importance: Option<f32>,
}

/// Planner 运行的输出结果。
///
/// 区分正常输出（回答和追问）与异常错误（由 PlannerError 处理）。
/// 追问是正常的交互流程，不应通过错误类型表示。
#[derive(Debug, Clone)]
pub enum PlannerOutput {
    /// 最终回答
    Answer(String),
    /// 需要追问（信息不足）
    Clarification {
        questions: Vec<ClarificationQuestion>,
        missing_info: Vec<String>,
    },
}

/// 单轮交互
#[derive(Debug, Clone, serde::Serialize)]
pub struct Turn {
    pub turn_id: usize,
    /// 该轮的计划（用于调试和日志）
    pub plan: Plan,
    /// 执行结果
    pub results: Vec<StepResult>,
    #[serde(skip)]
    pub timestamp: Instant,
}

/// Planner 输出的计划
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Plan {
    /// 直接回答（简单知识性问题）
    DirectAnswer { content: String, confidence: f32 },

    /// 追问（信息不足）
    Clarification {
        questions: Vec<ClarificationQuestion>,
        missing_info: Vec<String>,
    },

    /// 执行步骤（需要工具调用）
    /// - 1 个步骤：简单任务，执行后进入下一轮迭代
    /// - 多个步骤：并行执行，所有结果放入上下文后进入下一轮迭代
    Steps(Vec<Step>),
}

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
    /// 开放式
    #[serde(rename = "OpenEnded")]
    OpenEnded,
    /// 选择式
    #[serde(rename = "Choice")]
    Choice,
    /// 确认式
    #[serde(rename = "Confirmation")]
    Confirmation,
}

/// Planner 运行时上下文（只读快照，解耦自 SessionState）。
///
/// 包含 Planner 生成计划所需的所有信息，不依赖 SessionState 的具体实现。
/// 由 SessionState::to_planner_context() 构造，在调用 Planner 前创建。
#[derive(Debug, Clone)]
pub struct PlannerContext {
    /// 对话历史
    pub conversation: Vec<Message>,
    /// 执行轮次历史
    pub execution_turns: Vec<Turn>,
    /// 上下文窗口（由管线填充）
    pub context_window: Option<ContextWindow>,
}

impl PlannerContext {
    /// 构建完整的 Prompt 上下文。
    ///
    /// - `current_input` - 当前用户输入
    /// - returns: 完整的 Prompt 上下文字符串
    pub fn build_prompt(&self, current_input: &str) -> String {
        if let Some(ref window) = self.context_window {
            crate::context::assembly::assemble_prompt(
                window,
                &self.conversation,
                &self.execution_turns,
                current_input,
            )
        } else {
            format!("## 当前输入\n\n{}\n\n## 你的执行计划\n", current_input)
        }
    }

    /// 构建执行上下文摘要（JSON 格式）。
    ///
    /// - returns: 执行历史摘要
    pub fn build_execution_summary(&self) -> Value {
        let turns: Vec<Value> = self
            .execution_turns
            .iter()
            .map(|t| {
                serde_json::json!({
                    "turn_id": t.turn_id,
                    "results": t.results.iter().map(|r| {
                        serde_json::json!({
                            "step_id": r.step_id,
                            "success": r.success,
                            "output": r.output,
                            "error": r.error,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        Value::Array(turns)
    }
}

/// Planner 在运行过程中产生的状态变更。
///
/// Planner 不直接修改 SessionState，而是返回变更列表，
/// 由调用方（Agent）应用到 SessionState。
#[derive(Debug, Clone)]
pub enum PlannerMutation {
    /// 记录一轮执行
    AddTurn(Plan, Vec<StepResult>),
    /// 添加助手消息
    AddAssistantMessage(String),
    /// 设置待处理的追问
    SetPendingClarification(Vec<ClarificationQuestion>),
}

/// Planner 错误类型
#[derive(thiserror::Error, Debug, Clone)]
pub enum PlannerError {
    /// 达到最大递归深度
    #[error("达到最大递归深度: {0}")]
    MaxDepthExceeded(usize),

    /// LLM 调用失败
    #[error("LLM 调用失败: {0}")]
    LlmCallFailed(String),

    /// 解析 LLM 输出失败
    #[error("解析 LLM 输出失败: {message}, 原始输出: {raw_output}")]
    ParseError { message: String, raw_output: String },

    /// 执行步骤失败（可恢复，触发重规划）
    #[error("执行步骤 {step_id} 失败: {error}")]
    StepExecutionFailed { step_id: usize, error: String },

    /// 达到最大迭代次数
    #[error("达到最大迭代次数：{0}")]
    MaxIterationsReached(usize),

    /// 达到最大重规划次数
    #[error("达到最大重规划次数：{0}")]
    MaxRecoveryAttemptsReached(usize),

    /// 用户中断
    #[error("用户中断")]
    UserInterrupted,

    /// 配置错误
    #[error("配置错误: {0}")]
    ConfigError(String),

    /// 其他错误
    #[error("其他错误: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_error_display() {
        let err = PlannerError::MaxDepthExceeded(5);
        assert!(err.to_string().contains("达到最大递归深度"));
        assert!(err.to_string().contains("5"));

        let err = PlannerError::LlmCallFailed("连接失败".to_string());
        assert!(err.to_string().contains("LLM 调用失败"));
        assert!(err.to_string().contains("连接失败"));
    }
}
