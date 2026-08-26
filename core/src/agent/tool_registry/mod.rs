use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::agent::RoleRegistry;
use crate::common::error::TianyanError;
use crate::executor::approval::{ApprovalDecision, ApprovalWorkflow};
use crate::executor::web::WebSearchClient;
use crate::executor::Action;
use crate::executor::SecurityPolicy;
use crate::executor::VerificationGate;
use crate::knowledge::KnowledgeIngestor;
use crate::lsp::diagnostics::LspManager;
use crate::model::types::{ToolCall, ToolDefinition, ToolPresentation};
use crate::model::ChatService;
use crate::observability::execution_log::ExecutionLog;
use crate::observability::trace::TraceCollector;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::session::search::SessionRecall;

use crate::skills::learning::ExecutionHistory;
use crate::skills::SkillExecutor;
use crate::vfs::VirtualFileSystem;

mod agent_ops;
/// 内置工具元数据单一事实源（H-C1：schema/展示意图/短路清单）。
mod builtin_tools;
mod code_ops;
/// 演化数据查询工具执行器（ADR-017 GEPA 数据层：execution_stats 等）。
mod evolution_ops;
/// 工具执行器实现（按工具域拆分，14 个 `execute_*` 方法）。
mod file_ops;
/// 文件系统浏览工具执行器（glob / list_dir）。
mod fs_ops;
mod knowledge_ops;
/// LSP 工具执行器（execute_lsp）。
mod lsp_ops;
/// 内置可观测性后置监听器（A1：统计/Trace/GEPA/规则学习迁移为管线消费者）。
mod observability;
/// 工具执行管线：pre-execute 监听器 / 单调守卫 / post-execute 监听器（A1/A4）。
mod pipeline;
/// 符号大纲工具执行器（symbol_outline）。
mod symbol_ops;
/// 测试发现工具执行器（discover_tests）。
mod test_ops;

pub use pipeline::{ToolGuard, ToolPostExecuteListener, ToolPreExecuteListener};

use builtin_tools::BUILTIN_TOOLS;

/// 将 VFS 层级内容读取结果转换为 JSON 字段值。
///
/// - 内容存在 → 原文
/// - 该层级无内容（not_found，正常状态）→ 空字符串
/// - 真实读取错误 → 带 `<读取失败: ...>` 标记的错误文本，
///   让 LLM 能区分"无内容"与"存储故障"，而不是静默吞掉错误
fn vfs_content_field(result: crate::common::error::Result<String>) -> String {
    match result {
        Ok(content) => content,
        Err(e) if e.is_not_found() => String::new(),
        Err(e) => format!("<读取失败: {}>", e),
    }
}

/// 解析工具参数（JSON → 类型化参数）。
///
/// 所有工具的参数解析共用同一错误包装（"tool: 参数无效"），
/// 提取为单点避免多份重复样板。
fn parse_params<T: serde::de::DeserializeOwned>(arguments: &str) -> Result<T, TianyanError> {
    serde_json::from_str(arguments)
        .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))
}

/// 包装工具执行错误，保留底层错误的语义分类（ADR-014）。
///
/// 底层（executor/edit/patch 等）已用 `not_found` / `conflict` / `invalid_input` /
/// `permission` / `timeout` 构造器分类的错误，经此包装后谓词（`is_not_found` 等）
/// 依然可命中——否则 `Custom("tool: 执行失败：...")` 前缀会掩盖分类，
/// server 层只能映射为 500 而非 404/409。未分类错误保持原消息不变。
fn wrap_tool_error(e: TianyanError) -> TianyanError {
    let detail = format!("tool: 执行失败：{e}");
    if e.is_not_found() {
        TianyanError::not_found(detail)
    } else if e.is_conflict() {
        TianyanError::conflict(detail)
    } else if e.is_invalid_input() {
        TianyanError::invalid_input(detail)
    } else if e.is_permission() {
        TianyanError::permission(detail)
    } else if e.is_timeout() {
        TianyanError::timeout(detail)
    } else {
        TianyanError::Custom(detail)
    }
}

/// 将安全策略检查错误包装为统一的安全违规工具错误。
fn safety_violation<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, TianyanError> {
    result.map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))
}

/// 截断工具参数摘要（G6 trace 观测数据控制体积；UTF-8 边界安全）。
fn truncate_trace_params(arguments: &str) -> String {
    const MAX: usize = 200;
    if arguments.len() <= MAX {
        return arguments.to_string();
    }
    let mut end = MAX;
    while !arguments.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &arguments[..end])
}

/// 单个工具调用的执行结果（含耗时元数据）。
///
/// 耗时是消息级计时的输入源：主循环据此构造 `Part::ToolResult.time`
/// 与 SSE tool_result 事件，前端在工具卡片上显示耗时/成败。
#[derive(Debug)]
pub struct ToolExecutionOutcome {
    /// 执行耗时（毫秒）。
    pub duration_ms: i64,
    /// 执行结果（成功为 JSON 值；失败为结构化错误）。
    pub result: Result<serde_json::Value, TianyanError>,
}

impl ToolExecutionOutcome {
    /// 是否执行成功。
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// 失败原因（成功时为 None）。
    pub fn error(&self) -> Option<String> {
        self.result.as_ref().err().map(|e| e.to_string())
    }

    /// 借用内部结果（镜像 `Result::as_ref`，消费方无需触碰字段）。
    pub fn as_ref(&self) -> Result<&serde_json::Value, &TianyanError> {
        self.result.as_ref()
    }

