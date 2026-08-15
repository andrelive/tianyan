//! Agent 交互类工具执行器：execute_command / call_skill / ask_user / self_check /
//! delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::role_store::RoleStore;
use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
};
use crate::common::error::TianyanError;
use crate::common::types::Message;
use crate::executor::Action;
use crate::model::types::ChatCompletionRequest;
use crate::model::types::ToolCall;
use crate::skills::SkillExecutionRequest;

use super::{parse_params, safety_violation, wrap_tool_error, ToolRegistry};

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
    ///
    /// `session_id` 为真实会话（审批挂起通知/归属用）；`subagent` 为子任务
    /// 执行上下文——审批不交互（未授权操作立即拒绝，上报主 agent 确认）。
    pub(crate) async fn execute_execute_command(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
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

        // 默认工作目录：模型未指定 cwd 时使用会话绑定的工作目录（工作区），
        // 缺省回退进程当前目录——确保命令在用户的工作区内执行，而非进程 cwd
        // （如天演仓库根目录）
        let cwd = match params.cwd.clone() {
            Some(cwd) => Some(cwd),
            None => match &self.session_manager {
                Some(sm) => match sm.get_session(session_id).await {
                    Ok(Some(session)) => session
                        .working_directory(None)
                        .map(|p| p.to_string_lossy().into_owned()),
                    _ => None,
                },
                None => None,
            },
        };

        // Path sandbox for working directory
        if let Some(ref cwd) = cwd {
            safety_violation(self.security_policy.check_path(std::path::Path::new(cwd)))?;
        }

        // 审批门控（自动放行安全命令/拒绝关键命令/请求人类确认，
        // 统一序列见 [`ToolRegistry::ensure_approved`]；子任务非交互拒绝，
        // 错误携带"需要主任务授权"标记，由主 agent 在主对话确认后重新委托）
        self.ensure_approved(
            session_id,
            subagent,
            &Action::ExecuteCommand {
                command: params.command.clone(),
                cwd: cwd.clone(),
                timeout_secs: params.timeout_secs,
            },
        )
        .await?;
        // 后台模式：立即返回任务快照（task_id/log_file/pid），进程独立运行。
        // 观测：command_status 查询（输出尾部）/ 日志文件 read_file；终止：command_kill。
        if params.background == Some(true) {
            let task = self
                .command_tasks
                .spawn_background(session_id, &params.command, cwd.as_deref())
                .await
                .map_err(wrap_tool_error)?;
            return Ok(serde_json::json!({
                "task_id": task.id,
                "pid": task.pid,
                "log_file": task.log_file,
                "status": "running",
                "message": "后台命令已启动，不等待退出。可用 command_status 查询状态与输出尾部，command_kill 终止（杀进程树）。",
            }));
        }

        crate::executor::execute_command_action(
            &params.command,
            cwd.as_deref(),
            params.timeout_secs,
        )
        .await
        .map_err(wrap_tool_error)
    }

    /// 执行 call_skill 工具：调用已注册技能。
    pub(crate) async fn execute_call_skill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: CallSkillParams = parse_params(arguments)?;

        // G2a：技能激活可观测性——self_check 可见每个技能被调用的次数。
        if let Some(ref metrics) = self.metrics {
            metrics.record_skill_call(&params.skill_id).await;
        }

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
                Err(e) => Err(wrap_tool_error(e)),
            }
        } else {
            Err(TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "SkillExecutor not configured",
            )))
        }
    }

    /// 执行 ask_user 工具：向用户追问。
    ///
    /// 语义归一：主循环（`loop.rs::handle_llm_response`）在工具执行前按名称
    /// 拦截 ask_user 并转 `NeedsClarification`——本函数因此只在**子代理循环**
    /// （delegation）可达。子代理不可交互追问：返回指导性结果（携带原问题），
    /// 让子 LLM 基于已有上下文继续，而非收到误导性的错误字符串。
    pub(crate) async fn execute_ask_user(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: AskUserParams = parse_params(arguments)?;
        Ok(serde_json::json!({
            "status": "delegated_agent_cannot_ask",
            "question": params.question,
            "hint": "子代理无法向用户追问。请基于已有上下文与工具结果尽量回答该问题；若确需用户输入，请给出当前可交付的最佳结果。",
        }))
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
    ///
    /// `session_id` 随调用链显式传递（后台委托归属父会话；不依赖共享可变
    /// 状态，多会话并发 turn 安全）。
    pub(crate) fn execute_delegate_to_agent<'a>(
        &'a self,
        arguments: &'a str,
        session_id: &'a str,
    ) -> BoxFuture<'a, Result<serde_json::Value, TianyanError>> {
        Box::pin(async move {
            let params: DelegateToAgentParams = serde_json::from_str(arguments)
                .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

            // 后台委托：注册 + spawn + 立即返回（并发上限由任务管理器控制；
            // 后台任务内再委托由内层深度检查与并发上限共同防失控）。
            if params.background == Some(true) {
                return self.spawn_background_delegate(&params, session_id).await;
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

            self.run_delegation_loop(&params, session_id).await
        })
    }

    /// 角色会话存储（ADR-016 决策 5；VFS 未配置时 None——无会话语义，行为同现状）。
    fn role_session_store(&self) -> Option<RoleStore> {
        self.vfs.clone().map(RoleStore::new)
    }

    /// 委托核心循环（前台/后台共用）。
    ///
    /// 深度保护由调用方负责（前台在入口，后台任务内由内层委托自行检查）。
    ///
    /// 角色解析（`params.role`）：未知角色在循环启动前报错（模型可换用有效
    /// 角色重试）。字段优先级：显式参数 > 角色值 > 主 Agent 配置/默认值。
    pub(crate) async fn run_delegation_loop(
        &self,
        params: &DelegateToAgentParams,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        const DEFAULT_MAX_TURNS: usize = 200;
        const MAX_TURNS_CAP: usize = 500;

        let model_service = self.model_service.clone().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "ModelService not configured for delegation",
            ))
        })?;

        let role = match params.role.as_deref() {
            Some(name) => {
                let active = self.role_registry.get_active(name);
                match active {
                    Some(role) => Some(role),
                    None => {
                        // 区分：角色不存在 vs 试验性不可调用（ADR-016）
                        let experimental = self
                            .role_registry
                            .get(name)
                            .map(|r| r.status == crate::agent::RoleStatus::Experimental)
                            .unwrap_or(false);
                        return Err(TianyanError::Custom(if experimental {
                            format!("tool: 角色 {name} 为试验性，不可调用（仅展示供评估）")
                        } else {
                            format!(
                                "tool: 角色不存在：{name}。可用角色：{}",
                                self.role_registry.active_names().join("、")
                            )
                        }));
                    }
                }
            }
            None => None,
        };

        // 模型优先级：显式 model > role.model > 主 Agent 模型。
        let model = params
            .model
            .clone()
            .or_else(|| role.as_ref().and_then(|r| r.model.clone()))
            .unwrap_or_else(|| self.model.clone());

        // 系统提示优先级：显式 system_prompt > role.system_prompt > 无。
        let system_prompt = params
            .system_prompt
            .clone()
            .or_else(|| role.as_ref().and_then(|r| r.system_prompt.clone()));

        // 轮数/超时优先级：显式参数 > 角色值 > 默认值。
        let max_turns = params
            .max_turns
            .or_else(|| role.as_ref().and_then(|r| r.max_turns))
            .unwrap_or(DEFAULT_MAX_TURNS)
            .clamp(1, MAX_TURNS_CAP);
        let timeout_secs = params
            .timeout_secs
            .or_else(|| role.as_ref().and_then(|r| r.timeout_secs));

        // 工具白名单：Some 时白名单外工具调用返回合成错误结果（不硬失败循环）。
        let role_tools: Option<Vec<String>> = role.as_ref().and_then(|r| r.tools.clone());
        let role_name: Option<String> = role.as_ref().map(|r| r.name.clone());

        // ADR-016 决策 5：durable 角色会话（仅 role 委托生效；主 agent 自主决策）
        let session_mode = params.session.as_deref().unwrap_or("continue");
        // (role, 历史任务数, 加载消息数)——返回时附加给主 agent 作决策依据
        let mut session_meta: Option<(String, u32, usize)> = None;
        let mut sub_messages: Vec<Message> = Vec::new();
        if let Some(role_name) = role_name.as_deref() {
            match session_mode {
                "new" => {
                    // 新会话：干净上下文（不加载历史；执行后覆盖保存）
                }
                "discard" => {
                    // 显式废弃旧会话（旧事实已过期），再开新会话执行
                    if let Some(store) = self.role_session_store() {
                        if let Err(e) = store.clear_role_session(role_name).await {
                            tracing::warn!(role = %role_name, error = %e, "角色会话废弃失败");
                        }
                    }
                }
                _ => {
                    // continue（默认）：续用持久会话（领域记忆复用）
                    if let Some(store) = self.role_session_store() {
                        if let Ok(Some(session)) = store.load_role_session(role_name).await {
                            let loaded = session.messages.len();
                            let mut history = session.messages;
                            // 截断保护：保留首条（系统提示）+ 最近 N-1 条
                            const MAX_SESSION_MESSAGES: usize = 60;
                            if history.len() > MAX_SESSION_MESSAGES {
                                let first = history[0].clone();
                                let tail =
                                    history.split_off(history.len() - (MAX_SESSION_MESSAGES - 1));
                                history = tail;
                                history.insert(0, first);
                            }
                            sub_messages = history;
                            session_meta =
                                Some((role_name.to_string(), session.task_count, loaded));
                        }
                    }
                }
            }
        }
        // 无历史（首委托 / new / discard 或无会话存储）时注入系统提示
        if sub_messages.is_empty() {
            if let Some(prompt) = system_prompt.as_deref() {
                if !prompt.is_empty() {
                    sub_messages.push(Message::system(prompt));
                }
            }
        }
        sub_messages.push(Message::user(&params.task));

        let delegation_start = std::time::Instant::now();

        let run_loop = async {
            let mut total_tokens: usize = 0;

            for _turn in 0..max_turns {
                // T12 延期决策：子 agent 请求未应用 dynamic_max_tokens。
                // 原因：ToolRegistry 仅持有模型名字符串（self.model），既不持有
                // chat_spec（主 agent 的 ModelSpec 经 AgentLoop::with_chat_spec 注入，
                // 未传入 ToolRegistry），也不持有 provider 名——builtin_spec /
                // resolve_spec 需要 provider + model 双前缀匹配，仅有 model 名
                // 无法解析规格；ChatService trait 亦不暴露规格查询。接线需把
                // chat_spec 注入 ToolRegistry（架构变更，另行立项），届时按
                // dynamic_max_tokens(&spec, None, &sub_messages) 设置 max_tokens
                // （子 agent 无实测输入，退化为 TokenEstimator 估算）。
                // 主 agent 请求已应用动态 max_tokens（agent/loop.rs），本处保持默认。
                let request = ChatCompletionRequest::new(&model, sub_messages.clone());
                let response = model_service
                    .chat_completion(request)
                    .await
                    .map_err(wrap_tool_error)?;

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
                            // subagent=true：子任务工具执行——审批不交互，
                            // 未授权操作拒绝并上报主 agent 确认
                            let results = self
                                .execute_tool_calls_with_role(
                                    tool_calls,
                                    session_id,
                                    role_tools.as_deref(),
                                    role_name.as_deref(),
                                )
                                .await;
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
                    return Ok((
                        serde_json::json!({
                            "result": assistant_msg.content,
                            "total_tokens": total_tokens,
                        }),
                        sub_messages.clone(),
                    ));
                }
            }

            Ok((
                serde_json::json!({
                    "result": format!(
                        "sub-task reached max turns ({}) without final answer",
                        max_turns
                    ),
                    "total_tokens": total_tokens,
                }),
                sub_messages.clone(),
            ))
        };

        let outcome = match timeout_secs {
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
        };

        // ADR-016 决策 5：成功路径持久化角色会话（continue/new/discard 统一覆盖保存）
        if let (Some(name), Some(store)) = (role_name.as_deref(), self.role_session_store()) {
            if let Ok((_, final_messages)) = &outcome {
                let task_count = session_meta.as_ref().map(|(_, n, _)| *n).unwrap_or(0) + 1;
                if let Err(e) = store
                    .save_role_session(name, final_messages, task_count)
                    .await
                {
                    tracing::warn!(role = %name, error = %e, "角色会话保存失败");
                }
            }
        }

        // ADR-016：委托历史明细（统计面板；成功/失败均记录）
        if let Some(name) = role_name.as_deref() {
            if let Some(store) = self.role_session_store() {
                let (success, tokens) = match &outcome {
                    Ok((value, _)) => (
                        true,
                        value
                            .get("total_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                    ),
                    Err(_) => (false, 0),
                };
                let record = crate::agent::role_store::DelegationRecord {
                    ts: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                    role: name.to_string(),
                    task: crate::common::llm_judge::truncate_output(&params.task, 120),
                    success,
                    mode: session_mode.to_string(),
                    duration_ms: delegation_start.elapsed().as_millis() as u64,
                    tokens,
                };
                if let Err(e) = store.append_delegation_record(&record).await {
                    tracing::warn!(role = %name, error = %e, "委托历史记录失败");
                }
            }
        }

        // 返回时附加会话决策信息（主 agent 下一轮决策依据）
        match outcome {
            Ok((mut value, _)) => {
                if let Some((name, prev_count, loaded)) = session_meta {
                    value["role_session"] = serde_json::json!({
                        "role": name,
                        "mode": session_mode,
                        "task_count": prev_count + 1,
                        "loaded_messages": loaded,
                    });
                }
                Ok(value)
            }
            Err(e) => Err(e),
        }
    }

    /// 执行子 Agent 工具调用（含角色白名单过滤）。
    ///
    /// `role_tools` 为 Some 时，白名单外的工具调用**不执行**，而是返回合成
    /// 错误结果（保留 call_id，错误消息作为普通工具结果回喂模型，模型可换用
    /// 允许的工具重试）；白名单内工具正常并行执行。`role_tools` 为 None 时
    /// 不限制（与主 Agent 相同）。前台/后台共用此路径，过滤语义一致。
    async fn execute_tool_calls_with_role(
        &self,
        tool_calls: &[ToolCall],
        session_id: &str,
        role_tools: Option<&[String]>,
        role_name: Option<&str>,
    ) -> Vec<(String, Result<serde_json::Value, TianyanError>)> {
        let Some(allowed) = role_tools else {
            return self.execute_parallel(tool_calls, session_id, true).await;
        };

        let (allowed_refs, blocked): (Vec<&ToolCall>, Vec<&ToolCall>) = tool_calls
            .iter()
            .partition(|call| allowed.iter().any(|t| t == &call.function.name));
        let allowed_calls: Vec<ToolCall> = allowed_refs.into_iter().cloned().collect();

        let mut results = self
            .execute_parallel(&allowed_calls, session_id, true)
            .await;
        for call in blocked {
            results.push((
                call.id.clone(),
                Err(TianyanError::Custom(format!(
                    "tool: 角色 {} 不允许使用工具 {}（允许：{}）",
                    role_name.unwrap_or("?"),
                    call.function.name,
                    allowed.join("、")
                ))),
            ));
        }
        results
    }

    /// 后台委托：注册任务 → 获取并发许可 → spawn 独立执行 → 立即返回。
    ///
    /// `session_id` 由调用链显式传入（父会话归属），不再依赖共享可变状态。
    async fn spawn_background_delegate(
        &self,
        params: &DelegateToAgentParams,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let manager = &self.background_tasks;

        // 父会话归属：调用链显式传递（AgentLoop 经 execute_parallel 传入当前会话）
        let session_id = session_id.to_string();

        // 并发许可：达到上限立即拒绝（不阻塞）
        let _permit = manager.try_acquire().await?;

        let desc = params.task.clone();
        let task_id = manager.register(desc.clone(), session_id.clone()).await;
        manager.mark_running(&task_id).await;

        let this = self.clone();
        let mgr = manager.clone();
        let run_params = params.clone();
        let run_task_id = task_id.clone();
        let run_session_id = session_id.clone();

        tokio::spawn(async move {
            let outcome = this.run_delegation_loop(&run_params, &run_session_id).await;
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

    /// 执行 command_status 工具：查询后台命令任务状态与输出尾部（非阻塞快照）。
    pub(crate) async fn execute_command_status(
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
            .command_tasks
            .get(&params.task_id)
            .await
            .ok_or_else(|| TianyanError::Custom(format!("tool: 任务不存在：{}", params.task_id)))?;

        serde_json::to_value(task)
            .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")))
    }

    /// 执行 command_list 工具：列出全部后台命令任务（按注册序号升序）。
    pub(crate) async fn execute_command_list(
        &self,
        _arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let tasks = self.command_tasks.list().await;
        serde_json::to_value(tasks)
            .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")))
    }

    /// 执行 command_kill 工具：终止后台命令（杀进程树；终态任务为幂等空操作）。
    pub(crate) async fn execute_command_kill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        #[derive(serde::Deserialize)]
        struct Params {
            task_id: String,
        }
        let params: Params = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

        self.command_tasks
            .kill(&params.task_id)
            .await
            .map_err(wrap_tool_error)?;

        Ok(serde_json::json!({
            "task_id": params.task_id,
            "status": "cancelled",
            "message": "后台命令已终止（进程树已杀）",
        }))
    }
    /// 执行 suggest_role 工具（ADR-016 P3）：任务描述 → 角色语义匹配建议。
    pub(crate) async fn execute_suggest_role(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let router = self.role_router.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "RoleRouter not configured"))
        })?;
        #[derive(serde::Deserialize)]
        struct Params {
            task: String,
            top_k: Option<usize>,
        }
        let params: Params = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

        let matches = router
            .suggest(&params.task, params.top_k.unwrap_or(3))
            .await?;
        let items: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.name,
                    "score": m.score,
                    "purpose": m.purpose,
                    "status": if m.status == crate::agent::RoleStatus::Experimental {
                        "experimental"
                    } else {
                        "active"
                    },
                })
            })
            .collect();
        Ok(serde_json::json!({
            "task": params.task,
            "suggestions": items,
            "message": "建议仅供决策参考——最终选择由你决定；[experimental] 角色暂不可调用。",
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
