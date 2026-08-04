use std::sync::Arc;

use tokio::sync::Mutex;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    KnowledgeIngestParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    SelfCheckParams, VerifyBuildParams, VfsListParams, VfsReadParams, WriteFileParams,
};
use crate::common::error::TianyanError;
use crate::executor::approval::ApprovalWorkflow;
use crate::executor::Action;
use crate::executor::SecurityPolicy;
use crate::executor::VerificationGate;
use crate::knowledge::KnowledgeIngestor;
use crate::model::types::{FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ChatService;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::skills::learning::ExecutionHistory;
use crate::skills::SkillExecutor;
use crate::vfs::VirtualFileSystem;

/// 工具执行器（14 个 `execute_*` 方法，拆分自注册表主体）。
mod executors;

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
    /// 审批工作流（write_file / execute_command 危险操作门控）。
    pub(crate) approval_workflow: Option<Arc<ApprovalWorkflow>>,
    /// 待用户确认的操作指纹（审批拒绝 → 询问用户 → 用户回答后确认/放弃）。
    pub(crate) pending_approval_fingerprints: Arc<Mutex<Vec<String>>>,
    /// 语义验证门控（verify_build 工具的 LLM-as-Judge 支持）。
    pub(crate) verification_gate: Option<Arc<VerificationGate>>,
    /// 规则记录器（工具执行失败时自动学习规则，供 GEPA 进化引擎消费）。
    pub(crate) rule_recorder: Option<Arc<RuleRecorder>>,
    /// 使用统计追踪器（技能调用频率、文档访问热度）。
    pub(crate) usage_stats: Option<Arc<UsageStats>>,
    definitions: Vec<ToolDefinition>,
    /// 执行轨迹（GEPA 引擎消费）。
    pub(crate) execution_history: Arc<Mutex<Vec<ExecutionHistory>>>,
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
            rule_recorder: None,
            usage_stats: None,
            definitions: Vec::new(),
            execution_history: Arc::new(Mutex::new(Vec::new())),
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

    /// 设置可观测性指标（self_check 工具依赖）。
    pub fn with_metrics(mut self, metrics: Arc<AgentMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 设置审批工作流（write_file / execute_command 危险操作门控）。
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

    /// 设置语义验证门控（verify_build 工具的 LLM-as-Judge 支持）。
    pub fn with_verification_gate(mut self, gate: Arc<VerificationGate>) -> Self {
        self.verification_gate = Some(gate);
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

    /// 获取所有工具定义。
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    /// 排空已收集的执行轨迹（供 GEPA 引擎消费）。
    pub async fn drain_execution_history(&self) -> Vec<ExecutionHistory> {
        let mut guard = self.execution_history.lock().await;
        std::mem::take(&mut *guard)
    }

    /// 并行执行多个 tool_call。
    pub async fn execute_parallel(
        &self,
        calls: &[ToolCall],
    ) -> Vec<(String, Result<serde_json::Value, TianyanError>)> {
        let mut set = tokio::task::JoinSet::new();
        for call in calls {
            let call: ToolCall = call.clone();
            let this = self.clone();
            set.spawn(async move {
                let result = this.execute_single(&call).await;
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

    async fn execute_single(&self, call: &ToolCall) -> Result<serde_json::Value, TianyanError> {
        let start = std::time::Instant::now();
        let arguments = &call.function.arguments;
        let result = match call.function.name.as_str() {
            "read_file" => self.execute_read_file(arguments).await,
            "write_file" => self.execute_write_file(arguments).await,
            "execute_command" => self.execute_execute_command(arguments).await,
            "search_code" => self.execute_search_code(arguments).await,
            "search_knowledge" => self.execute_search_knowledge(arguments).await,
            "vfs_read" => self.execute_vfs_read(arguments).await,
            "vfs_list" => self.execute_vfs_list(arguments).await,
            "call_skill" => self.execute_call_skill(arguments).await,
            "run_tests" => self.execute_run_tests(arguments).await,
            "verify_build" => self.execute_verify_build(arguments).await,
            "ask_user" => self.execute_ask_user(arguments).await,
            "self_check" => self.execute_self_check().await,
            "knowledge_ingest" => self.execute_knowledge_ingest(arguments).await,
            "delegate_to_agent" => self.execute_delegate_to_agent(arguments).await,
            _ => Err(TianyanError::Custom(format!(
                "tool: 未知工具：{}",
                call.function.name.clone()
            ))),
        };

        // Record usage stats for tool call
        if let Some(ref stats) = self.usage_stats {
            let elapsed_us = start.elapsed().as_micros() as u64;
            stats.record_skill_call(&call.function.name, result.is_ok(), elapsed_us);
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
                "Delegate a sub-task to an isolated sub-agent with its own context.",
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        assert!(!registry.definitions().is_empty());
        assert!(registry
            .definitions()
            .iter()
            .any(|d| d.function.name == "read_file"));
    }

    #[test]
    fn test_tool_registry_has_ask_user() {
        let registry = ToolRegistry::new(SecurityPolicy::default());
        let defs = registry.definitions();
        assert!(defs.iter().any(|d| d.function.name == "ask_user"));
        assert!(defs.iter().any(|d| d.function.name == "delegate_to_agent"));
        assert!(defs.iter().any(|d| d.function.name == "search_knowledge"));
        assert!(defs.iter().any(|d| d.function.name == "vfs_read"));
        assert!(defs.iter().any(|d| d.function.name == "vfs_list"));
        assert!(defs.iter().any(|d| d.function.name == "self_check"));
        assert!(defs.iter().any(|d| d.function.name == "knowledge_ingest"));
    }
}