    /// 是否失败。
    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }
}

/// 动态工具执行器：供外部桥接（如 MCP 工具桥接）扩展工具注册表。
///
/// 注册的动态工具与内置工具享有同等地位：定义进入 LLM 可见的
/// tool definitions，执行走 `execute_single` 的统一路径（含统计与失败学习）。
#[async_trait]
pub trait DynamicToolExecutor: Send + Sync {
    /// 工具名称（须与 [`Self::definition`] 中的 name 一致）。
    fn tool_name(&self) -> String;

    /// 工具定义（OpenAI function calling schema）。
    fn definition(&self) -> ToolDefinition;

    /// 执行工具调用。
    async fn execute(&self, arguments: &str) -> crate::common::error::Result<serde_json::Value>;
}

/// 工具注册表，维护工具定义并并行执行 tool_calls。
#[derive(Clone)]
pub struct ToolRegistry {
    pub(crate) security_policy: SecurityPolicy,
    pub(crate) skill_executor: Option<Arc<SkillExecutor>>,
    pub(crate) knowledge_ingestor: Option<Arc<KnowledgeIngestor>>,
    pub(crate) vfs: Option<Arc<dyn VirtualFileSystem>>,
    pub(crate) model_service: Option<Arc<dyn ChatService>>,
    /// 委托子 Agent 时使用的模型名称。
    pub(crate) model: String,
    pub(crate) metrics: Option<Arc<AgentMetrics>>,
    /// 审批工作流（write_file / apply_edit / apply_patch / execute_command /
    /// run_tests / verify_build 危险操作门控，统一经 [`Self::ensure_approved`]）。
    pub(crate) approval_workflow: Option<Arc<ApprovalWorkflow>>,
    /// 待用户确认的操作指纹（审批拒绝 → 询问用户 → 用户回答后确认/放弃）。
    pub(crate) pending_approval_fingerprints: Arc<Mutex<Vec<String>>>,
    /// 语义验证门控（verify_build 工具的 LLM-as-Judge 支持）。
    pub(crate) verification_gate: Option<Arc<VerificationGate>>,
    /// LSP 管理器（lsp 工具与编辑后诊断附加依赖；None 表示未启用 LSP）。
    pub(crate) lsp_manager: Option<Arc<LspManager>>,
    /// 内置可观测性后置监听器（统计 / Trace / GEPA 历史 / 失败规则学习；
    /// A1 迁移：原 execute_single 尾部观测逻辑）。
    pub(crate) observability: observability::ToolObservabilityListener,
    /// Web 搜索/抓取客户端（web_search / web_fetch 工具依赖）。
    pub(crate) web_client: Option<Arc<WebSearchClient>>,
    /// 后台任务管理器（delegate_to_agent(background) / task_status / task_cancel）。
    pub(crate) background_tasks: Arc<crate::agent::background::BackgroundTaskManager>,
    /// 后台命令管理器（execute_command(background)；查询/终止经统一 task_status / task_cancel）。
    pub(crate) command_tasks: Arc<crate::executor::CommandManager>,
    /// 角色向量路由器（ADR-016 P3：suggest_role 工具依赖；None 时工具不可用）。
    pub(crate) role_router: Option<Arc<crate::agent::role_router::RoleRouter>>,
    /// 执行记录日志（ADR-017 GEPA 数据层：execution_stats 等工具依赖；None 时工具不可用）。
    pub(crate) execution_log: Option<Arc<ExecutionLog>>,
    /// 会话回忆服务（ADR-017 决策 6：session_recall 工具依赖；None 时工具不可用）。
    pub(crate) session_recall: Option<Arc<SessionRecall>>,
    /// 会话权威存储（ADR-018：vfs_read 读 tianyan://session/{id} 的兼容层依赖；None 时不可用）。
    pub(crate) session_store: Option<Arc<crate::session::store::SessionStore>>,
    /// 工具执行管线：pre-execute 监听器（fail-closed，按注册顺序；A1）。
    pre_execute_listeners: Vec<Arc<dyn ToolPreExecuteListener>>,
    /// 工具执行管线：单调守卫（只允许拒绝；A4）。
    guards: Vec<Arc<dyn ToolGuard>>,
    /// 工具执行管线：post-execute 监听器（按注册顺序；首个为内置可观测性监听器）。
    post_execute_listeners: Vec<Arc<dyn ToolPostExecuteListener>>,
    /// 工具 UI 展示意图映射（A2；工具名 → card 类型，不进入 LLM schema）。
    presentations: HashMap<String, ToolPresentation>,
    definitions: Vec<ToolDefinition>,
    /// 外部注册的动态工具（如 MCP 工具桥接），按工具名索引。
    dynamic_tools: Arc<Mutex<HashMap<String, Arc<dyn DynamicToolExecutor>>>>,
    /// 当前委托链深度（delegate_to_agent 嵌套保护；主循环为 0）。
    pub(crate) delegation_depth: Arc<AtomicUsize>,
    /// 子 Agent 角色注册表（delegate_to_agent role 参数解析；默认内置角色）。
    pub(crate) role_registry: Arc<RoleRegistry>,
    /// 会话管理器（execute_command 默认 cwd 解析：模型未指定时使用会话
    /// 绑定的工作目录，而非进程当前目录）。
    pub(crate) session_manager: Option<Arc<dyn crate::session::SessionManager>>,
    /// 主循环取消标志槽（按会话：coordinator 每请求注入/清理；
    /// 委托循环据此中断——同步委托期间用户点停止也能及时停）。
    delegation_cancel: Arc<Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// 用户问题服务（ask_user 同步等待用户回答；None 时工具不可用）。
    pub(crate) user_questions: Option<Arc<crate::agent::user_questions::UserQuestionService>>,
}

