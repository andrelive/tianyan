//! Agent 交互类工具执行器：execute_command / call_skill / ask_user / self_check /
//! delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

/// 委托链最大深度（主循环为 0；1 = 一层子 Agent，以此类推）。
pub(crate) const MAX_DELEGATION_DEPTH: usize = 3;

/// 委托深度 RAII guard：Drop 时自动递减，保证异常路径不泄漏深度计数。
pub(crate) struct DelegationDepthGuard(pub(crate) Arc<AtomicUsize>);

impl Drop for DelegationDepthGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

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
    /// 编排能力：
    /// - **并行委托**：同一轮内多个 `delegate_to_agent` 调用经
    ///   [`execute_parallel`]（JoinSet）并行执行，多个子任务可同时推进。
    /// - **嵌套委托**：子 Agent 内可继续委托（树状编排），深度受
    ///   [`MAX_DELEGATION_DEPTH`] 限制，超限调用被拒绝（防失控派生）。
    /// - **有界循环**：默认 200 轮，可用 `max_turns` 收紧；`timeout_secs`
    ///   可整体限时，超时中断委托。
    /// - **后台委托**：`background = true` 时 fire-and-forget——立即返回
    ///   任务 ID，任务独立运行；完成时自动向父会话注入通知（含结果摘要
    ///   与剩余任务计数），主 LLM 下一轮看到后聚合继续（业界模式：
    ///   opencode task(background) / Claude Code background subagents）。
    ///
    /// Returns `BoxFuture` to break async recursion with `execute_single`.
    pub(crate) fn execute_delegate_to_agent<'a>(
        &'a self,
        arguments: &'a str,
    ) -> BoxFuture<'a, Result<serde_json::Value, TianyanError>> {
        Box::pin(async move {
            let params: DelegateToAgentParams = serde_json::from_str(arguments)
                .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

            // 后台委托：注册 + spawn + 立即返回（并发上限由任务管理器控制；
            // 后台任务内再委托由内层深度检查与并发上限共同防失控）。
            if params.background == Some(true) {
                return self.spawn_background_delegate(&params).await;
            }

            // 嵌套委托深度保护：进入委托时 +1，超出上限拒绝。
            let depth = self.delegation_depth.fetch_add(1, Ordering::SeqCst) + 1;
            if depth > MAX_DELEGATION_DEPTH {
                self.delegation_depth.fetch_sub(1, Ordering::SeqCst);
                return Err(TianyanError::Custom(format!(
                    "tool: 委托深度超过上限（{} 层），已拒绝嵌套委托",
                    MAX_DELEGATION_DEPTH
                )));
            }
            // 深度 guard 必须活到委托结束（含 Err 路径），Drop 时自动回滚计数。
            let _depth_guard = DelegationDepthGuard(self.delegation_depth.clone());

            self.run_delegation_loop(&params).await
        })
    }

    /// 委托核心循环（前台/后台共用）。
    ///
    /// 深度保护由调用方负责（前台在入口，后台任务内由内层委托自行检查）。
    pub(crate) async fn run_delegation_loop(
        &self,
        params: &DelegateToAgentParams,
    ) -> Result<serde_json::Value, TianyanError> {
        const DEFAULT_MAX_TURNS: usize = 200;
        const MAX_TURNS_CAP: usize = 500;

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

        let max_turns = params
            .max_turns
            .unwrap_or(DEFAULT_MAX_TURNS)
            .clamp(1, MAX_TURNS_CAP);

        let run_loop = async {
            let mut total_tokens: usize = 0;

            for _turn in 0..max_turns {
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
                        // 嵌套委托不再过滤：子 Agent 内调用 delegate_to_agent 直接执行，
                        // 由委托深度计数器（depth_guard）防止失控派生。
                        sub_messages.push(Message::assistant_with_tools(
                            String::new(),
                            tool_calls.clone(),
                        ));

                        if !tool_calls.is_empty() {
                            let results = self.execute_parallel(tool_calls).await;
                            for (call_id, result) in results {
                                let content = match result {
                                    Ok(val) => val.to_string(),
                                    Err(e) => format!("Error: {}", e),
                                };
                                sub_messages.push(Message::tool(call_id, content));
                            }
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
                "result": format!("sub-task reached max turns ({}) without final answer", max_turns),
                "total_tokens": total_tokens,
            }))
        };

        match params.timeout_secs {
            Some(secs) => {
                let result =
                    tokio::time::timeout(std::time::Duration::from_secs(secs), run_loop).await;
                match result {
                    Ok(inner) => inner,
                    Err(_) => Err(TianyanError::Custom(format!(
                        "tool: 委托执行超时（{}s）",
                        secs
                    ))),
                }
            }
            None => run_loop.await,
        }
    }

    /// 后台委托：注册任务 → 获取并发许可 → spawn 独立执行 → 立即返回。
    async fn spawn_background_delegate(
        &self,
        params: &DelegateToAgentParams,
    ) -> Result<serde_json::Value, TianyanError> {
        let manager = &self.background_tasks;

        // 父会话归属：AgentLoop 每轮工具执行前设置
        let session_id = self
            .current_session_id
            .lock()
            .await
            .clone()
            .ok_or_else(|| {
                TianyanError::Custom(
                    "tool: 后台任务需要会话上下文（current_session_id 未设置）".to_string(),
                )
            })?;

        // 并发许可：达到上限立即拒绝（不阻塞）
        let _permit = manager.try_acquire().await?;

        let desc = params.task.clone();
        let task_id = manager.register(desc.clone(), session_id.clone()).await;
        manager.mark_running(&task_id).await;

        let this = self.clone();
        let mgr = manager.clone();
        let run_params = params.clone();
        let run_task_id = task_id.clone();

        tokio::spawn(async move {
            let outcome = this.run_delegation_loop(&run_params).await;
            match outcome {
                Ok(v) => mgr.complete(&run_task_id, format!("{v}")).await,
                Err(e) => mgr.fail(&run_task_id, e.to_string()).await,
            }
            // _permit 在此作用域结束时释放
        });

        Ok(serde_json::json!({
            "task_id": task_id,
            "status": "running",
            "message": format!(
                "后台任务已启动（{desc}）。完成后将自动通知本会话，无需轮询；可用 task_status 查询状态。"
            ),
        }))
    }

    /// 执行 task_status 工具：查询后台任务状态与结果（非阻塞快照）。
    pub(crate) async fn execute_task_status(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        #[derive(serde::Deserialize)]
        struct Params {
            task_id: String,
        }
        let params: Params = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

        let task = self
            .background_tasks
            .get(&params.task_id)
            .await
            .ok_or_else(|| TianyanError::Custom(format!("tool: 任务不存在：{}", params.task_id)))?;

        serde_json::to_value(task)
            .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")))
    }

    /// 执行 task_cancel 工具：取消后台任务（终态任务为幂等空操作）。
    pub(crate) async fn execute_task_cancel(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        #[derive(serde::Deserialize)]
        struct Params {
            task_id: String,
        }
        let params: Params = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

        self.background_tasks.cancel(&params.task_id).await?;

        Ok(serde_json::json!({
            "task_id": params.task_id,
            "status": "cancelled",
            "message": "后台任务已取消",
        }))
    }
    /// 执行 web_search 工具：搜索网页并返回结构化结果（标题/URL/摘要）。
    pub(crate) async fn execute_web_search(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let client = self.web_client.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "WebSearchClient not configured"
            ))
        })?;
        crate::executor::web::execute_web_search(client, arguments).await
    }

    /// 执行 web_fetch 工具：抓取网页并提取可读文本。
    ///
    /// SSRF 防护（仅公网 http/https）在 [`crate::executor::web::validate_public_url`]
    /// 内强制执行，未配置客户端时返回明确错误。
    pub(crate) async fn execute_web_fetch(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let client = self.web_client.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "WebSearchClient not configured"
            ))
        })?;
        crate::executor::web::execute_web_fetch(client, arguments).await
    }
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "agent_ops_tests.rs"]
mod tests;
