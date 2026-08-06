//! Agent 交互类工具执行器：execute_command / call_skill / ask_user / self_check /
//! delegate_to_agent。

use std::collections::HashMap;

use futures::future::BoxFuture;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
};
use crate::common::error::TianyanError;
use crate::common::types::Message;
use crate::executor::approval::ApprovalDecision;
use crate::executor::Action;
use crate::model::types::ChatCompletionRequest;
use crate::skills::SkillExecutionRequest;

use super::{parse_params, safety_violation, ToolRegistry};

impl ToolRegistry {
    /// 执行 execute_command 工具：运行 shell 命令（含安全策略 + 审批门控）。
    pub(crate) async fn execute_execute_command(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let mut params: ExecuteCommandParams = parse_params(arguments)?;

        // Check command against security policy.
        // If blocked but safety_mode is Transform, try rewriting to a safe equivalent.
        if let Err(security_err) = self.security_policy.check_command(&params.command) {
            if let Some(transformed) = self.security_policy.transform_command(&params.command) {
                tracing::info!(
                    original = %params.command,
                    transformed = %transformed,
                    "命令已由安全策略自动重写"
                );
                params.command = transformed;
            } else {
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规：{}",
                    security_err,
                )));
            }
        }

        // Path sandbox for working directory
        if let Some(ref cwd) = params.cwd {
            safety_violation(self.security_policy.check_path(std::path::Path::new(cwd)))?;
        }

        // Approval workflow check — auto-approve safe commands,
        // auto-deny critical commands (rm, format, dd), request
        // human approval for Medium/High risk commands.
        if let Some(ref approval) = self.approval_workflow {
            let action = Action::ExecuteCommand {
                command: params.command.clone(),
                cwd: params.cwd.clone(),
                timeout_secs: params.timeout_secs,
            };
            let resp = approval
                .request_approval("tool-execution", &action)
                .await
                .map_err(|e| {
                    TianyanError::Custom(format!("tool: 执行失败：审批工作流错误: {}", e))
                })?;
            if resp.decision != ApprovalDecision::Approve {
                // 记录待确认操作：用户通过"询问用户"链路批准后放行
                self.remember_pending_approval(&action).await;
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规：操作需要用户确认：{}",
                    resp.reason.unwrap_or_default()
                )));
            }
        }
        crate::executor::execute_command_action(
            &params.command,
            params.cwd.as_deref(),
            params.timeout_secs,
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 call_skill 工具：调用已注册技能。
    pub(crate) async fn execute_call_skill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: CallSkillParams = parse_params(arguments)?;
        if let Some(ref skill_executor) = self.skill_executor {
            let request = SkillExecutionRequest::new(params.skill_id, params.parameters);
            match skill_executor.execute(request).await {
                Ok(result) => {
                    let mut data = HashMap::new();
                    if let Some(output) = result.output {
                        data.insert("output".to_string(), serde_json::Value::String(output));
                    }
                    if let Some(error) = result.error {
                        data.insert("error".to_string(), serde_json::Value::String(error));
                    }
                    if let Some(exit_code) = result.exit_code {
                        data.insert(
                            "exit_code".to_string(),
                            serde_json::Value::Number(exit_code.into()),
                        );
                    }
                    data.insert(
                        "execution_time_ms".to_string(),
                        serde_json::Value::Number(result.execution_time_ms.into()),
                    );
                    data.insert(
                        "success".to_string(),
                        serde_json::Value::Bool(result.success),
                    );
                    Ok(serde_json::Value::Object(data.into_iter().collect()))
                }
                Err(e) => Err(TianyanError::Custom(format!("tool: 执行失败：{}", e))),
            }
        } else {
            Err(TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "SkillExecutor not configured",
            )))
        }
    }

    /// 执行 ask_user 工具：向用户追问。
    pub(crate) async fn execute_ask_user(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: AskUserParams = parse_params(arguments)?;
        Err(TianyanError::Custom(format!(
            "tool: 需要追问：{}",
            params.question
        )))
    }

    /// 执行 self_check 工具：查询内部指标。
    pub(crate) async fn execute_self_check(&self) -> Result<serde_json::Value, TianyanError> {
        let metrics = self.metrics.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "AgentMetrics not configured"))
        })?;
        Ok(metrics.query_harness_health().await)
    }

    /// 执行 delegate_to_agent 工具：委托子任务到隔离子 Agent。
    ///
    /// The LLM drives the loop: each turn it can either return a final answer
    /// or request tool calls.  When tools are requested, they are executed and
    /// results are fed back into the next turn so the LLM can see them and
    /// decide what to do next — the tool facilitates the mechanics, the LLM
    /// owns the decisions.
    ///
    /// This is intentionally a bounded loop (max 200 turns) to prevent
    /// runaway delegation.
    ///
    /// Returns `BoxFuture` to break async recursion with `execute_single`.
    pub(crate) fn execute_delegate_to_agent<'a>(
        &'a self,
        arguments: &'a str,
    ) -> BoxFuture<'a, Result<serde_json::Value, TianyanError>> {
        Box::pin(async move {
            const MAX_DELEGATION_TURNS: usize = 200;

            let params: DelegateToAgentParams = serde_json::from_str(arguments)
                .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

            let model_service = self.model_service.clone().ok_or_else(|| {
                TianyanError::Custom(format!(
                    "tool: 执行失败：{}",
                    "ModelService not configured for delegation",
                ))
            })?;

            let mut sub_messages: Vec<Message> = Vec::new();
            if let Some(prompt) = params.system_prompt.as_deref() {
                if !prompt.is_empty() {
                    sub_messages.push(Message::system(prompt));
                }
            }
            sub_messages.push(Message::user(&params.task));

            let mut total_tokens: usize = 0;

            for _turn in 0..MAX_DELEGATION_TURNS {
                let request = ChatCompletionRequest::new(&self.model, sub_messages.clone());
                let response = model_service
                    .chat_completion(request)
                    .await
                    .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;

                total_tokens += response.usage.total_tokens;
                let choice = response.choices.into_iter().next().ok_or_else(|| {
                    TianyanError::Custom(format!("tool: 执行失败：{}", "Empty response"))
                })?;

                let assistant_msg = choice.message;
                if assistant_msg.content.is_empty() {
                    if let Some(ref tool_calls) = assistant_msg.tool_calls {
                        // LLM requested tools — execute them in parallel, feed results back.

                        // Record the assistant's tool-call request in the history.
                        sub_messages.push(Message::assistant_with_tools(
                            String::new(),
                            tool_calls.clone(),
                        ));

                        // Filter out nested delegation (breaks async recursion).
                        let (allowed, denied): (Vec<_>, Vec<_>) = tool_calls
                            .iter()
                            .cloned()
                            .partition(|tc| tc.function.name != "delegate_to_agent");

                        if !allowed.is_empty() {
                            let results = self.execute_parallel(&allowed).await;
                            for (call_id, result) in results {
                                let content = match result {
                                    Ok(val) => val.to_string(),
                                    Err(e) => format!("Error: {}", e),
                                };
                                sub_messages.push(Message::tool(call_id, content));
                            }
                        }
                        for tc in denied {
                            sub_messages.push(Message::tool(
                                &tc.id,
                                "Error: nested delegation is not supported",
                            ));
                        }
                        continue;
                    }
                } else {
                    // LLM provided a final answer — record it and return.
                    sub_messages.push(Message::assistant(assistant_msg.content.clone()));
                    return Ok(serde_json::json!({
                        "result": assistant_msg.content,
                        "total_tokens": total_tokens,
                    }));
                }
            }

            Ok(serde_json::json!({
                "result": "sub-task reached max turns without final answer",
                "total_tokens": total_tokens,
            }))
        })
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "agent_ops_tests.rs"]
mod tests;
