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
//! use tianyan::agent::{Agent, AgentCoordinator};
//! use tianyan::common::types::Message;
//!
//! async fn example(agent: &Agent) -> Result<(), Box<dyn std::error::Error>> {
//!     let message = Message::user("帮我分析项目性能问题");
//!     let response = agent.process_message("session-001", &message, None, None).await?;
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
//! use tianyan::agent::{Agent, AgentCoordinator};
//! use tianyan::common::types::Message;
//!
//! async fn chat_with_agent(
//!     agent: &Agent,
//!     user_input: &str,
//! ) -> Result<String, Box<dyn std::error::Error>> {
//!     let response = agent
//!         .process_message("session-001", &Message::user(user_input), None, None)
//!         .await?;
//!     Ok(response.content)
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
//!                      └─→ Answer
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
//! - **追问处理**：ask_user 作为普通工具同步等待用户回答（回答作为工具结果）
//! - **执行失败**：记录日志并返回友好的错误消息
//! - **资源限制**：当达到最大迭代次数时生成错误响应

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;

use crate::agent::agent_core::Agent;
use crate::agent::stream_forward::spawn_null_forwarder;
use crate::agent::types::{AgentResponse, AgentState, StreamEventSender};
use crate::common::error::Result;
use crate::common::types::Message;