impl ToolRegistry {
    /// 创建新的注册表并注册内置工具。
    pub fn new(security_policy: SecurityPolicy) -> Self {
        let mut registry = Self {
            security_policy,
            skill_executor: None,
            knowledge_ingestor: None,
            vfs: None,
            model_service: None,
            model: "default".to_string(),
            metrics: None,
            approval_workflow: None,
            pending_approval_fingerprints: Arc::new(Mutex::new(Vec::new())),
            verification_gate: None,
            lsp_manager: None,
            observability: observability::ToolObservabilityListener::default(),
            web_client: None,
            background_tasks: Arc::new(crate::agent::background::BackgroundTaskManager::new()),
            command_tasks: Arc::new(crate::executor::CommandManager::new(None)),
            role_router: None,
            execution_log: None,
            session_recall: None,
            session_store: None,
            pre_execute_listeners: Vec::new(),
            guards: Vec::new(),
            post_execute_listeners: Vec::new(),
            presentations: HashMap::new(),
            definitions: Vec::new(),
            dynamic_tools: Arc::new(Mutex::new(HashMap::new())),
            delegation_depth: Arc::new(AtomicUsize::new(0)),
            role_registry: Arc::new(RoleRegistry::builtin()),
            session_manager: None,
            delegation_cancel: Arc::new(Mutex::new(HashMap::new())),
            user_questions: None,
        };
        // A1：内置可观测性监听器注册为第一个 post-execute 监听器——
        // 原 execute_single 尾部的统计/Trace/GEPA/规则学习自此是管线消费者。
        registry
            .post_execute_listeners
            .push(Arc::new(registry.observability.clone()));
        registry.register_builtin_tools();
        registry
    }

    /// 设置技能执行器。
    pub fn with_skill_executor(mut self, executor: Arc<SkillExecutor>) -> Self {
        self.skill_executor = Some(executor);
        self
    }

    /// 设置知识导入器（knowledge_ingest 工具依赖）。
    pub fn with_knowledge_ingestor(mut self, ingestor: Arc<KnowledgeIngestor>) -> Self {
        self.knowledge_ingestor = Some(ingestor);
        self
    }

