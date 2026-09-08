//! Agent 交互类工具执行器：execute_command / call_skill / ask_user / self_check /
//! delegate_to_agent。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::r#loop::{
    AgentLoop, AgentLoopConfig, AgentLoopResult, ToolExecutorKind, TurnPolicy,
};
use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    SubmitResultParams, SuggestRoleParams, TaskCancelParams, TaskStatusParams,
};
use crate::agent::types::{AgentStreamChunk, StreamChunkType, StreamEventSender};
use crate::common::error::TianyanError;
use crate::common::types::{ContentLevel, ContextNamespace, Message, TianyanUri};
use crate::executor::Action;
use crate::model::types::{FunctionDefinition, ToolCall, ToolDefinition};

use super::{parse_params, safety_violation, wrap_tool_error, ToolExecutionOutcome, ToolRegistry};

/// 委托链最大深度（主循环为 0；1 = 一层子 Agent，以此类推）。
pub(crate) const MAX_DELEGATION_DEPTH: usize = 3;

/// ask_user 等待用户回答的超时（防挂死；超时返回错误，模型看到失败结果）。
const ASK_USER_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// 委托深度 RAII guard：Drop 时自动递减，保证异常路径不泄漏深度计数。
pub(crate) struct DelegationDepthGuard(pub(crate) Arc<AtomicUsize>);

