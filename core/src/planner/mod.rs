//! 天演智能体系统的任务规划器。
//!
//! 本模块实现基于 LLM 的任务规划器，采用 Planner-Executor 分离架构。
//!
//! # 架构设计
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        Planner                               │
//! │  (LLM 驱动，负责意图理解、信息收集、计划生成)                   │
//! │                                                              │
//! │  输入：Goal + SessionState + WorkingMemory                   │
//! │  输出：Plan (DirectAnswer / Clarification / Steps)           │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!                             ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                       Executor                               │
//! │  (纯机械执行，无思考能力)                                      │
//! │                                                              │
//! │  输入：Steps                                                 │
//! │  输出：Vec<StepResult>                                       │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 使用示例
//!
//! ## 基本使用
//!
//! ```rust,no_run
//! use tianyan::planner::Planner;
//! use tianyan::planner::types::PlannerOutput;
//! use tianyan::agent::session_state::SessionState;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 创建 Planner 需要模型服务，此处省略
//!     // let mut planner = Planner::new(0, model_service);
//!     // let mut state = SessionState::new("session-001");
//!     // state.add_user_message("帮我分析项目性能问题");
//!     // let result = planner.run("开始分析".to_string(), &mut state).await?;
//!     // match result {
//!     //     PlannerOutput::Answer(content) => println!("回答：{}", content),
//!     //     PlannerOutput::Clarification { questions, .. } => println!("需要追问"),
//!     // }
//!     Ok(())
//! }
//! ```
//!
//! ## Planner 迭代循环
//!
//! Planner 会自动执行迭代循环，直到生成 DirectAnswer 或达到最大迭代次数：
//!
//! ```text
//! 用户输入 → Planner 第 1 轮 → Plan::Steps([...])
//!              ↓
//!         Executor 执行步骤
//!              ↓
//!         SessionState.add_turn(plan, results)
//!              ↓
//!         Planner 第 2 轮（看到执行结果）
//!              ↓
//!         Plan::Steps([...]) 或 Plan::DirectAnswer(...)
//! ```
//!
//! ## 三种计划类型
//!
//! ### 1. DirectAnswer（直接回答）
//!
//! 适用于简单知识性问题：
//!
//! ```rust
//! use tianyan::planner::types::Plan;
//!
//! let plan = Plan::DirectAnswer {
//!     content: "Vec 是动态数组，VecDeque 是双端队列...".to_string(),
//!     confidence: 0.95,
//! };
//! ```
//!
//! ### 2. Clarification（追问）
//!
//! 当信息不足时，Planner 会输出追问：
//!
//! ```rust
//! use tianyan::planner::types::{Plan, ClarificationQuestion, QuestionType};
//!
//! let plan = Plan::Clarification {
//!     questions: vec![
//!         ClarificationQuestion {
//!             question: "您想优化哪个文件？".to_string(),
//!             question_type: QuestionType::OpenEnded,
//!             options: None,
//!             required: true,
//!         },
//!         ClarificationQuestion {
//!             question: "主要关注什么指标？".to_string(),
//!             question_type: QuestionType::Choice,
//!             options: Some(vec!["执行速度".into(), "内存占用".into(), "代码可读性".into()]),
//!             required: true,
//!         },
//!     ],
//!     missing_info: vec!["target_file".into(), "optimization_goal".into()],
//! };
//! ```
//!
//! ### 3. Steps（执行步骤）
//!
//! 需要工具调用时，Planner 生成执行步骤：
//!
//! ```rust
//! use tianyan::planner::types::{Plan, Step, Action, FailureHandling};
//!
//! let plan = Plan::Steps(vec![
//!     Step {
//!         step_id: 1,
//!         description: "读取项目配置文件".to_string(),
//!         action: Action::ReadFile {
//!             path: "Cargo.toml".to_string(),
//!         },
//!         expected_importance: 0.8,
//!         on_failure: FailureHandling::Ignore,
//!     },
//!     Step {
//!         step_id: 2,
//!         description: "运行性能基准测试".to_string(),
//!         action: Action::ExecuteCommand {
//!             command: "cargo bench".to_string(),
//!             cwd: Some("/work/project".to_string()),
//!             timeout_secs: None,
//!         },
//!         expected_importance: 0.9,
//!         on_failure: FailureHandling::Abort {
//!             error_message: "无法运行 benchmark".to_string(),
//!         },
//!     },
//! ]);
//! ```
//!
//! ## 递归规划（子任务）
//!
//! Planner 可以使用 SubPlanner 动作分解复杂任务（递归深度由 PlannerConfig::max_recursion_depth 控制）：
//!
//! ```rust,ignore
//! use tianyan::planner::types::{Plan, Step, Action, FailureHandling};
//!
//! let plan = Plan::Steps(vec![
//!     Step {
//!         step_id: 1,
//!         description: "列出所有 Rust 源文件".to_string(),
//!         action: Action::ExecuteCommand {
//!             command: "find . -name '*.rs'".to_string(),
//!             cwd: None,
//!             timeout_secs: None,
//!         },
//!         expected_importance: 0.7,
//!         on_failure: FailureHandling::Abort {
//!             error_message: "无法列出文件".to_string(),
//!         },
//!     },
//! ]);
//! ```
//!
//! # 配置说明
//!
//! Planner 的默认配置：
//!
//! ```rust
//! use tianyan::planner::config::PlannerConfig;
//!
//! let config = PlannerConfig::default();
//!
//! assert_eq!(config.max_iterations, 10);  // 最大迭代次数
//! assert_eq!(config.max_depth, 5);        // 最大递归深度
//! assert_eq!(config.context_config.age_decay_factor, 0.9);
//! assert_eq!(config.context_config.cleanup_threshold, 0.3);
//! ```
//!
//! # 错误处理
//!
//! Planner 可能返回的错误：
//!
//! - `PlannerError::MaxDepthExceeded` - 超过最大递归深度
//! - `PlannerError::StepExecutionFailed` - 步骤执行失败
//! - `PlannerError::LlmCallFailed` - LLM 调用失败
//!
//! 追问（Clarification）通过 `PlannerOutput::Clarification` 正常返回，不使用错误类型。
//!
//! # 与 Coordinator 集成
//!
//! Planner 通常由 Coordinator 调用，Coordinator 负责管理会话状态和错误处理：
//!
//! ```rust,no_run
//! use tianyan::planner::Planner;
//! use tianyan::planner::types::{PlannerContext, PlannerOutput};
//!
//! async fn handle_chat(
//!     planner: &mut Planner,
//!     ctx: &PlannerContext,
//!     user_input: &str,
//! ) -> Result<String, String> {
//!     match planner.run(user_input.to_string(), ctx).await {
//!         Ok((PlannerOutput::Answer(content), _mutations)) => Ok(content),
//!         Ok((PlannerOutput::Clarification { questions, .. }, _mutations)) => {
//!             // 展示问题给用户，等待回答
//!             todo!()
//!         }
//!         Err(e) => Err(format!("Planner 执行失败：{}", e)),
//!     }
//! }
//! ```