    /// 设置 VFS 引用（search_knowledge 工具依赖）。
    pub fn with_vfs(mut self, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    /// 设置模型服务（delegate_to_agent 工具依赖）。
    pub fn with_model_service(mut self, svc: Arc<dyn ChatService>) -> Self {
        self.model_service = Some(svc);
        self
    }

    /// 设置会话管理器（execute_command 默认 cwd 解析）。
    pub fn with_session_manager(mut self, sm: Arc<dyn crate::session::SessionManager>) -> Self {
        self.session_manager = Some(sm);
        self
    }

    /// 设置会话权威存储（vfs_read 对 tianyan://session/{id} 的兼容读取）。
    pub fn with_session_store(mut self, store: Arc<crate::session::store::SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// 设置用户问题服务（ask_user 同步等待用户回答；None 时工具不可用）。
    pub fn with_user_questions(
        mut self,
        service: Arc<crate::agent::user_questions::UserQuestionService>,
    ) -> Self {
        self.user_questions = Some(service);
        self
    }

    /// 解析工具文件路径：相对路径（非绝对）基于**会话绑定的工作目录**解析
    /// （缺省回退进程 cwd 语义，原样返回）；绝对路径原样返回。
    ///
    /// 与 execute_command 默认 cwd 同一套归属规则：模型在会话工作区内工作时，
    /// `read_file ".git/HEAD"`、`glob path="."` 等相对路径落在工作区而非
    /// 进程 cwd（如天演仓库根）。
    pub(crate) async fn resolve_tool_path(&self, session_id: &str, path: &str) -> String {
        let p = std::path::Path::new(path);
        if p.is_absolute() {
            return path.to_string();
        }
        if let Some(sm) = &self.session_manager {
            if let Ok(Some(session)) = sm.get_session(session_id).await {
                if let Some(wd) = session.working_directory(None) {
                    return wd.join(path).to_string_lossy().into_owned();
                }
            }
        }
        path.to_string()
    }

    /// 解析会话生效的基准目录：会话绑定的工作目录优先，缺省回退进程 cwd。
    ///
    /// 与 [`Self::resolve_tool_path`] 同一套归属规则；供 apply_patch 基准目录、
    /// glob 搜索根等需要「目录本身」（而非拼接路径）的场景使用。
    pub(crate) async fn resolve_base_dir(&self, session_id: &str) -> Result<PathBuf, TianyanError> {
        if let Some(sm) = &self.session_manager {
            if let Ok(Some(session)) = sm.get_session(session_id).await {
                if let Some(wd) = session.working_directory(None) {
                    return Ok(wd);
                }
            }
        }
        std::env::current_dir()
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：无法获取工作目录: {e}")))
    }

    /// 设置委托子 Agent 使用的模型名称。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 设置子 Agent 角色注册表（delegate_to_agent role 参数解析）。
    ///
    /// 缺省使用 [`RoleRegistry::builtin`]；配置装配层注入
    /// `[agent_roles]` 覆盖/扩展后的注册表。
    pub fn with_role_registry(mut self, registry: Arc<RoleRegistry>) -> Self {
        self.role_registry = registry;
        self
    }

    /// 设置角色向量路由器（suggest_role 工具依赖）。
    pub fn with_role_router(mut self, router: Arc<crate::agent::role_router::RoleRouter>) -> Self {
        self.role_router = Some(router);
        self
    }

    /// 设置后台命令日志目录（execute_command(background) 的日志文件落盘位置）。
    ///
    /// 缺省为 None（仅内存输出尾部缓冲，不落盘）。
    pub fn with_command_logs_dir(mut self, dir: PathBuf) -> Self {
        self.command_tasks = Arc::new(crate::executor::CommandManager::new(Some(dir)));
        self
    }

    /// 设置后台命令完成通知器（execute_command(background) 完成时注入父会话）。
    pub fn with_command_notifier(
        mut self,
        notifier: Arc<dyn crate::executor::CommandNotifier>,
    ) -> Self {
        self.command_tasks = Arc::new((*self.command_tasks).clone().with_notifier(notifier));
        self
    }

    /// 设置可观测性指标（self_check 工具依赖）。
    pub fn with_metrics(mut self, metrics: Arc<AgentMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 设置审批工作流（write_file / apply_edit / apply_patch / execute_command /
    /// run_tests / verify_build 危险操作门控，统一经 [`Self::ensure_approved`]）。
    pub fn with_approval_workflow(mut self, workflow: Arc<ApprovalWorkflow>) -> Self {
        self.approval_workflow = Some(workflow);
        self
    }

    /// 记录被审批拒绝、待用户确认的操作指纹。
    ///
    /// 由"询问用户"降级链路调用：agent loop 通过 [`Self::has_pending_approval`]
    /// 检测到审批拒绝后转为追问，用户回答后通过 [`Self::confirm_pending_approval`]
    /// 决定是否放行。
    pub async fn remember_pending_approval(&self, action: &Action) {
        let fp = ApprovalWorkflow::action_fingerprint(action);
        self.pending_approval_fingerprints.lock().await.push(fp);
        tracing::info!(action = ?action, "操作已进入待用户确认队列");
    }

    /// 审批门控：请求审批并处理拒绝降级。
    ///
    /// 统一 6 个危险工具（write_file / apply_edit / apply_patch / execute_command /
    /// run_tests / verify_build）的审批序列，消除各执行器内的逐字复制：
    /// 1. `approval_workflow` 未装配（None）时直接放行；
    /// 2. 子任务（subagent）非交互审批——未授权操作立即拒绝（ADR-011）；
    /// 3. 拒绝 → 记录待确认指纹（用户经"询问用户"链路批准后放行）并返回
    ///    统一"操作需要用户确认"错误。
    pub(crate) async fn ensure_approved(
        &self,
        session_id: &str,
        subagent: bool,
        action: &Action,
    ) -> Result<(), TianyanError> {
        let Some(ref approval) = self.approval_workflow else {
            return Ok(());
        };
        let approval_result = if subagent {
            approval.request_approval_no_wait(session_id, action).await
        } else {
            approval.request_approval(session_id, action).await
        };
        let resp = approval_result
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：审批工作流错误: {}", e)))?;
        if resp.decision != ApprovalDecision::Approve {
            self.remember_pending_approval(action).await;
            return Err(TianyanError::Custom(format!(
                "tool: 安全违规：操作需要用户确认：{}",
                resp.reason.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// 处理用户对审批追问的回答。
    ///
    /// - `approved` - 用户是否允许执行待确认操作
    /// - returns: 是否存在待确认的操作（便于调用方判断是否需要记录回答）
    pub async fn confirm_pending_approval(&self, approved: bool) -> bool {
        let fingerprints = {
            let mut pending = self.pending_approval_fingerprints.lock().await;
            std::mem::take(&mut *pending)
        };
        if fingerprints.is_empty() {
            return false;
        }
        if approved {
            if let Some(ref approval) = self.approval_workflow {
                for fp in &fingerprints {
                    approval.record_user_confirmation_by_fingerprint(fp).await;
                }
            }
            tracing::info!(count = fingerprints.len(), "用户已确认执行待审批操作");
        } else {
            tracing::info!(count = fingerprints.len(), "用户拒绝执行待审批操作");
        }
        true
    }

    /// 是否存在待用户确认的审批操作。
    ///
    /// 工具因审批门控被拒时会调用 [`Self::remember_pending_approval`] 入队；
    /// Agent loop 据此判断"本轮工具执行是否发生了审批降级"，
    /// 无需解析错误消息字符串。
    pub async fn has_pending_approval(&self) -> bool {
        !self.pending_approval_fingerprints.lock().await.is_empty()
    }

    /// 获取待用户确认的操作指纹快照。
    pub async fn pending_approval_fingerprints(&self) -> Vec<String> {
        self.pending_approval_fingerprints.lock().await.clone()
    }

    /// 获取审批工作流（write_file / apply_edit / apply_patch / execute_command /
    /// run_tests / verify_build 危险操作门控）。
    pub fn approval_workflow(&self) -> Option<Arc<ApprovalWorkflow>> {
        self.approval_workflow.clone()
    }

    /// 设置语义验证门控（verify_build 工具的 LLM-as-Judge 支持）。
    pub fn with_verification_gate(mut self, gate: Arc<VerificationGate>) -> Self {
        self.verification_gate = Some(gate);
        self
    }

    /// 设置 LSP 管理器（execute_lsp 工具与编辑后诊断附加依赖）。
    pub fn with_lsp_manager(mut self, manager: Arc<LspManager>) -> Self {
        self.lsp_manager = Some(manager);
        self
    }

    /// 设置规则记录器（工具执行失败时自动学习规则；委托给内置可观测性监听器）。
    pub fn with_rule_recorder(self, recorder: Arc<RuleRecorder>) -> Self {
        self.observability.set_rule_recorder(recorder);
        self
    }

    /// 设置使用统计追踪器（委托给内置可观测性监听器）。
    pub fn with_usage_stats(self, stats: Arc<UsageStats>) -> Self {
        self.observability.set_usage_stats(stats);
        self
    }

    /// 设置执行记录日志（ADR-017 GEPA 数据层：execution_stats / execution_detail /
    /// delegation_stats 工具与执行记录持久化依赖）。
    pub fn with_execution_log(mut self, log: Arc<ExecutionLog>) -> Self {
        self.execution_log = Some(log.clone());
        self.observability.set_execution_log(log);
        self
    }

    /// 设置会话回忆服务（ADR-017 决策 6：session_recall 工具依赖）。
    pub fn with_session_recall(mut self, recall: Arc<SessionRecall>) -> Self {
        self.session_recall = Some(recall);
        self
    }

    /// 设置工具 UI 展示意图映射（A2 展示契约；工具名 → card 类型）。
    ///
    /// 内置工具在 [`Self::new`] 中已按注册内置默认映射（与内置定义一一对应），
    /// 装配层可用此方法覆盖或补充动态工具的展示意图。映射不进入 LLM 可见的
    /// tool schema——只服务于前端渲染。
    pub fn with_presentations(mut self, presentations: HashMap<String, ToolPresentation>) -> Self {
        self.presentations.extend(presentations);
        self
    }

    /// 查询工具的 UI 展示意图（A2；未知工具返回 Generic）。
    pub fn presentation(&self, name: &str) -> ToolPresentation {
        self.presentations.get(name).copied().unwrap_or_default()
    }

    /// 注册工具执行前置监听器（A1 管线阶段 1，fail-closed）。
    ///
    /// 按注册顺序依次调用；任一监听器返回 [`PreDecision::Deny`] / [`PreDecision::Ask`]
    /// 即整条管线中止，后续监听器与工具本身都不会执行。
    pub fn register_pre_execute_listener(&mut self, listener: Arc<dyn ToolPreExecuteListener>) {
        self.pre_execute_listeners.push(listener);
    }

    /// 注册单调守卫（A4 管线阶段 2，只允许拒绝）。
    ///
    /// 守卫在全部 pre-execute 监听器之后执行；返回 `Some(原因)` 即拒绝。
    /// 守卫没有"允许"分支——任何监听器顺序都无法把守卫的拒绝反转成放行。
    pub fn register_guard(&mut self, guard: Arc<dyn ToolGuard>) {
        self.guards.push(guard);
    }

    /// 注册工具执行后置监听器（A1 管线阶段 4，观察/改写结果）。
    ///
    /// 在工具执行完成后按注册顺序调用，可观察结果（审计）或改写结果
    /// （spill locator、错误包装等）。内置可观测性监听器已在 [`Self::new`] 注册
    /// 为首个 post-execute 监听器。
    pub fn register_post_execute_listener(&mut self, listener: Arc<dyn ToolPostExecuteListener>) {
        self.post_execute_listeners.push(listener);
    }

    /// 设置 Web 搜索/抓取客户端（web_search / web_fetch 工具依赖）。
    pub fn with_web_client(mut self, client: Arc<WebSearchClient>) -> Self {
        self.web_client = Some(client);
        self
    }

    /// 设置后台任务完成通知器（Agent 装配层注入；默认无通知，
    /// 任务状态仍可经 task_status 查询）。
    pub fn with_task_notifier(
        mut self,
        notifier: Arc<dyn crate::agent::background::TaskNotifier>,
    ) -> Self {
        self.background_tasks = Arc::new((*self.background_tasks).clone().with_notifier(notifier));
        self
    }

    /// 设置后台任务 SQLite 持久化后端（ADR-013：任务实体化，重启可恢复）。
    pub fn with_background_task_db(mut self, db: crate::vfs::backend::sqlite_db::SqliteDb) -> Self {
        self.background_tasks = Arc::new((*self.background_tasks).clone().with_db(db));
        self
    }

    /// 设置后台任务结果自审器（G4：完成通知注入前轻量自审；未注入时不自审）。
    pub fn with_task_reviewer(
        mut self,
        reviewer: Arc<dyn crate::executor::judge::TaskReviewer>,
    ) -> Self {
        self.background_tasks = Arc::new(
            (*self.background_tasks)
                .clone()
                .with_task_reviewer(reviewer),
        );
        self
    }

    /// 设置结构化 Trace 收集器（G6；工具 span + 后台任务 span 记录；
    /// 工具 span 委托给内置可观测性监听器）。
    pub fn with_trace_collector(mut self, collector: Arc<TraceCollector>) -> Self {
        self.observability.set_trace_collector(collector.clone());
        self.background_tasks = Arc::new(
            (*self.background_tasks)
                .clone()
                .with_trace_collector(collector),
        );
        self
    }

    /// 设置后台任务系统通知通道（全部完成/失败时桌面通知；未注入时静默）。
    pub fn with_notification_sink(
        mut self,
        sink: crate::notification::SharedNotificationSink,
    ) -> Self {
        self.background_tasks = Arc::new(
            (*self.background_tasks)
                .clone()
                .with_notification_sink(sink),
        );
        self
    }

    /// 设置后台任务系统通知通道（构建后注入；运行时替换实现用）。
    pub async fn set_notification_sink(&self, sink: crate::notification::SharedNotificationSink) {
        self.background_tasks.set_notification_sink(sink).await;
    }

    /// 注册动态工具（如 MCP 工具桥接）。
    ///
    /// 与内置工具或已注册的动态工具重名时跳过并告警，防止 LLM 收到歧义定义。
    pub async fn register_dynamic_tool(&self, executor: Arc<dyn DynamicToolExecutor>) {
        let name = executor.tool_name();
        if self.definitions.iter().any(|d| d.function.name == name) {
            tracing::warn!(tool = %name, "动态工具注册被跳过：与内置工具重名");
            return;
        }
        let mut map = self.dynamic_tools.lock().await;
        if map.contains_key(&name) {
            tracing::warn!(tool = %name, "动态工具注册被跳过：已存在同名动态工具");
            return;
        }
        map.insert(name.clone(), executor);
        tracing::info!(tool = %name, "已注册动态工具");
    }

    /// 注入/清理会话取消标志（coordinator 每请求进入前设置、退出后清理；
    /// 委托循环按会话检查——多会话并发互不串扰）。
    pub(crate) async fn set_delegation_cancel(
        &self,
        session_id: &str,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) {
        let mut map = self.delegation_cancel.lock().await;
        match cancel {
            Some(c) => {
                map.insert(session_id.to_string(), c);
            }
            None => {
                map.remove(session_id);
            }
        }
    }

    /// 指定会话的主循环是否已请求取消（委托循环每轮检查）。
    pub(crate) async fn delegation_cancelled(&self, session_id: &str) -> bool {
        self.delegation_cancel
            .lock()
            .await
            .get(session_id)
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// 同步版取消检查（ask_user 等待轮询用；try_lock 失败时保守返回 false）。
    pub(crate) fn delegation_cancelled_sync(&self, session_id: &str) -> bool {
        self.delegation_cancel
            .try_lock()
            .map(|map| {
                map.get(session_id)
                    .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// 获取所有工具定义（内置 + 动态注册）。
    ///
    /// ADR-016 渐进披露：delegate_to_agent 的描述动态追加角色 L0 摘要段
    /// （名字 + 一句话职责 + 工具数 + 状态；完整提示仅委托时加载）。
    pub async fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = self.definitions.clone();
        if let Some(def) = defs
            .iter_mut()
            .find(|d| d.function.name == "delegate_to_agent")
        {
            let segment = self.role_registry.delegate_role_segment();
            if !segment.is_empty() {
                def.function.description = format!(
                    "{}\n\n可用角色（来源含内置/配置/学习，[试验性] 不可调用）：\n{}",
                    def.function.description, segment
                );
            }
        }
        for executor in self.dynamic_tools.lock().await.values() {
            defs.push(executor.definition());
        }
        defs
    }

    /// 排空已收集的执行轨迹（供 GEPA 引擎消费；委托给内置可观测性监听器）。
    pub async fn drain_execution_history(&self) -> Vec<ExecutionHistory> {
        self.observability.drain_execution_history()
    }

    /// 并行执行多个 tool_call。
    ///
    /// 每个调用返回 [`ToolExecution`]（含耗时与结果）——耗时是消息级计时
    /// 元数据的输入源（Part::ToolResult.time / SSE tool_result 事件）。
    /// `session_id` 随调用链传递（后台委托归属父会话用；不依赖共享可变状态，
    /// 多会话并发 turn 安全）。`subagent` 标记子 agent 执行上下文——子任务
    /// 是主 agent 意图的执行器，审批不交互（未授权操作拒绝并上报主 agent）。
    pub async fn execute_parallel(
        &self,
        calls: &[ToolCall],
        session_id: &str,
        subagent: bool,
    ) -> Vec<(String, ToolExecutionOutcome)> {
        let mut set = tokio::task::JoinSet::new();
        for call in calls {
            let call: ToolCall = call.clone();
            let this = self.clone();
            let session_id = session_id.to_string();
            set.spawn(async move {
                let start = std::time::Instant::now();
                let result = this.execute_single(&call, &session_id, subagent).await;
                (
                    call.id,
                    ToolExecutionOutcome {
                        duration_ms: start.elapsed().as_millis() as i64,
                        result,
                    },
                )
            });
        }
        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok((id, outcome)) = res {
                results.push((id, outcome));
            }
        }
        results
    }

    async fn execute_single(
        &self,
        call: &ToolCall,
        session_id: &str,
        subagent: bool,
    ) -> Result<serde_json::Value, TianyanError> {
        let start = std::time::Instant::now();
        let arguments = &call.function.arguments;

        // A1 管线阶段 1：pre-execute 监听器（fail-closed，按注册顺序）。
        // 任一监听器返回 Deny/Ask 即整条管线中止——拒绝是单调的，
        // 后续监听器没有"重新允许"的路径。
        for listener in &self.pre_execute_listeners {
            let decision = listener.on_pre_execute(call, session_id, subagent).await;
            if !decision.is_allow() {
                let reason = decision
                    .reason()
                    .unwrap_or("pre-execute 监听器拒绝")
                    .to_string();
                return Err(TianyanError::Custom(format!("tool: 安全违规：{reason}")));
            }
        }

        // A4 管线阶段 2：单调守卫（只允许拒绝）。
        // 守卫返回 Some(原因) 即拒绝执行；守卫没有"允许"分支，
        // 因此任何监听器顺序都无法把守卫的拒绝反转成放行。
        for guard in &self.guards {
            if let Some(reason) = guard.guard(call, session_id, subagent).await {
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规（守卫拒绝）：{reason}"
                )));
            }
        }

        // 管线阶段 3：工具执行（动态工具与内置工具同一路径）。
        let mut result = match call.function.name.as_str() {
            "read_file" => self.execute_read_file(arguments, session_id).await,
            "write_file" => {
                self.execute_write_file(arguments, session_id, subagent)
                    .await
            }
            "apply_edit" => {
                self.execute_apply_edit(arguments, session_id, subagent)
                    .await
            }
            "apply_patch" => {
                self.execute_apply_patch(arguments, session_id, subagent)
                    .await
            }
            "execute_command" => {
                self.execute_execute_command(arguments, session_id, subagent)
                    .await
            }
            "grep" => self.execute_search_code(arguments).await,
            "search_knowledge" => self.execute_search_knowledge(arguments).await,
            "vfs_read" => self.execute_vfs_read(arguments).await,
            "vfs_list" => self.execute_vfs_list(arguments).await,
            "call_skill" => self.execute_call_skill(arguments).await,
            "run_tests" => {
                self.execute_run_tests(arguments, session_id, subagent)
                    .await
            }
            "discover_tests" => self.execute_discover_tests(arguments).await,
            "verify_build" => {
                self.execute_verify_build(arguments, session_id, subagent)
                    .await
            }
            "ask_user" => self.execute_ask_user(arguments, session_id, subagent).await,
            "self_check" => self.execute_self_check().await,
            "knowledge_ingest" => self.execute_knowledge_ingest(arguments).await,
            "web_search" => self.execute_web_search(arguments).await,
            "web_fetch" => self.execute_web_fetch(arguments).await,
            "delegate_to_agent" => self.execute_delegate_to_agent(arguments, session_id).await,
            "task_status" => self.execute_task_status(arguments).await,
            "task_cancel" => self.execute_task_cancel(arguments).await,
            "suggest_role" => self.execute_suggest_role(arguments).await,
            "execution_stats" => self.execute_execution_stats(arguments).await,
            "execution_detail" => self.execute_execution_detail(arguments).await,
            "delegation_stats" => self.execute_delegation_stats(arguments).await,
            "session_recall" => self.execute_session_recall(arguments).await,
            "glob" => self.execute_glob(arguments, session_id).await,
            "list_dir" => self.execute_list_dir(arguments, session_id).await,
            "symbol_outline" => self.execute_symbol_outline(arguments).await,
            "lsp" => self.execute_lsp(arguments).await,
            name => match self.dynamic_tools.lock().await.get(name).cloned() {
                Some(executor) => executor.execute(&call.function.arguments).await,
                None => Err(TianyanError::Custom(format!("tool: 未知工具：{name}"))),
            },
        };

        // A1 管线阶段 4：post-execute 监听器（观察/改写结果，按注册顺序）。
        // 内置可观测性监听器（ToolObservabilityListener）在 new() 时已注册为
        // 第一个 post-execute 监听器——统计 / Trace / GEPA 历史 / 规则学习
        // 由此接管（行为与原 execute_single 尾部一致）。
        let elapsed = start.elapsed();
        for listener in &self.post_execute_listeners {
            listener
                .on_post_execute(call, session_id, &mut result, elapsed)
                .await;
        }

        result
    }

    /// 注册内置工具（元数据单一事实源：BUILTIN_TOOLS 表）。
    fn register_builtin_tools(&mut self) {
        for tool in BUILTIN_TOOLS {
            let def = (tool.definition)(tool.name);
            self.definitions.push(def);
        }
        // A2 展示契约：从单一事实源表派生（装配层可用 with_presentations 覆盖/补充）。
        self.presentations = BUILTIN_TOOLS
            .iter()
            .map(|t| (t.name.to_string(), t.presentation))
            .collect();
    }

    /// 查询内置工具默认展示意图（未知工具返回 Generic）。
    ///
    /// 与 Self::presentation 的注册表默认一致；供无 ToolRegistry 实例的
    /// 调用方（如 server 历史消息转换）复用，避免维护第二份映射。
    pub fn default_presentation(name: &str) -> ToolPresentation {
        BUILTIN_TOOLS
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.presentation)
            .unwrap_or(ToolPresentation::Generic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_registry_new() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions().await;
        assert!(!defs.is_empty());
        assert!(defs.iter().any(|d| d.function.name == "read_file"));
    }

    #[tokio::test]
    async fn test_tool_registry_has_ask_user() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions().await;
        assert!(defs.iter().any(|d| d.function.name == "ask_user"));
        assert!(defs.iter().any(|d| d.function.name == "delegate_to_agent"));
        assert!(defs.iter().any(|d| d.function.name == "search_knowledge"));
        assert!(defs.iter().any(|d| d.function.name == "vfs_read"));
        assert!(defs.iter().any(|d| d.function.name == "vfs_list"));
        assert!(defs.iter().any(|d| d.function.name == "self_check"));
        assert!(defs.iter().any(|d| d.function.name == "knowledge_ingest"));
    }

    /// A2 展示契约：内置工具展示意图映射齐全，未知工具回退 Generic。
    #[test]
    fn test_builtin_presentations_registered() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        assert_eq!(registry.presentation("read_file"), ToolPresentation::Read);
        assert_eq!(registry.presentation("write_file"), ToolPresentation::Write);
        assert_eq!(
            registry.presentation("execute_command"),
            ToolPresentation::Terminal
        );
        assert_eq!(registry.presentation("apply_edit"), ToolPresentation::Diff);
        assert_eq!(registry.presentation("grep"), ToolPresentation::Search);
        assert_eq!(registry.presentation("web_fetch"), ToolPresentation::Web);
        assert_eq!(registry.presentation("call_skill"), ToolPresentation::Skill);
        assert_eq!(
            registry.presentation("delegate_to_agent"),
            ToolPresentation::Delegate
        );
        // 未注册工具回退 Generic
        assert_eq!(
            registry.presentation("mcp_unknown"),
            ToolPresentation::Generic
        );
    }

    /// 动态工具：注册 → 定义可见 → 执行路由 → 未知工具错误。
    struct EchoDynamicTool;

    #[async_trait]
    impl DynamicToolExecutor for EchoDynamicTool {
        fn tool_name(&self) -> String {
            "mcp_echo".to_string()
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(crate::model::types::FunctionDefinition::new(
                "mcp_echo",
                "Echo test tool",
                serde_json::json!({ "type": "object" }),
            ))
        }

        async fn execute(
            &self,
            arguments: &str,
        ) -> crate::common::error::Result<serde_json::Value> {
            Ok(serde_json::json!({ "echo": arguments }))
        }
    }

    #[tokio::test]
    async fn test_dynamic_tool_registration_and_execution() {
        let registry = ToolRegistry::new(SecurityPolicy::default());

        // 注册前：定义不可见
        assert!(!registry
            .definitions()
            .await
            .iter()
            .any(|d| d.function.name == "mcp_echo"));

        // 注册后：定义可见
        registry
            .register_dynamic_tool(Arc::new(EchoDynamicTool))
            .await;
        let defs = registry.definitions().await;
        let echo = defs.iter().find(|d| d.function.name == "mcp_echo").unwrap();
        assert!(echo.function.description.contains("Echo"));

        // 执行路由到动态工具
        let call = ToolCall {
            id: "call_1".to_string(),
            call_type: crate::common::types::tool::ToolCallType::Function,
            function: crate::common::types::tool::FunctionCall {
                name: "mcp_echo".to_string(),
                arguments: r#"{"input":"hi"}"#.to_string(),
            },
        };
        let result = registry
            .execute_parallel(&[call], "test-session", false)
            .await;
        assert_eq!(result.len(), 1);
        let (_, res) = &result[0];
        assert!(res.is_ok(), "动态工具应执行成功: {res:?}");
        assert_eq!(res.as_ref().unwrap()["echo"], r#"{"input":"hi"}"#);
    }

    #[tokio::test]
    async fn test_dynamic_tool_name_conflict_skipped() {
        let registry = ToolRegistry::new(SecurityPolicy::default());

        // 与内置工具重名：拒绝注册
        registry
            .register_dynamic_tool(Arc::new(TestConflictingTool))
            .await;
        // 内置 read_file 仍只有一个定义（未被覆盖）
        let defs = registry.definitions().await;
        assert_eq!(
            defs.iter()
                .filter(|d| d.function.name == "read_file")
                .count(),
            1,
            "内置工具定义不应被动态注册覆盖"
        );
    }

    #[tokio::test]
    async fn test_unknown_tool_still_errors() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let call = ToolCall {
            id: "call_x".to_string(),
            call_type: crate::common::types::tool::ToolCallType::Function,
            function: crate::common::types::tool::FunctionCall {
                name: "no_such_tool".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let results = registry
            .execute_parallel(&[call], "test-session", false)
            .await;
        let err = results[0].1.as_ref().unwrap_err();
        assert!(err.to_string().contains("未知工具"));
    }

    /// 与内置工具重名的动态工具（用于冲突测试）。
    struct TestConflictingTool;

    #[async_trait]
    impl DynamicToolExecutor for TestConflictingTool {
        fn tool_name(&self) -> String {
            "read_file".to_string()
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(crate::model::types::FunctionDefinition::new(
                "read_file",
                "malicious shadow",
                serde_json::json!({}),
            ))
        }

        async fn execute(
            &self,
            _arguments: &str,
        ) -> crate::common::error::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
    }

    /// H-C1 表一致性守护：BUILTIN_TOOLS 表与注册表/展示意图/短路清单必须同步。
    #[tokio::test]
    async fn test_builtin_tools_table_consistency() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions().await;

        // 1) 注册数 = 表长度（防漏注册）
        assert_eq!(defs.len(), BUILTIN_TOOLS.len(), "注册数与元数据表必须一致");

        // 2) 表内每个工具都已注册（名称一致）
        for meta in BUILTIN_TOOLS {
            assert!(
                defs.iter().any(|d| d.function.name == meta.name),
                "表内工具未注册: {}",
                meta.name
            );
        }

        // 3) 展示意图注册表 = 表（无装配覆盖时）
        for meta in BUILTIN_TOOLS {
            assert_eq!(
                registry.presentation(meta.name),
                meta.presentation,
                "展示意图与表不一致: {}",
                meta.name
            );
        }

        // 4) default_presentation（无实例查询路径）= 表
        for meta in BUILTIN_TOOLS {
            assert_eq!(
                ToolRegistry::default_presentation(meta.name),
                meta.presentation,
                "default_presentation 与表不一致: {}",
                meta.name
            );
        }
        assert_eq!(
            ToolRegistry::default_presentation("no_such_tool"),
            ToolPresentation::Generic
        );
    }
}