/// 智能体协调器 trait。
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// 请求中止该会话**当前活动轮**（ADR-035 §9 / U10：「停止」端点用）。
    ///
    /// 语义：置位轮自己注册的会话取消槽（唤醒轮/后台轮场景——无用户请求级
    /// 标志）；返回是否命中（false = 无活动轮可停）。默认实现 = 无操作
    /// （向导模式/测试桩无需感知）。
    async fn cancel_active_turn(&self, _session_id: &str) -> bool {
        false
    }

    /// 停止**所有**会话的当前活动轮（服务关停时主动收尾）。默认 = 无操作。
    async fn cancel_all_active_turns(&self) -> usize {
        0
    }

    /// 该会话当前是否有活动轮（权威查询；纯通知状态模型的快照校正来源）。
    /// 默认实现返回 false（向导模式/测试桩无需感知）。
    async fn has_active_turn(&self, _session_id: &str) -> bool {
        false
    }

    /// 处理用户消息。
    /// - `message` — 完整消息（含可选的多模态图片片段，`content` 为纯文本）。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    /// - `thinking_effort` — 本会话思考强度（会话时选择；None 时使用模型默认）。
    async fn process_message(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
        thinking_effort: Option<String>,
    ) -> Result<AgentResponse>;

    /// 处理用户消息（流式响应）。
    /// - `message` — 完整消息（含可选的多模态图片片段）。
    /// - `model` — 可选指定模型，None 时使用默认配置。
    /// - `cancel` — 取消标志（客户端断开/服务关停时置位）；`None` 表示不可取消。
    /// - `thinking_effort` — 本会话思考强度（会话时选择；None 时使用模型默认）。
    /// - `user_message_id` — 前端生成的用户消息临时 id（ADR-031 乐观渲染
    ///   定位键；落库后经 UserMessageId 确认事件回显真实 id，此后弃用）。
    /// - `sender` — 流式事件出口（ADR-032：由调用方经
    ///   [`crate::agent::spawn_stream_forwarder`] 创建并**先于本调用**开始
    ///   消费——通道所有权归调用方，与唤醒轮 / 子代理路径完全同构）。
    ///
    /// 本方法**等待整轮结束**（不内部 spawn）；需要非阻塞语义的调用方
    /// 自行 spawn（server 侧在请求协程内 spawn 后立即返回 HTTP 响应）。
    #[allow(clippy::too_many_arguments)]
    async fn process_message_stream(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
        cancel: Option<Arc<AtomicBool>>,
        thinking_effort: Option<String>,
        user_message_id: Option<&str>,
        sender: StreamEventSender,
    ) -> Result<()>;

    /// 初始化智能体。
    ///
    /// 默认空实现（无初始化资源的适配器/测试替身直接继承；
    /// Agent 实现保留日志后覆盖）。
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    /// 获取审批状态快照（工作流配置 + 待处理请求 + 最近审计记录）。
    ///
    /// 用于状态查询接口；审批工作流未装配时返回默认配置与空队列。
    async fn approval_status(&self) -> Result<crate::executor::approval::ApprovalStatusSnapshot>;

    /// 响应待处理审批请求（GUI 审批面板调用）。
    ///
    /// `edited_command` 为用户在审批面板编辑后的命令（纠正/改写场景；
    /// 仅 decision=Approve 且提供时生效，记入审计记录与通知文本，
    /// 不影响实际执行——执行仍由 agent 按原命令发起）。
    /// 请求不存在或已超时返回 not_found 语义错误（调用方以 is_not_found 映射 404）。
    async fn respond_approval(
        &self,
        request_id: &str,
        decision: crate::executor::approval::ApprovalDecision,
        reason: Option<String>,
        edited_command: Option<String>,
    ) -> Result<()>;

    /// 获取智能体状态。
    async fn get_state(&self) -> AgentState;

    /// 获取已注册工具定义（内置 + 动态注册，如 `clipboard_write` / MCP 桥接）。
    ///
    /// 用于组合根接线验证（server 层测试断言动态工具已装配）。
    /// 默认空实现（无工具装配的适配器/测试替身直接继承）。
    async fn tool_definitions(&self) -> Vec<crate::model::types::ToolDefinition> {
        Vec::new()
    }

    /// 运行时注册动态工具（server 层在 agent 构建后追加工具，如 schedule_task）。
    /// 默认空实现（无工具装配的适配器/测试替身直接继承）。
    async fn register_dynamic_tools(
        &self,
        _tools: Vec<Arc<dyn crate::agent::DynamicToolExecutor>>,
    ) {
    }

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
    /// 返回 `Some(summary_sm)` = 实际发生压缩（摘要消息已持久化）；
    /// `None` = 无需压缩（消息不足 / token 未超阈值）。
    async fn compress_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::common::types::StructuredMessage>>;

    /// 回退会话到指定消息之前（完整回退事务，ADR-040）。
    ///
    /// 全过程内聚在 core（`Agent::rollback_to`），一把会话锁覆盖「取消轮与任务 →
    /// 保存重做数据 → 恢复工作区文件 → 截断时序链」：调用方（server handler）只
    /// 负责置位取消标志、调用与响应映射，不再自己编排截断（此前截断在锁外执行，
    /// 窗口内新轮落库的消息会被整表重写覆盖）。
    ///
    /// 默认实现返回空结果（无快照/任务装配的适配器与测试替身直接继承）。
    async fn rollback_to(
        &self,
        _session_id: &str,
        _message_id: &str,
    ) -> Result<crate::agent::rollback::RollbackOutcome> {
        Ok(crate::agent::rollback::RollbackOutcome::default())
    }

    /// 重做被回退的消息与工作区文件（与回退同一排他事务入口，ADR-040）。
    ///
    /// 默认实现返回空结果（无快照装配的适配器与测试替身直接继承）。
    async fn redo_to(
        &self,
        _session_id: &str,
        _message_id: &str,
    ) -> Result<crate::agent::rollback::RedoOutcome> {
        Ok(crate::agent::rollback::RedoOutcome::default())
    }

    /// 唤醒指定会话的主 agent（T1 事件驱动：外部事件触发一轮系统消息处理）。
    ///
    /// 默认空实现（不唤醒）；Agent 实现转发到 `process_wake`（ADR-013）。
    async fn wake_session(&self, _session_id: &str) {}

    /// 关闭智能体。
    ///
    /// 默认空实现（无持有资源的适配器/测试替身直接继承；
    /// Agent 实现保留日志后覆盖）。
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl AgentCoordinator for Agent {
    async fn cancel_active_turn(&self, session_id: &str) -> bool {
        // ADR-035 §9 / U10：「停止」端点经此置位唤醒轮/后台轮自己注册的取消槽
        Agent::cancel_active_turn(self, session_id).await
    }

    async fn cancel_all_active_turns(&self) -> usize {
        Agent::cancel_all_active_turns(self).await
    }

    async fn has_active_turn(&self, session_id: &str) -> bool {
        Agent::has_active_turn(self, session_id).await
    }

    async fn process_message(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
        thinking_effort: Option<String>,
    ) -> Result<AgentResponse> {
        let start = Instant::now();

        // ADR-013 串行化：单会话同一时刻只有一个活动轮（用户轮 / 唤醒轮互斥）
        let _turn_guard = self.turn_guard(session_id).await;

        // Resolve model: caller-specified > Agent default config
        let model = model.unwrap_or(&self.default_model);

        // 1. Load session and build state
        let state = self.load_and_build_state(session_id).await?;

        // 2-6. 共享编排骨架：快照 → 持久化 → 上下文 → AgentLoop → 结果 → 指标 → 压缩
        // 非流式路径无可取消源（HTTP 请求生命周期内），传 None
        let response = self
            .run_agent_turn(
                &state,
                session_id,
                message,
                model,
                start,
                None,
                None, // 非流式路径无乐观渲染确认（响应直接携带消息 id）
                crate::agent::agent_core::TurnOptions {
                    mode: crate::agent::agent_core::TurnMode::Plain,
                    do_snapshot: true,
                    do_compress: true,
                    do_toolset_check: true,
                    thinking_effort,
                },
            )
            .await?;

        Ok(response)
    }

    async fn process_message_stream(
        &self,
        session_id: &str,
        message: &Message,
        model: Option<&str>,
        cancel: Option<Arc<AtomicBool>>,
        thinking_effort: Option<String>,
        user_message_id: Option<&str>,
        sender: StreamEventSender,
    ) -> Result<()> {
        let model = model.unwrap_or(&self.default_model).to_string();
        // ADR-013 串行化：单会话同一时刻只有一个活动轮（用户轮 / 唤醒轮互斥）。
        // 锁必须先于状态加载：并发轮（如唤醒轮进行中又来用户消息）若在锁外
        // 加载状态，会拿到前一轮完成前的旧快照——上下文缺失前一轮消息，
        // 且状态与库分叉。非流式路径（process_message）已是锁内加载，此处对齐。
        let _turn_guard = self.turn_guard(session_id).await;
        // ADR-035 §9：轮状态 running（auto=false 用户轮）——前端据此联动
        // 输入框与停止按钮（U10）。配对 idle 在函数退出时发出。
        sender.send_turn_state(true, false).await;

        // 状态加载失败：经流式通道下发错误事件（调用方映射为 error 事件 +
        // HTTP 错误，与锁外加载返回 Err 的语义一致）。
        let state = match self.load_and_build_state(session_id).await {
            Ok(s) => s,
            Err(e) => {
                sender.send_turn_state(false, false).await;
                sender.send_error(&format!("处理失败：{}", e)).await;
                return Ok(());
            }
        };

        let start = Instant::now();
        // 注入取消标志：工具执行（含同步委托）期间也能响应停止
        // （此前 cancel 只在 chunk 循环/轮顶检查，长工具调用点停止无效）。
        self.agent_loop
            .tool_registry()
            .set_delegation_cancel(session_id, cancel.clone())
            .await;
        // 共享编排骨架：快照 → 持久化 → 上下文 → run_stream → 流式事件 → 指标 → 压缩
        // （do_compress = true：修复流式路径缺失压缩检查的漂移，与 process_message 对齐）
        let _ = self
            .run_agent_turn(
                &state,
                session_id,
                message,
                &model,
                start,
                cancel.as_deref(),
                user_message_id,
                crate::agent::agent_core::TurnOptions {
                    mode: crate::agent::agent_core::TurnMode::Stream {
                        sender: sender.clone(),
                    },
                    do_snapshot: true,
                    do_compress: true,
                    do_toolset_check: true,
                    thinking_effort,
                },
            )
            .await;
        // 清理会话取消槽：仅正常结束时清理。用户停止（cancel 置位）时保留——
        // 已转入后台的委托子代理仍要看到取消标志并退出（否则"停止"对
        // 后台任务无效）；下次请求会注入新的取消槽覆盖本槽。
        let cancelled = cancel
            .as_ref()
            .map(|c| c.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(false);
        if !cancelled {
            self.agent_loop
                .tool_registry()
                .set_delegation_cancel(session_id, None)
                .await;
        }
        // ADR-035 §9：轮状态 idle（与开头 running 配对）
        sender.send_turn_state(false, false).await;

        Ok(())
    }

    async fn compress_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::common::types::StructuredMessage>> {
        // ADR-040 §1：压缩是「读链 → 生成摘要 → 写链」的 read-modify-write，
        // 必须与回退 / 轮同处一把锁的排他事务内（顺带保证读基准是最新链：
        // 与轮并发会基于轮中未完成的消息生成摘要，且轮内状态与库分叉）。
        // force=true：手动压缩跳过窗口阈值——用户主动点击即明确意图。
        self.working_sets
            .with_session_exclusive(session_id, |ws| async move {
                let state = ws.state();
                Ok(self
                    .maybe_compress_and_persist(&state, session_id, true)
                    .await)
            })
            .await
    }

    async fn rollback_to(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<crate::agent::rollback::RollbackOutcome> {
        Agent::rollback_to(self, session_id, message_id).await
    }

    async fn redo_to(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<crate::agent::rollback::RedoOutcome> {
        Agent::redo_to(self, session_id, message_id).await
    }

    async fn wake_session(&self, session_id: &str) {
        // 无事件转发通道（webhook 唤醒路径）：统一 null 转发器——消费端
        // 存活（持续排空，不阻塞发送端），事件静默丢弃，唤醒轮本身正常执行。
        let (sender, _handle) = spawn_null_forwarder(session_id);
        self.process_wake(session_id, sender).await;
    }

    async fn initialize(&self) -> Result<()> {
        tracing::info!("Agent initialized with AgentLoop architecture");
        Ok(())
    }

    async fn approval_status(&self) -> Result<crate::executor::approval::ApprovalStatusSnapshot> {
        use crate::executor::approval::{ApprovalStatusSnapshot, ApprovalWorkflowConfig};

        let pending_confirmations = self.pending_approval_fingerprints.lock().await.clone();
        let Some(workflow) = &self.approval_workflow else {
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
        edited_command: Option<String>,
    ) -> Result<()> {
        let Some(workflow) = &self.approval_workflow else {
            return Err(crate::TianyanError::Custom(
                "审批流程未装配（审批工作流不可用）".to_string(),
            ));
        };
        // GUI 人工响应：approved_by 固定为 "user"
        workflow
            .respond_to_approval(request_id, decision, reason, "user", edited_command)
            .await
    }

    async fn get_state(&self) -> AgentState {
        self.state.read().await.clone()
    }

    async fn tool_definitions(&self) -> Vec<crate::model::types::ToolDefinition> {
        self.agent_loop.tool_registry().definitions().await
    }

    async fn register_dynamic_tools(&self, tools: Vec<Arc<dyn crate::agent::DynamicToolExecutor>>) {
        self.register_dynamic_tools(tools).await
    }

    async fn background_tasks(&self) -> Vec<crate::agent::background::BackgroundTask> {
        let mut all = self.background_tasks.snapshot().await;
        // 合并后台终端命令（execute_command background）：与委托任务共用
        // 同一面板视图与取消入口（此前命令任务仅 agent 内 task_status 可见）。
        // 注意：命令条目来自进程内存（不落 SQL，ADR-026 §5 D 边界澄清）——
        // 仅本次运行可见、重启即清空；委托任务走 SQL 快照（可跨重启恢复）。
        for t in self.command_tasks.list().await {
            all.push(crate::agent::background::BackgroundTask::from_command_task(
                t,
            ));
        }
        all
    }

    async fn cancel_background_task(&self, task_id: &str) -> Result<bool> {
        if self.background_tasks.get(task_id).await.is_some() {
            self.background_tasks.cancel(task_id).await?;
            return Ok(true);
        }
        // 后台终端命令（cmd_ 前缀）：kill 终态幂等；未知 id Err → 不存在
        match self.command_tasks.kill(task_id).await {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn shutdown(&self) -> Result<()> {
        tracing::info!("Shutting down agent");
        Ok(())
    }
}
