use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::agent::tool_params::{
    ApplyEditParams, ApplyPatchParams, AskUserParams, CallSkillParams, DelegateToAgentParams,
    DiscoverTestsParams, ExecuteCommandParams, GlobParams, KnowledgeIngestParams, ListDirParams,
    LspParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    SelfCheckParams, SymbolOutlineParams, TaskCancelParams, TaskStatusParams, VerifyBuildParams,
    VfsListParams, VfsReadParams, WebFetchParams, WebSearchParams, WriteFileParams,
};
use crate::agent::RoleRegistry;
use crate::common::error::TianyanError;
use crate::executor::approval::{ApprovalDecision, ApprovalWorkflow};
use crate::executor::web::WebSearchClient;
use crate::executor::Action;
use crate::executor::SecurityPolicy;
use crate::executor::VerificationGate;
use crate::knowledge::KnowledgeIngestor;
use crate::lsp::diagnostics::LspManager;
use crate::model::types::{FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ChatService;
use crate::observability::trace::TraceCollector;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::skills::learning::ExecutionHistory;
use crate::skills::SkillExecutor;
use crate::vfs::VirtualFileSystem;

mod agent_ops;
mod code_ops;
/// 工具执行器实现（按工具域拆分，14 个 `execute_*` 方法）。
mod file_ops;
/// 文件系统浏览工具执行器（glob / list_dir）。
mod fs_ops;
mod knowledge_ops;
/// LSP 工具执行器（execute_lsp）。
mod lsp_ops;
/// 符号大纲工具执行器（symbol_outline）。
mod symbol_ops;
/// 测试发现工具执行器（discover_tests）。
mod test_ops;

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
    /// 规则记录器（工具执行失败时自动学习规则，供 GEPA 进化引擎消费）。
    pub(crate) rule_recorder: Option<Arc<RuleRecorder>>,
    /// 使用统计追踪器（技能调用频率、文档访问热度）。
    pub(crate) usage_stats: Option<Arc<UsageStats>>,
    /// Web 搜索/抓取客户端（web_search / web_fetch 工具依赖）。
    pub(crate) web_client: Option<Arc<WebSearchClient>>,
    /// 后台任务管理器（delegate_to_agent(background) / task_status / task_cancel）。
    pub(crate) background_tasks: Arc<crate::agent::background::BackgroundTaskManager>,
    definitions: Vec<ToolDefinition>,
    /// 外部注册的动态工具（如 MCP 工具桥接），按工具名索引。
    dynamic_tools: Arc<Mutex<HashMap<String, Arc<dyn DynamicToolExecutor>>>>,
    /// 执行轨迹（GEPA 引擎消费）。
    pub(crate) execution_history: Arc<Mutex<Vec<ExecutionHistory>>>,
    /// 当前委托链深度（delegate_to_agent 嵌套保护；主循环为 0）。
    pub(crate) delegation_depth: Arc<AtomicUsize>,
    /// 子 Agent 角色注册表（delegate_to_agent role 参数解析；默认内置角色）。
    pub(crate) role_registry: Arc<RoleRegistry>,
    /// 结构化 Trace 收集器（G6；None 时不记录 span——行为零变化）。
    pub(crate) trace_collector: Option<Arc<TraceCollector>>,
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
            rule_recorder: None,
            usage_stats: None,
            web_client: None,
            background_tasks: Arc::new(crate::agent::background::BackgroundTaskManager::new()),
            definitions: Vec::new(),
            dynamic_tools: Arc::new(Mutex::new(HashMap::new())),
            execution_history: Arc::new(Mutex::new(Vec::new())),
            delegation_depth: Arc::new(AtomicUsize::new(0)),
            role_registry: Arc::new(RoleRegistry::builtin()),
            trace_collector: None,
        };
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

    /// 设置规则记录器（工具执行失败时自动学习规则）。
    pub fn with_rule_recorder(mut self, recorder: Arc<RuleRecorder>) -> Self {
        self.rule_recorder = Some(recorder);
        self
    }

    /// 设置使用统计追踪器。
    pub fn with_usage_stats(mut self, stats: Arc<UsageStats>) -> Self {
        self.usage_stats = Some(stats);
        self
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

    /// 设置结构化 Trace 收集器（G6；工具 span + 后台任务 span 记录）。
    pub fn with_trace_collector(mut self, collector: Arc<TraceCollector>) -> Self {
        self.trace_collector = Some(collector.clone());
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

    /// 获取所有工具定义（内置 + 动态注册）。
    pub async fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = self.definitions.clone();
        for executor in self.dynamic_tools.lock().await.values() {
            defs.push(executor.definition());
        }
        defs
    }

    /// 工具短路选择阈值（G1）：LLM 可见工具数超过该值时启用按相关性过滤。
    ///
    /// 业界证据（IBM General Agent Evaluation）：工具数 >128 且无短路时
    /// 模型工具调用质量崩塌；40-60 个工具内模型通常仍可应付。阈值取 40：
    /// 内置 25 + 少量 MCP 时行为零变化，MCP/技能生态膨胀后自动启用。
    pub(crate) const TOOL_SHORTLIST_THRESHOLD: usize = 40;

    /// 恒存核心工具集：基础操作与通用检索，任何场景都保留。
    const CORE_TOOLS: &[&str] = &[
        "read_file",
        "write_file",
        "apply_edit",
        "apply_patch",
        "execute_command",
        "search_code",
        "glob",
        "list_dir",
        "call_skill",
        "ask_user",
        "self_check",
        "delegate_to_agent",
        "task_status",
        "task_cancel",
        "search_knowledge",
        "vfs_read",
        "vfs_list",
        "web_search",
        "web_fetch",
    ];

    /// 条件工具关键词表（query 命中任一关键词即保留；G1）。
    ///
    /// 仅覆盖低频/专用工具；关键词为小写（英文）或原文（中文）。
    const CONDITIONAL_TOOL_KEYWORDS: &[(&str, &[&str])] = &[
        (
            "knowledge_ingest",
            &["知识", "导入", "ingest", "文档库", "knowledge"],
        ),
        (
            "run_tests",
            &["测试", "用例", "test", "pytest", "cargo test", "run_tests"],
        ),
        (
            "discover_tests",
            &["测试", "用例", "test", "发现测试", "discover"],
        ),
        (
            "verify_build",
            &["构建", "编译", "build", "报错", "编译错误", "verify"],
        ),
        (
            "symbol_outline",
            &["符号", "大纲", "symbol", "outline", "结构"],
        ),
        (
            "lsp",
            &["lsp", "诊断", "跳转", "定义", "引用", "diagnostic", "符号"],
        ),
    ];

    /// 按相关性过滤的 LLM 可见工具定义（G1 工具短路选择）。
    ///
    /// - 工具总数 ≤ [`TOOL_SHORTLIST_THRESHOLD`] 或 `query` 为 None 时：
    ///   返回全量（行为零变化，保守——无查询信号不冒险）。
    /// - 超阈值时：核心工具恒存；条件工具按关键词表匹配；
    ///   动态（MCP）工具按名称/描述与 query 分词重叠匹配。
    /// - **执行层不受影响**：`execute_single` 保留全部工具——被过滤工具
    ///   若被模型调用（如模型从历史中学到该工具），仍正常执行；
    ///   过滤仅改变 LLM 可见的 schema 数量，不产生"未加载"错误路径。
    pub async fn definitions_shortlisted(&self, query: Option<&str>) -> Vec<ToolDefinition> {
        let defs = self.definitions().await;
        let Some(query) = query else {
            return defs;
        };
        if defs.len() <= Self::TOOL_SHORTLIST_THRESHOLD {
            return defs;
        }
        let query_lower = query.to_lowercase();

        defs.into_iter()
            .filter(|def| {
                let name = def.function.name.as_str();
                // 核心工具恒存
                if Self::CORE_TOOLS.contains(&name) {
                    return true;
                }
                // 条件工具：关键词任一命中 query
                if let Some((_, keywords)) = Self::CONDITIONAL_TOOL_KEYWORDS
                    .iter()
                    .find(|(tool, _)| *tool == name)
                {
                    return keywords
                        .iter()
                        .any(|k| query_lower.contains(&k.to_lowercase()));
                }
                // 动态（MCP）工具：query 分词与名称/描述重叠匹配
                Self::dynamic_tool_matches(&query_lower, def)
            })
            .collect()
    }

    /// 动态工具相关性：query 的英文分词（≥3 字符）与中文片段（≥2 字符）
    /// 任一出现在工具名或描述中即保留。
    fn dynamic_tool_matches(query_lower: &str, def: &ToolDefinition) -> bool {
        let haystack = format!("{} {}", def.function.name, def.function.description).to_lowercase();
        // 英文/数字分词：按非字母数字切分
        let tokens: Vec<&str> = query_lower
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 3)
            .collect();
        // 中文片段：提取连续 CJK 字符块（长度 ≥2）作为匹配键
        let mut cjk = String::new();
        let mut cjk_chunks: Vec<String> = Vec::new();
        for ch in query_lower.chars() {
            if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                cjk.push(ch);
            } else {
                if cjk.chars().count() >= 2 {
                    cjk_chunks.push(cjk.clone());
                }
                cjk.clear();
            }
        }
        if cjk.chars().count() >= 2 {
            cjk_chunks.push(cjk);
        }

        let overlap = tokens.iter().any(|t| haystack.contains(t))
            || cjk_chunks.iter().any(|c| haystack.contains(c));
        if !overlap {
            tracing::debug!(tool = %def.function.name, "工具被短路过滤（与当前 query 不相关）");
        }
        overlap
    }

    /// 排空已收集的执行轨迹（供 GEPA 引擎消费）。
    pub async fn drain_execution_history(&self) -> Vec<ExecutionHistory> {
        let mut guard = self.execution_history.lock().await;
        std::mem::take(&mut *guard)
    }

    /// 并行执行多个 tool_call。
    ///
    /// `session_id` 随调用链传递（后台委托归属父会话用；不依赖共享可变状态，
    /// 多会话并发 turn 安全）。`subagent` 标记子 agent 执行上下文——子任务
    /// 是主 agent 意图的执行器，审批不交互（未授权操作拒绝并上报主 agent）。
    pub async fn execute_parallel(
        &self,
        calls: &[ToolCall],
        session_id: &str,
        subagent: bool,
    ) -> Vec<(String, Result<serde_json::Value, TianyanError>)> {
        let mut set = tokio::task::JoinSet::new();
        for call in calls {
            let call: ToolCall = call.clone();
            let this = self.clone();
            let session_id = session_id.to_string();
            set.spawn(async move {
                let result = this.execute_single(&call, &session_id, subagent).await;
                (call.id, result)
            });
        }
        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok((id, result)) = res {
                results.push((id, result));
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
        let result = match call.function.name.as_str() {
            "read_file" => self.execute_read_file(arguments).await,
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
            "search_code" => self.execute_search_code(arguments).await,
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
            "ask_user" => self.execute_ask_user(arguments).await,
            "self_check" => self.execute_self_check().await,
            "knowledge_ingest" => self.execute_knowledge_ingest(arguments).await,
            "web_search" => self.execute_web_search(arguments).await,
            "web_fetch" => self.execute_web_fetch(arguments).await,
            "delegate_to_agent" => self.execute_delegate_to_agent(arguments, session_id).await,
            "task_status" => self.execute_task_status(arguments).await,
            "task_cancel" => self.execute_task_cancel(arguments).await,
            "glob" => self.execute_glob(arguments).await,
            "list_dir" => self.execute_list_dir(arguments).await,
            "symbol_outline" => self.execute_symbol_outline(arguments).await,
            "lsp" => self.execute_lsp(arguments).await,
            name => match self.dynamic_tools.lock().await.get(name).cloned() {
                Some(executor) => executor.execute(&call.function.arguments).await,
                None => Err(TianyanError::Custom(format!("tool: 未知工具：{name}"))),
            },
        };

        // Record usage stats for tool call
        if let Some(ref stats) = self.usage_stats {
            let elapsed_us = start.elapsed().as_micros() as u64;
            stats.record_skill_call(&call.function.name, result.is_ok(), elapsed_us);
        }

        // G6 结构化 Trace：工具 span（归属当前轮；参数摘要截断控制体积）
        if let Some(ref trace) = self.trace_collector {
            trace.record_tool(
                session_id,
                &call.function.name,
                &truncate_trace_params(arguments),
                start.elapsed().as_millis() as i64,
                result.is_ok(),
                result.as_ref().err().map(|e| e.to_string()),
            );
        }

        // Record execution history for GEPA
        let elapsed = start.elapsed().as_millis() as u64;
        let success = result.is_ok();
        let mut history = self.execution_history.lock().await;
        history.push(ExecutionHistory {
            task_description: format!("{}: {}", call.function.name, arguments),
            steps: vec![],
            result: match &result {
                Ok(v) => format!("{}", v),
                Err(e) => e.to_string(),
            },
            success,
            execution_time_ms: elapsed,
            skills_used: if call.function.name == "call_skill" {
                serde_json::from_str::<CallSkillParams>(arguments)
                    .map(|p| vec![p.skill_id])
                    .unwrap_or_default()
            } else {
                vec![]
            },
        });

        // Record tool failures as learned rules for GEPA evolution.
        // Only Logic/System failures are persisted; Transient errors (e.g.,
        // network timeouts) are skipped by RuleRecorder::record_with_kind.
        if !success {
            if let Some(ref recorder) = self.rule_recorder {
                let tool_name = &call.function.name;
                let err_msg = result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                let abstract_text = format!("{} 工具执行失败", tool_name);
                let detail_text = format!(
                    "工具: {}\n参数: {}\n错误: {}\n",
                    tool_name, arguments, err_msg
                );
                // Use a fixed session_id — recordings are later aggregated
                // by RuleSuggester across all sessions.
                if let Err(e) = recorder
                    .record_with_kind(
                        &abstract_text,
                        &detail_text,
                        "tool-execution",
                        crate::scheduler::tasks::rule_recorder::FailureKind::Logic,
                    )
                    .await
                {
                    tracing::debug!(
                        tool = %tool_name,
                        error = %e,
                        "RuleRecorder 记录失败（将被 RuleSuggester 后续聚合）"
                    );
                }
            }
        }

        result
    }

    fn register_builtin_tools(&mut self) {
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ReadFileParams,
            >(
                "read_file",
                "Read the full text content of a file from the given path.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                WriteFileParams,
            >(
                "write_file",
                "Write content to a file at the given path.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ApplyEditParams,
            >(
                "apply_edit",
                "Apply precise edits to a file using line-number + content-hash anchors. Each edit targets a line range verified by an anchor hash; edits are applied bottom-up after all anchors are validated (atomic batch).",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ApplyPatchParams,
            >(
                "apply_patch",
                "Apply a unified diff patch (*** Update File format) to one or more files. Supports fuzzy matching of context lines and multiple files in one patch.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ExecuteCommandParams,
            >(
                "execute_command",
                "Execute a shell command with optional working directory and timeout.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                SearchCodeParams,
            >(
                "search_code",
                "Search for code patterns using ripgrep.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                SearchKnowledgeParams,
            >(
                "search_knowledge",
                "Search the knowledge base semantically (all namespaces) using vector RRF fusion. Returns abstract + overview + URI for each result. Use vfs_read to load full detail when needed.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                VfsReadParams,
            >(
                "vfs_read",
                "Read full content (abstract, overview, and detail) of a VFS entry by its tianyan:// URI. Use after search_knowledge to load detailed content of relevant entries.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                VfsListParams,
            >(
                "vfs_list",
                "List entries in a VFS directory by its tianyan:// URI. Useful for browsing the knowledge base structure.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                CallSkillParams,
            >(
                "call_skill",
                "Call a registered skill by ID with parameters.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                RunTestsParams,
            >(
                "run_tests",
                "Run a test command (e.g. cargo test) and return results.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                DiscoverTestsParams,
            >(
                "discover_tests",
                "Discover tests in the project (cargo test -- --list / pytest --collect-only -q / vitest --list) and return structured test list with suite, name, file and line. Does not execute tests.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                VerifyBuildParams,
            >(
                "verify_build",
                "Run a build verification command (e.g. cargo check) and return results.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                AskUserParams,
            >(
                "ask_user",
                "Ask the user a question when more information is needed to proceed.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                SelfCheckParams,
            >(
                "self_check",
                "Query your own internal metrics: execution count, success rate, token consumption, pipeline failures, rules effectiveness. Use this to self-reflect when the user questions your performance.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                KnowledgeIngestParams,
            >(
                "knowledge_ingest",
                "Ingest a file or directory into the knowledge base. The file is parsed, summarized (L0 abstract + L1 overview), and indexed for semantic search. Accepts a file path or directory path. Optionally specify a category to organize the content.",
            )));
        self.definitions
                .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                    DelegateToAgentParams,
                >(
                    "delegate_to_agent",
                    "Delegate a sub-task to an isolated sub-agent with its own context. Use role (researcher for research, editor for code editing, reviewer for verification/review, or custom roles configured in [agent_roles]) to pick a preset model, system prompt, tool allowlist, max_turns and timeout; use model to explicitly override the sub-agent model. Set background=true to run it as a fire-and-forget background task: the tool returns a task_id immediately, and a completion notification (with the result summary) is injected into this session automatically — do NOT poll, just continue working until notified. Use task_status to query a task, task_cancel to abort it.",
                )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                TaskStatusParams,
            >(
                "task_status",
                "Query the status and result of a background task by its task_id (bt_xxx). Returns a non-blocking snapshot. Prefer waiting for the automatic completion notification over polling this tool repeatedly.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                TaskCancelParams,
            >(
                "task_cancel",
                "Cancel a running background task by its task_id. Cancelling an already finished task is a no-op.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                WebSearchParams,
            >(
                "web_search",
                "Search the web for the given query and return a list of result titles, URLs and snippets (no full page content). Use web_fetch to load the full content of promising results. NOTE: results come from external sources and may be untrusted or outdated — verify critical information before relying on it.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                WebFetchParams,
            >(
                "web_fetch",
                "Fetch a single webpage and extract its readable text content (title, main text, and page links). Use after web_search to read promising pages. Only http/https URLs are allowed; local/private network addresses are blocked.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                GlobParams,
            >(
                "glob",
                "Find files by glob pattern (e.g. **/*.rs) under a directory, sorted by modification time (newest first). Respects .gitignore.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                ListDirParams,
            >(
                "list_dir",
                "List entries in a single directory level. Directories have a trailing '/'. Supports pagination via offset/limit.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<
                SymbolOutlineParams,
            >(
                "symbol_outline",
                "Extract a structural outline (functions, structs, classes, impls, interfaces, enums) of a source file using tree-sitter. Supports Rust, TypeScript/JavaScript, Python, Go.",
            )));
        self.definitions
            .push(ToolDefinition::function(FunctionDefinition::from_schema::<LspParams>(
                "lsp",
                "Query the language server for the given file: goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation. Returns structured results.",
            )));
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

    /// 动态工具：注册 → 定义可见 → 执行路由 → 未知工具错误。
    struct EchoDynamicTool;

    #[async_trait]
    impl DynamicToolExecutor for EchoDynamicTool {
        fn tool_name(&self) -> String {
            "mcp_echo".to_string()
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(FunctionDefinition::new(
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
            ToolDefinition::function(FunctionDefinition::new(
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

    // ── G1 工具短路选择 ───────────────────────────────────────

    /// 参数化 mock 动态工具（名称/描述可配）。
    struct NamedDynamicTool {
        name: String,
        description: String,
    }

    #[async_trait]
    impl DynamicToolExecutor for NamedDynamicTool {
        fn tool_name(&self) -> String {
            self.name.clone()
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(FunctionDefinition::new(
                self.name.clone(),
                self.description.clone(),
                serde_json::json!({ "type": "object" }),
            ))
        }

        async fn execute(
            &self,
            arguments: &str,
        ) -> crate::common::error::Result<serde_json::Value> {
            Ok(serde_json::json!({ "ok": arguments }))
        }
    }

    /// 注册 N 个 mock 动态工具（使总数超阈值）。
    async fn register_mock_tools(registry: &ToolRegistry, count: usize) {
        for i in 0..count {
            registry
                .register_dynamic_tool(Arc::new(NamedDynamicTool {
                    name: format!("mcp_tool_{i:02}"),
                    description: "Generic mock MCP tool with no specific domain".to_string(),
                }))
                .await;
        }
    }

    #[tokio::test]
    async fn test_shortlist_below_threshold_returns_all() {
        // 内置 25 工具 < 40 阈值：即使传 query 也返回全量（行为零变化）
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry
            .definitions_shortlisted(Some("帮我测试一下构建"))
            .await;
        assert_eq!(defs.len(), registry.definitions().await.len());
        assert!(defs.iter().any(|d| d.function.name == "lsp"));
    }

    #[tokio::test]
    async fn test_shortlist_none_query_returns_all() {
        // 无查询信号：保守全量
        let registry = ToolRegistry::new(SecurityPolicy::default());
        register_mock_tools(&registry, 20).await; // 25 + 20 = 45 > 40
        let defs = registry.definitions_shortlisted(None).await;
        assert_eq!(defs.len(), 45);
    }

    #[tokio::test]
    async fn test_shortlist_core_tools_always_kept() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        register_mock_tools(&registry, 20).await;

        let defs = registry.definitions_shortlisted(Some("随便聊聊天气")).await;
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        // 核心工具恒存
        for core in [
            "read_file",
            "write_file",
            "apply_edit",
            "apply_patch",
            "execute_command",
            "search_code",
            "glob",
            "list_dir",
            "call_skill",
            "ask_user",
            "self_check",
            "delegate_to_agent",
            "task_status",
            "task_cancel",
            "search_knowledge",
            "vfs_read",
            "vfs_list",
            "web_search",
            "web_fetch",
        ] {
            assert!(names.contains(&core), "核心工具 {core} 应恒存");
        }
        // 不相关条件工具被过滤（query 无编程关键词）
        assert!(!names.contains(&"lsp"));
        assert!(!names.contains(&"run_tests"));
        assert!(!names.contains(&"verify_build"));
    }

    #[tokio::test]
    async fn test_shortlist_conditional_keyword_match() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        register_mock_tools(&registry, 20).await;

        // query 含"测试" → run_tests / discover_tests 保留
        let defs = registry
            .definitions_shortlisted(Some("帮我跑一下测试看看哪些失败"))
            .await;
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"run_tests"));
        assert!(names.contains(&"discover_tests"));

        // query 含"构建" → verify_build 保留
        let defs = registry
            .definitions_shortlisted(Some("构建报错了，帮我看看"))
            .await;
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"verify_build"));
    }

    #[tokio::test]
    async fn test_shortlist_dynamic_tool_match_by_name() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        // 注册一个领域工具 + 一堆无关工具
        registry
            .register_dynamic_tool(Arc::new(NamedDynamicTool {
                name: "mcp_git_status".to_string(),
                description: "Inspect git repository status and diff".to_string(),
            }))
            .await;
        register_mock_tools(&registry, 20).await;

        let defs = registry
            .definitions_shortlisted(Some("git 状态怎么样"))
            .await;
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        // 中文片段 "git" 命中工具名 → 保留
        assert!(names.contains(&"mcp_git_status"));
        // 无关 mock 工具被过滤
        assert!(!names.contains(&"mcp_tool_01"));
        assert!(!names.contains(&"mcp_tool_15"));
    }

    #[tokio::test]
    async fn test_shortlist_dynamic_tool_match_by_description() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        registry
            .register_dynamic_tool(Arc::new(NamedDynamicTool {
                name: "mcp_browser_click".to_string(),
                description: "Click an element in the browser via playwright".to_string(),
            }))
            .await;
        register_mock_tools(&registry, 20).await;

        // 英文分词 "browser"/"playwright" 命中描述 → 保留
        let defs = registry
            .definitions_shortlisted(Some("use playwright to click the button in browser"))
            .await;
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"mcp_browser_click"));
    }
}