impl Drop for DelegationDepthGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 子代理流式事件转发映射（ADR-030：AgentStreamChunk → ChatStreamEvent
/// 同构 JSON，与主会话协议一致——前端完全复用 reducer 渲染）。
///
/// Message chunk（落库即广播已推 message 事件）与 Error 不转发。
fn chunk_to_stream_json(chunk: AgentStreamChunk) -> Option<serde_json::Value> {
    if matches!(
        chunk.chunk_type,
        StreamChunkType::Message | StreamChunkType::Error
    ) {
        return None;
    }
    let is_thought = chunk.chunk_type == StreamChunkType::Thought;
    let is_observational = matches!(
        chunk.chunk_type,
        StreamChunkType::ToolCall | StreamChunkType::Observation
    );
    let delta = if is_thought || is_observational {
        String::new()
    } else {
        chunk.delta.clone()
    };
    Some(serde_json::json!({
        "chunk_type": chunk.chunk_type,
        "delta": delta,
        "thinking": if is_thought { Some(chunk.delta) } else { None },
        "finish_reason": if chunk.is_complete {
            chunk.finish_reason.or_else(|| Some("stop".to_string()))
        } else {
            None
        },
        "tool_call": chunk.tool_call,
        "tool_result": chunk.tool_result,
    }))
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
        // 观测：task_status 查询（输出尾部）/ 日志文件 read_file；终止：task_cancel。
        if params.background == Some(true) {
            // 就绪探测规格（可选）：端口/日志关键词就绪后自动通知主 agent；
            // 长驻服务（server/dev server 等）必须配置，否则只能等进程退出通知。
            let ready_spec = params
                .ready
                .as_ref()
                .map(|r| crate::executor::command::ReadySpec {
                    port: r.port,
                    pattern: r.pattern.clone(),
                    initial_delay_ms: r.initial_delay_ms.unwrap_or(500),
                    timeout_ms: r.timeout_ms.unwrap_or(300_000),
                });
            let task = self
                .command_tasks
                .spawn_background(
                    session_id,
                    &params.command,
                    cwd.as_deref(),
                    ready_spec,
                    // 时序锚点：任务启动时链上消息数（= 下一条消息的 seq）。
                    // 回退到用户输入 U 时，锚点 > U.seq 的任务属于"回退点之后"，
                    // 应一并取消（方案 B：任务与链上时序点关联，精确取消）。
                    match &self.session_manager {
                        Some(sm) => match sm.get_session(session_id).await {
                            Ok(Some(s)) => s.messages.len() as i64,
                            _ => 0,
                        },
                        None => 0,
                    },
                )
                .await
                .map_err(wrap_tool_error)?;
            let ready_msg = if params.ready.is_some() {
                "已配置就绪探测：端口监听/日志关键词出现后自动通知本会话。"
            } else {
                "可用 task_status 查询状态与输出尾部，task_cancel 终止（杀进程树）。"
            };
            return Ok(serde_json::json!({
                "task_id": task.id,
                "pid": task.pid,
                "log_file": task.log_file,
                "status": "running",
                "message": format!("后台命令已启动，不等待退出。{ready_msg}"),
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

    /// 执行 call_skill 工具：读取 VFS 技能文档（L0 摘要 + L2 详情）返回。
    ///
    /// 技能 = 方法论文档（VFS `skill/{id}/`），无执行语义——模型读到内容后
    /// 参考方法论自行用基础工具执行。planning 为预置技能（bootstrap 写入），
    /// GEPA 学习技能由进化引擎写入，两者同构。
    pub(crate) async fn execute_call_skill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: CallSkillParams = parse_params(arguments)?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "VFS not configured"))
        })?;

        let uri = TianyanUri::new(ContextNamespace::Skill, vec![params.skill_id.clone()]);
        if !vfs.exists(&uri).await.map_err(wrap_tool_error)? {
            return Err(TianyanError::Custom(format!(
                "tool: 执行失败：技能 '{}' 不存在（VFS skill/ 命名空间下无此技能）",
                params.skill_id
            )));
        }

        let (abstract_text, detail) = tokio::join!(
            vfs.read_content(&uri, ContentLevel::Abstract),
            vfs.read_content(&uri, ContentLevel::Detail),
        );
        let abstract_text = abstract_text.map_err(wrap_tool_error)?;
        let detail = detail.map_err(wrap_tool_error)?;

        if detail.trim().is_empty() {
            return Err(TianyanError::Custom(format!(
                "tool: 执行失败：技能 '{}' 内容为空",
                params.skill_id
            )));
        }

        let mut data = HashMap::new();
        data.insert(
            "skill_id".to_string(),
            serde_json::Value::String(params.skill_id.clone()),
        );
        data.insert(
            "abstract".to_string(),
            serde_json::Value::String(abstract_text),
        );
        data.insert("detail".to_string(), serde_json::Value::String(detail));
        Ok(serde_json::Value::Object(data.into_iter().collect()))
    }

    /// 执行 ask_user 工具：**同步等待用户回答**（对齐 DSH：工具执行挂起，
    /// 回答作为普通工具结果返回，loop 继续——无拦截/无独立澄清轮）。
    ///
    /// 工具调用事件（ToolCall chunk）已把问题（arguments JSON）推给前端，
    /// 前端拉起组件；用户回答经 server 回答端点提交到 [`UserQuestionService`]，
    /// 本方法 await 到回答后返回 `{answers: [...]}` 作为普通工具结果。
    ///
    /// 子代理不能向用户提问（ADR-011：任务下发即授权边界）——返回合成结果，
    /// 子代理把问题/决策带进最终结果，由主 agent 在主对话中确认。
    ///
    /// 审批降级：回答命中确认语义（允许/是等）时放行待确认指纹——
    /// 模型看到回答后重试被拒工具时审批通过（原 NeedsClarification 确认链路）。
    pub(crate) async fn execute_ask_user(
        &self,
        arguments: &str,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: AskUserParams = parse_params(arguments)?;
        if subagent {
            return Ok(serde_json::json!({
                "status": "delegated_agent_cannot_ask",
                "question": params.question,
                "hint": "子代理无法向用户追问。请基于已有上下文与工具结果尽量回答该问题；若确需用户输入，请给出当前可交付的最佳结果。",
            }));
        }
        let service = self.user_questions.as_ref().ok_or_else(|| {
            TianyanError::Custom("tool: 执行失败：用户问题服务未装配".to_string())
        })?;
        // 挂起等待用户回答（超时防挂死；主循环停止时取消等待）
        let answers = service
            .ask(session_id, ASK_USER_WAIT_TIMEOUT, || {
                self.delegation_cancelled_sync(session_id)
            })
            .await?;
        // 审批降级：确认语义放行待确认指纹（模型重试工具时通过）
        let approved = crate::executor::approval::is_user_confirmation(&answers.to_string());
        self.confirm_pending_approval(approved).await;
        Ok(answers)
    }

    /// 执行 self_check 工具：查询内部指标。
    ///
    /// 技能激活统计读 UsageStats（持久化权威，ToolObservabilityListener 逐次记录）；
    /// AgentMetrics 不再维护第二份内存计数（E1：口径统一、重启不丢）。
    pub(crate) async fn execute_self_check(&self) -> Result<serde_json::Value, TianyanError> {
        let metrics = self.metrics.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "AgentMetrics not configured"))
        })?;
        let mut health = metrics.query_harness_health().await;
        let top_skills = match self.observability.usage_stats_ref() {
            Some(stats) => stats.query_top_skills(20).await,
            None => Vec::new(),
        };
        if let Some(obj) = health.as_object_mut() {
            obj.insert(
                "skills_invoked".to_string(),
                serde_json::json!(top_skills.len()),
            );
            obj.insert(
                "skill_calls_total".to_string(),
                serde_json::json!(top_skills.iter().map(|s| s.total_calls).sum::<u64>()),
            );
            obj.insert("top_skills".to_string(), serde_json::json!(top_skills));
        }
        Ok(health)
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

            // ADR-026：委托只支持异步——所有委托一律注册 + spawn + 立即返回 task_id。
            // 嵌套委托深度保护：进入委托时 +1，guard 移入任务闭包持有到任务结束
            // （含排队等待期间），Drop 时自动回滚计数。
            let depth = self.delegation_depth.fetch_add(1, Ordering::SeqCst) + 1;
            if depth > MAX_DELEGATION_DEPTH {
                self.delegation_depth.fetch_sub(1, Ordering::SeqCst);
                return Err(TianyanError::Custom(format!(
                    "tool: 委托深度超过上限（{} 层），已拒绝嵌套委托",
                    MAX_DELEGATION_DEPTH
                )));
            }
            let depth_guard = DelegationDepthGuard(self.delegation_depth.clone());

            self.spawn_background_delegate(&params, session_id, depth_guard)
                .await
        })
    }

    /// 子代理循环（ADR-030：统一循环框架——AgentLoop 实例 + 委托策略）。
    ///
    /// 上下文 = 角色系统提示 + 任务指令（一次性子代理：不加载角色历史）；
    /// 工具集 = 角色白名单过滤 + submit_result（可选结果落盘工具）；
    /// 流式路径（run_stream）→ 逐 token 事件经 TaskEventSink 转发（task_id
    /// 即 session_id，前端完全复用主会话 reducer）；
    /// 收尾统一"无工具调用"（submit_result 降级为普通工具，暂存结果优先）；
    /// 完成回调由调用方（watcher）处理任务状态机。
    pub(crate) async fn run_subagent_loop(
        &self,
        params: &DelegateToAgentParams,
        session_id: &str,
        task_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        const DEFAULT_MAX_TURNS: usize = 200;
        const MAX_TURNS_CAP: usize = 500;

        let model_service = self.model_service.clone().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "ModelService not configured for delegation",
            ))
        })?;
        let session_manager = self.session_manager.clone().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "SessionManager not configured for delegation",
            ))
        })?;

        // 角色解析（`params.role`）：未知角色在循环启动前报错（模型可换用
        // 有效角色重试）。字段优先级：显式参数 > 角色值 > 主 Agent 配置/默认值。
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

        // 轮数优先级：显式参数 > 角色值 > 默认值。
        let max_turns = params
            .max_turns
            .or_else(|| role.as_ref().and_then(|r| r.max_turns))
            .unwrap_or(DEFAULT_MAX_TURNS)
            .clamp(1, MAX_TURNS_CAP);
        // 超时优先级：显式参数 > 角色值 > 无（后台任务默认无超时，
        // 显式设置作为兜底守卫——ADR-026）。
        let timeout_secs = params
            .timeout_secs
            .or_else(|| role.as_ref().and_then(|r| r.timeout_secs));

        // 工具白名单：Some 时白名单外工具调用返回合成错误结果（不硬失败循环）。
        let role_tools: Option<Vec<String>> = role.as_ref().and_then(|r| r.tools.clone());
        let role_name: Option<String> = role.as_ref().map(|r| r.name.clone());

        // 一次性子代理（2026-08 决策：移除 durable 角色会话 continue 模式）：
        // 每次委托都是干净上下文——不加载角色历史（旧任务上下文会污染当前
        // 任务，子代理顺着旧对话跑偏），也不持久化。上下文 = 角色系统提示
        // + 当前任务指令。任务间记忆由主 agent 承担（主对话是唯一记忆载体）。
        let mut messages: Vec<Message> = Vec::new();
        if let Some(prompt) = system_prompt.as_deref() {
            if !prompt.is_empty() {
                messages.push(Message::system(prompt));
            }
        }
        messages.push(Message::user(&params.task));

        // 委托工具列表：角色白名单过滤（无白名单则全量）+ submit_result 恒在。
        // 此前请求未携带工具定义（ChatCompletionRequest::new 默认 tools=None）——
        // 子代理 LLM 看不到任何工具，只能凭空输出，这是"子代理报告与代码不符"
        // 的根本原因。
        let mut delegate_tools: Vec<ToolDefinition> = match role_tools.as_deref() {
            Some(allowed) => {
                let all = self.definitions().await;
                all.into_iter()
                    .filter(|d| allowed.iter().any(|t| t == &d.function.name))
                    .collect::<Vec<_>>()
            }
            None => self.definitions().await,
        };
        delegate_tools.push(ToolDefinition::function(
            FunctionDefinition::from_schema::<SubmitResultParams>(
                "submit_result",
                "当你的任务完成时，调用本工具把最终结果写入任务存储（result 为完整报告/结论，summary 为可选一句话摘要）。工具返回结果 ID（task_id）。调用后请在最终回复中告知主智能体结果 ID，主智能体可用 task_status 工具查询。",
            ),
        ));

        // 取消标志：主循环停止时置位（coordinator 注入 delegation_cancel）
        let cancel = self
            .delegation_cancel
            .lock()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)));

        // 流式事件转发：AgentStreamChunk → ChatStreamEvent 同构 JSON →
        // TaskEventSink（task_id 即 session_id，前端完全复用主会话 reducer）。
        // ADR-031：事件带 type=chat_stream + session_id=task_id——前端
        // routeChatStreamEvent 按 session_id 路由到任务面板的活跃归约器
        // （此前无 type/session_id，事件被前端丢弃——子代理流式未生效）。
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<AgentStreamChunk, TianyanError>>(64);
        let sink = self.task_event_sink.clone();
        let task_id_owned = task_id.to_string();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                let Ok(chunk) = chunk else { continue };
                if let Some(mut json) = chunk_to_stream_json(chunk) {
                    json["type"] = serde_json::json!("chat_stream");
                    json["session_id"] = serde_json::json!(task_id_owned);
                    if let Some(sink) = &sink {
                        sink.emit(&task_id_owned, json).await;
                    }
                }
            }
        });
        let sender = StreamEventSender::new(tx);

        // AgentLoop 实例（统一循环框架：委托策略——角色过滤工具执行、
        // 不索引 FTS（ADR-026）、max_turns 尽力而为）
        let agent_loop = AgentLoop::new(
            model_service,
            self.clone(),
            session_manager,
            AgentLoopConfig { max_turns },
        )
        .with_turn_policy(TurnPolicy {
            tool_executor: ToolExecutorKind::RoleFiltered {
                role_tools,
                role_name,
            },
            persist_no_fts: true,
            max_turns_graceful: true,
            tools: Some(delegate_tools),
        });

        let result = match timeout_secs {
            Some(secs) => {
                let run = agent_loop.run_stream(
                    &mut messages,
                    sender,
                    task_id,
                    None,
                    &model,
                    Some(cancel.as_ref()),
                    None,
                );
                match tokio::time::timeout(std::time::Duration::from_secs(secs), run).await {
                    Ok(inner) => inner,
                    Err(_) => {
                        // 超时：置位取消标志（AgentLoop 轮顶/工具执行边界响应），
                        // 返回超时错误（watcher 标 fail + 通知主 agent）
                        cancel.store(true, Ordering::Relaxed);
                        return Err(TianyanError::Custom(format!(
                            "tool: 委托执行超时（{secs}s）"
                        )));
                    }
                }
            }
            None => {
                agent_loop
                    .run_stream(
                        &mut messages,
                        sender,
                        task_id,
                        None,
                        &model,
                        Some(cancel.as_ref()),
                        None,
                    )
                    .await
            }
        }?;

        // 结果转换：AgentLoopResult → 任务结果（watcher 写入任务状态机）
        match result {
            AgentLoopResult::Answer {
                content,
                total_tokens,
                ..
            } => {
                // ADR-030：submit_result 暂存结果优先（模型最后输出兜底）
                let result = self
                    .background_tasks
                    .staged_result(task_id)
                    .await
                    .map(|s| s.result)
                    .unwrap_or(content);
                Ok(serde_json::json!({
                "result": result,
                    "submitted": true,
                    "total_tokens": total_tokens.total_tokens,
                }))
            }
            AgentLoopResult::MaxTurnsReached {
                content,
                total_tokens,
                ..
            } => Ok(serde_json::json!({
                "result": content,
                "submitted": false,
                "total_tokens": total_tokens.total_tokens,
            })),
            AgentLoopResult::Cancelled { .. } => {
                Err(TianyanError::cancelled("委托已取消（主循环停止）"))
            }
        }
    }

    /// 执行子 Agent 工具调用（含角色白名单过滤）。
    ///
    /// `role_tools` 为 Some 时，白名单外的工具调用**不执行**，而是返回合成
    /// 错误结果（保留 call_id，错误消息作为普通工具结果回喂模型，模型可换用
    /// 允许的工具重试）；白名单内工具正常并行执行。`role_tools` 为 None 时
    /// 不限制（与主 Agent 相同）。前台/后台共用此路径，过滤语义一致。
    /// pub(crate)：ADR-030 统一循环框架——AgentLoop 按 TurnPolicy 调用。
    pub(crate) async fn execute_tool_calls_with_role(
        &self,
        tool_calls: &[ToolCall],
        session_id: &str,
        role_tools: Option<&[String]>,
        role_name: Option<&str>,
    ) -> Vec<(String, ToolExecutionOutcome)> {
        let Some(allowed) = role_tools else {
            return self.execute_parallel(tool_calls, session_id, true).await;
        };

        let (allowed_refs, blocked_refs): (Vec<&ToolCall>, Vec<&ToolCall>) = tool_calls
            .iter()
            .partition(|call| allowed.iter().any(|t| t == &call.function.name));
        let allowed_calls: Vec<ToolCall> = allowed_refs.into_iter().cloned().collect();

        let mut results = self
            .execute_parallel(&allowed_calls, session_id, true)
            .await;
        for call in blocked_refs {
            results.push((
                call.id.clone(),
                ToolExecutionOutcome {
                    duration_ms: 0,
                    result: Err(TianyanError::Custom(format!(
                        "tool: 角色 {} 不允许使用工具 {}（允许：{}）",
                        role_name.unwrap_or("?"),
                        call.function.name,
                        allowed.join("、")
                    ))),
                },
            ));
        }
        results
    }

    /// 后台委托（ADR-026 唯一路径）：排队槽位 → 注册（Pending）→ 运行许可 →
    /// spawn 独立执行 → 立即返回 task_id。
    ///
    /// session_id 由调用链显式传入（父会话归属），不再依赖共享可变状态。
    /// depth_guard 移入任务闭包持有到任务结束（含排队等待期间），
    /// 保证嵌套委托深度计数在任务生命周期内有效。
    async fn spawn_background_delegate(
        &self,
        params: &DelegateToAgentParams,
        session_id: &str,
        depth_guard: DelegationDepthGuard,
    ) -> Result<serde_json::Value, TianyanError> {
        let manager = &self.background_tasks;

        // 父会话归属：调用链显式传递（AgentLoop 经 execute_parallel 传入当前会话）
        let session_id = session_id.to_string();

        // ADR-026 排队模型：先拿排队槽位（满则拒绝）→ 注册（Pending，面板可见）
        // → 等待运行许可（阻塞排队）→ 标记运行。许可随任务闭包持有，任务结束释放。
        let queue_permit = manager.acquire_queue_slot().await?;

        let desc = params.task.clone();
        // 时序锚点：任务启动时链上消息数（= 下一条消息的 seq）。
        // 回退到用户输入 U 时，锚点 > U.seq 的任务属于"回退点之后"，应一并取消。
        let anchor_seq = match &self.session_manager {
            Some(sm) => match sm.get_session(&session_id).await {
                Ok(Some(s)) => s.messages.len() as i64,
                _ => 0,
            },
            None => 0,
        };
        let task_id = manager
            .register(
                crate::agent::background::TaskKind::Delegate,
                desc.clone(),
                session_id.clone(),
                anchor_seq,
            )
            .await;
        let run_permit = manager.acquire_run().await?;
        manager.mark_running(&task_id).await;

        // ADR-026：创建子智能体会话（会话 id = task_id，关联主会话；
        // 消息流走会话存储，面板可复用主对话流渲染）。上限保护（B）惰性触发。
        if let Some(store) = &self.session_store {
            let header = crate::session::types::SessionHeader {
                parent_session_id: Some(session_id.clone()),
                kind: Some("delegate".to_string()),
                role: params.role.clone(),
                ..crate::session::types::SessionHeader::default()
            };
            if let Err(e) = store.create(&task_id, &header).await {
                tracing::warn!(task_id, error = %e, "子智能体会话创建失败");
            }
            if let Err(e) = store.enforce_child_session_limit(300).await {
                tracing::warn!(error = %e, "子智能体会话上限清理失败");
            }
        }

        let this = self.clone();
        let mgr = manager.clone();
        let run_params = params.clone();
        let run_task_id = task_id.clone();
        let run_session_id = session_id.clone();

        // ADR-028：watcher 模式——任务主体只返回 Result，watcher 等 JoinHandle
        // 捕获 panic（JoinHandle.await 在任务任何方式结束时 resolve：正常/panic/abort），
        // panic 也转 fail + 通知，任务不悬死。watcher 协程挂起不占线程，
        // 主调用链 spawn 后立即返回。
        let task_id_for_loop = run_task_id.clone();
        let handle = tokio::spawn(async move {
            // 许可 + 深度 guard 在任务运行期间持有（作用域结束释放）
            let _permits = (queue_permit, run_permit);
            let _depth_guard = depth_guard;
            this.run_subagent_loop(&run_params, &run_session_id, &task_id_for_loop)
                .await
        });
        tokio::spawn(async move {
            match handle.await {
                Ok(Ok(v)) => mgr.complete(&run_task_id, format!("{v}")).await,
                Ok(Err(e)) => mgr.fail(&run_task_id, e.to_string()).await,
                Err(join_err) => {
                    tracing::error!(
                        task_id = %run_task_id,
                        error = %join_err,
                        "后台委托任务异常终止（panic）"
                    );
                    mgr.fail(&run_task_id, format!("任务进程异常终止: {join_err}"))
                        .await;
                }
            }
        });

        Ok(serde_json::json!({
            "task_id": task_id,
            "status": "running",
            "message": format!(
                "后台任务已启动（{desc}）。完成后将自动通知本会话，无需轮询；可用 task_status 查询状态。"
            ),
        }))
    }

    /// 执行 submit_result 工具（ADR-030：可选结果落盘工具）。
    ///
    /// 子代理把最终结果写入任务存储（暂存，complete 时优先使用），返回
    /// task_id 供主 agent 用 task_status 工具查询。收尾统一为"无工具调用"：
    /// 模型调用本工具后继续输出（告知主 agent 结果 ID），无工具调用时
    /// 任务完成（暂存结果优先，模型最后输出兜底）。
    pub(crate) async fn execute_submit_result(
        &self,
        arguments: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SubmitResultParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: submit_result 参数无效：{e}")))?;
        // 子代理 = 会话：task_id 即 session_id（ADR-026）
        self.background_tasks
            .stage_result(session_id, params.result, params.summary)
            .await?;
        Ok(serde_json::json!({
            "task_id": session_id,
            "status": "staged",
            "message": "结果已写入任务存储。请在最终回复中告知主智能体结果 ID（task_id），主智能体可用 task_status 工具查询。",
        }))
    }

    /// 执行 task_status 工具：查询后台任务状态与结果（统一：委托 + 命令）。
    ///
    /// 参数：task_id 可选——给定则查询单任务（委托/命令自动识别）；
    /// 缺省则列出全部任务（kind 可选过滤：delegate | command）。
    pub(crate) async fn execute_task_status(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        // 与 schema 共用同一参数类型（task_id 可选 = 列表模式 + kind 过滤），
        // 避免本地重复定义导致 schema 与行为漂移
        let params: TaskStatusParams = parse_params(arguments)?;

        // 单任务查询：按 ID 先查委托表再查命令表（前缀不互斥，双表探测）。
        if let Some(task_id) = params.task_id.as_deref() {
            if let Some(task) = self.background_tasks.get(task_id).await {
                return serde_json::to_value(task)
                    .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")));
            }
            if let Some(task) = self.command_tasks.get(task_id).await {
                return serde_json::to_value(task)
                    .map_err(|e| TianyanError::Custom(format!("tool: 序列化失败：{e}")));
            }
            return Err(TianyanError::Custom(format!("tool: 任务不存在：{task_id}")));
        }

        // 列表：委托任务 + 命令任务（kind 过滤）。
        let mut entries: Vec<serde_json::Value> = Vec::new();
        let kind_filter = params.kind.as_deref().unwrap_or("");
        if kind_filter.is_empty() || kind_filter == "delegate" {
            for t in self.background_tasks.snapshot().await {
                if let Ok(v) = serde_json::to_value(t) {
                    entries.push(v);
                }
            }
        }
        if kind_filter.is_empty() || kind_filter == "command" {
            for t in self.command_tasks.list().await {
                if let Ok(v) = serde_json::to_value(t) {
                    entries.push(v);
                }
            }
        }
        Ok(serde_json::json!({ "tasks": entries, "total": entries.len() }))
    }

    /// 执行 task_cancel 工具：取消后台任务（终态任务为幂等空操作）。
    pub(crate) async fn execute_task_cancel(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: TaskCancelParams = parse_params(arguments)?;

        self.background_tasks.cancel(&params.task_id).await?;

        Ok(serde_json::json!({
            "task_id": params.task_id,
            "status": "cancelled",
            "message": "后台任务已取消",
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
        let params: SuggestRoleParams = parse_params(arguments)?;

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