use std::sync::Arc;

use crate::executor::types::{Action, StepResult};
use crate::executor::{Executor, ExecutorTrait};
use crate::model::ChatCompletionRequest;
use crate::model::ModelService;
use crate::planner::config::PlannerConfig;
use crate::planner::types::PlannerError;
use crate::planner::types::{Plan, PlannerContext, PlannerMutation, PlannerOutput};
use crate::skills::SkillExecutor;

pub mod config;
pub mod context;
pub mod types;

/// Planner 的抽象 trait，用于测试和可替换性。
///
/// 提取此 trait 使得 Agent 可以依赖 trait 而非具体类型，
/// 便于单元测试 mock 和未来的 Planner 替代实现。
///
/// Planner 不直接依赖 SessionState，而是使用 PlannerContext（只读快照）
/// 并返回 PlannerMutation 列表，由调用方应用到 SessionState。
#[async_trait::async_trait]
pub trait PlannerTrait: Send {
    /// 运行 Planner（非流式）。
    ///
    /// - `input` - 用户输入
    /// - `ctx` - Planner 运行时上下文
    /// - returns: 规划输出和状态变更列表
    async fn run(
        &mut self,
        input: String,
        ctx: &PlannerContext,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError>;

    /// 运行 Planner（带流式事件回调）。
    ///
    /// - `input` - 用户输入
    /// - `ctx` - Planner 运行时上下文
    /// - `stream_sender` - 可选的流式事件发送器
    /// - returns: 规划输出和状态变更列表
    async fn run_with_stream(
        &mut self,
        input: String,
        ctx: &PlannerContext,
        stream_sender: Option<crate::agent::StreamEventSender>,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError>;
}

/// Planner 结构体。
///
/// 基于 LLM 的任务规划器，负责意图理解、信息收集和计划生成。
#[derive(Clone)]
pub struct Planner {
    depth: usize,
    config: PlannerConfig,
    executor: Arc<dyn ExecutorTrait>,
    model_service: Arc<dyn ModelService>,
    skill_executor: Option<Arc<SkillExecutor>>,
}

impl Planner {
    /// 创建新的 Planner。
    ///
    /// - `depth` - 当前递归深度
    /// - `model_service` - ModelService trait 对象（通常由 ModelRouter 实现）
    /// - `config` - Planner 配置（迭代次数、并发度、重试策略等）
    pub fn new(depth: usize, model_service: Arc<dyn ModelService>, config: PlannerConfig) -> Self {
        Self {
            depth,
            executor: Arc::new(Executor::new(config.executor_max_concurrency)),
            config,
            model_service,
            skill_executor: None,
        }
    }

    /// 设置技能执行器。
    pub fn with_skill_executor(mut self, skill_executor: Arc<SkillExecutor>) -> Self {
        let executor = Executor::new(self.config.executor_max_concurrency)
            .with_skill_executor(skill_executor.clone());
        self.executor = Arc::new(executor);
        self.skill_executor = Some(skill_executor);
        self
    }

    /// 创建用于测试的 Planner
    #[cfg(test)]
    pub fn for_testing(_depth: usize) -> Self {
        unimplemented!("测试时需要提供 Mock ModelService 实现")
    }

    /// 执行子 Planner（递归规划）。
    ///
    /// 处理 Action::SubPlanner 类型的步骤。创建子 Planner 实例，
    /// 递归调用自身执行子任务，确保深度控制。
    ///
    /// - `task` - 子任务描述
    /// - returns: 子任务执行结果（模仿 StepResult 格式）
    async fn execute_sub_planner(&self, task: &str) -> StepResult {
        let sub_depth = self.depth + 1;

        let mut sub_planner =
            Planner::new(sub_depth, self.model_service.clone(), self.config.clone());

        if let Some(ref se) = self.skill_executor {
            sub_planner = sub_planner.with_skill_executor(se.clone());
        }

        let sub_ctx = PlannerContext {
            conversation: vec![],
            execution_turns: vec![],
            context_window: None,
        };

        match Box::pin(sub_planner.run(task.to_string(), &sub_ctx)).await {
            Ok((output, _mutations)) => match output {
                PlannerOutput::Answer(answer) => StepResult {
                    step_id: 0,
                    success: true,
                    output: serde_json::Value::String(answer),
                    error: None,
                    actual_importance: None,
                },
                PlannerOutput::Clarification { questions, .. } => StepResult {
                    step_id: 0,
                    success: false,
                    output: serde_json::Value::Null,
                    error: Some(format!(
                        "子 Planner 需要追问: {}",
                        questions
                            .iter()
                            .map(|q| q.question.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    actual_importance: Some(0.3),
                },
            },
            Err(e) => StepResult {
                step_id: 0,
                success: false,
                output: serde_json::Value::Null,
                error: Some(format!("子 Planner 执行失败: {}", e)),
                actual_importance: Some(0.1),
            },
        }
    }

    /// 运行 Planner（带边界检查）
    ///
    /// - `input` - 用户输入
    /// - `ctx` - Planner 运行时上下文（只读快照）
    ///
    /// - returns: `(PlannerOutput, Vec<PlannerMutation>)` 表示输出和状态变更
    ///
    /// # Errors
    /// - `PlannerError::MaxDepthExceeded` - 超过最大递归深度
    /// - `PlannerError::StepExecutionFailed` - 步骤执行失败
    /// - `PlannerError::LlmCallFailed` - LLM 调用失败
    pub async fn run(
        &mut self,
        input: String,
        ctx: &PlannerContext,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError> {
        self.run_with_stream(input, ctx, None).await
    }

    /// 运行 Planner（带流式事件回调）。
    ///
    /// 与 `run` 相同，但在关键阶段通过 `stream_sender` 发送实时事件：
    /// - 计划生成后发送 Thought 事件
    /// - 步骤执行前发送 ToolCall 事件
    /// - 步骤执行后发送 Observation 事件
    ///
    /// - `input` - 用户输入
    /// - `ctx` - Planner 运行时上下文
    /// - `stream_sender` - 可选的流式事件发送器
    pub async fn run_with_stream(
        &mut self,
        input: String,
        ctx: &PlannerContext,
        stream_sender: Option<crate::agent::StreamEventSender>,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError> {
        if self.depth >= self.config.max_depth {
            return Err(PlannerError::MaxDepthExceeded(self.depth));
        }

        let mut iteration_count = 0;
        let mut recovery_attempts = 0;
        let mut mutations: Vec<PlannerMutation> = Vec::new();

        loop {
            iteration_count += 1;

            if iteration_count > self.config.max_iterations {
                let answer = self.generate_partial_answer(ctx).await?;
                mutations.push(PlannerMutation::AddAssistantMessage(answer.clone()));
                return Ok((PlannerOutput::Answer(answer), mutations));
            }

            let prompt = ctx.build_prompt(&input);

            if let Some(ref sender) = stream_sender {
                sender.send_thought("正在分析需求并制定执行计划...").await;
            }

            let plan = self.generate_plan_from_prompt(&prompt).await?;

            match plan {
                Plan::DirectAnswer { ref content, .. } => {
                    mutations.push(PlannerMutation::AddTurn(
                        plan.clone(),
                        vec![],
                    ));
                    mutations.push(PlannerMutation::AddAssistantMessage(content.clone()));
                    return Ok((PlannerOutput::Answer(content.clone()), mutations));
                }
                Plan::Clarification {
                    ref questions,
                    ref missing_info,
                } => {
                    mutations.push(PlannerMutation::SetPendingClarification(questions.clone()));
                    return Ok((PlannerOutput::Clarification {
                        questions: questions.clone(),
                        missing_info: missing_info.clone(),
                    }, mutations));
                }
                Plan::Steps(ref steps) => {
                    if let Some(ref sender) = stream_sender {
                        let descriptions: Vec<String> = steps
                            .iter()
                            .map(|s| format!("步骤 {}: {}", s.step_id, s.description))
                            .collect();
                        sender
                            .send_tool_call(&format!(
                                "执行 {} 个步骤\n{}",
                                steps.len(),
                                descriptions.join("\n")
                            ))
                            .await;
                    }

                    // 预处理：分离 SubPlanner 步骤和普通步骤
                    let (sub_planner_steps, normal_steps): (Vec<_>, Vec<_>) = steps
                        .iter()
                        .partition(|s| matches!(s.action, Action::SubPlanner { .. }));

                    // 执行普通步骤（通过 Executor）
                    let mut results = if normal_steps.is_empty() {
                        vec![]
                    } else {
                        self.executor.execute_steps(normal_steps.into_iter().cloned().collect()).await
                    };

                    // 执行子 Planner 步骤（Planner 自身处理）
                    for step in sub_planner_steps {
                        if let Action::SubPlanner { ref task } = step.action {
                            let sub_result = self.execute_sub_planner(task).await;
                            let mut result = sub_result;
                            result.step_id = step.step_id;
                            results.push(result);
                        }
                    }

                    // 按 step_id 排序结果
                    results.sort_by_key(|r| r.step_id);

                    if let Some(ref sender) = stream_sender {
                        for result in &results {
                            let status = if result.success { "成功" } else { "失败" };
                            let summary = if result.success {
                                format!("步骤 {} 执行{}", result.step_id, status)
                            } else {
                                format!(
                                    "步骤 {} 执行{}: {}",
                                    result.step_id,
                                    status,
                                    result.error.clone().unwrap_or_default()
                                )
                            };
                            sender.send_observation(&summary).await;
                        }
                    }

                    mutations.push(PlannerMutation::AddTurn(plan.clone(), results.clone()));

                    if let Some(failed) = results.iter().find(|r| !r.success) {
                        if self.config.enable_recovery
                            && recovery_attempts < self.config.max_recovery_attempts
                        {
                            recovery_attempts += 1;
                            tracing::info!(
                                step_id = failed.step_id,
                                error = %failed.error.clone().unwrap_or_default(),
                                recovery_attempt = recovery_attempts,
                                "步骤执行失败，触发自适应重规划"
                            );

                            if let Some(ref sender) = stream_sender {
                                sender
                                    .send_thought(&format!(
                                        "步骤 {} 执行失败，正在分析原因并重新规划... (重试 {}/{})",
                                        failed.step_id,
                                        recovery_attempts,
                                        self.config.max_recovery_attempts
                                    ))
                                    .await;
                            }

                            continue;
                        }

                        return Err(PlannerError::StepExecutionFailed {
                            step_id: failed.step_id,
                            error: failed.error.clone().unwrap_or_default(),
                        });
                    }

                    continue;
                }
            }
        }
    }

    /// 调用 LLM（统一调用路径）
    async fn call_llm(&self, prompt: &str) -> Result<String, PlannerError> {
        use crate::common::types::Message;

        let request = ChatCompletionRequest::new("default", vec![Message::user(prompt)]);

        let response = self
            .model_service
            .chat_completion(request)
            .await
            .map_err(|e| PlannerError::LlmCallFailed(format!("ModelService 调用失败：{}", e)))?;

        let content = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| PlannerError::LlmCallFailed("响应为空".to_string()))?
            .message
            .content;

        Ok(content)
    }

    /// 调用 LLM 生成计划（从完整的 Prompt）
    async fn generate_plan_from_prompt(&self, prompt: &str) -> Result<Plan, PlannerError> {
        let content = self.call_llm(prompt).await?;
        parse_llm_output(&content)
    }

    /// 生成部分回答（当达到最大迭代次数时）
    async fn generate_partial_answer(&self, ctx: &PlannerContext) -> Result<String, PlannerError> {
        let summary = ctx.build_execution_summary();
        let prompt = format!(
            "已达到最大迭代次数。请基于以下执行结果，生成一个部分总结：\n\n{}",
            serde_json::to_string_pretty(&summary).unwrap_or_default()
        );

        self.call_llm(&prompt).await
    }
}

#[async_trait::async_trait]
impl PlannerTrait for Planner {
    async fn run(
        &mut self,
        input: String,
        ctx: &PlannerContext,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError> {
        self.run(input, ctx).await
    }

    async fn run_with_stream(
        &mut self,
        input: String,
        ctx: &PlannerContext,
        stream_sender: Option<crate::agent::StreamEventSender>,
    ) -> Result<(PlannerOutput, Vec<PlannerMutation>), PlannerError> {
        self.run_with_stream(input, ctx, stream_sender).await
    }
}

/// 解析 LLM 输出为 Plan 结构体。
fn parse_llm_output(output: &str) -> Result<Plan, PlannerError> {
    let cleaned = output
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let plan: Plan = serde_json::from_str(cleaned).map_err(|e| {
        let raw_output = if cleaned.len() > 500 {
            format!("{}... (截断，共 {} 字符)", &cleaned[..500], cleaned.len())
        } else {
            cleaned.to_string()
        };
        PlannerError::ParseError {
            message: format!("JSON 解析失败：{}", e),
            raw_output,
        }
    })?;

    if let Plan::Steps(steps) = &plan {
        let mut ids = std::collections::HashSet::new();
        for step in steps {
            if !ids.insert(step.step_id) {
                let raw_output = if cleaned.len() > 500 {
                    format!("{}... (截断，共 {} 字符)", &cleaned[..500], cleaned.len())
                } else {
                    cleaned.to_string()
                };
                return Err(PlannerError::ParseError {
                    message: format!("步骤 ID 重复：{}", step.step_id),
                    raw_output,
                });
            }
        }
    }

    Ok(plan)
}
