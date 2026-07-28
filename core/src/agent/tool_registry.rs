use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    KnowledgeIngestParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    SelfCheckParams, VerifyBuildParams, VfsListParams, VfsReadParams, WriteFileParams,
};
use crate::common::types::{
    ContentLevel, ContentSource, ContextNamespace, Message, SearchResult, TianyanUri,
};
use crate::executor::approval::{ApprovalDecision, ApprovalWorkflow};
use crate::executor::SecurityPolicy;
use crate::executor::Action;
use crate::executor::VerificationGate;
use crate::scheduler::tasks::RuleRecorder;
use crate::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use crate::model::types::{ChatCompletionRequest, FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ChatService;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::skills::learning::ExecutionHistory;
use crate::skills::{SkillExecutionRequest, SkillExecutor};
use crate::vfs::VirtualFileSystem;

/// 工具执行错误。
#[derive(thiserror::Error, Debug, Clone)]
pub enum ToolExecutionError {
    /// 未知工具。
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    /// 参数无效。
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    /// 执行失败。
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    /// 安全违规。
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    /// 需要追问。
    #[error("Ask user: {0}")]
    AskUser(String),
}

/// 工具注册表，维护工具定义并并行执行 tool_calls。
#[derive(Clone)]
pub struct ToolRegistry {
    security_policy: SecurityPolicy,
    skill_executor: Option<Arc<SkillExecutor>>,
    knowledge_ingestor: Option<Arc<KnowledgeIngestor>>,
    vfs: Option<Arc<dyn VirtualFileSystem>>,
    model_service: Option<Arc<dyn ChatService>>,
    /// 委托子 Agent 时使用的模型名称。
    model: String,
    metrics: Option<Arc<AgentMetrics>>,
    /// 审批工作流（write_file / execute_command 危险操作门控）。
    approval_workflow: Option<Arc<ApprovalWorkflow>>,
    /// 语义验证门控（verify_build 工具的 LLM-as-Judge 支持）。
    verification_gate: Option<Arc<VerificationGate>>,
    /// 规则记录器（工具执行失败时自动学习规则，供 GEPA 进化引擎消费）。
    rule_recorder: Option<Arc<RuleRecorder>>,
    /// 使用统计追踪器（技能调用频率、文档访问热度）。
    usage_stats: Option<Arc<UsageStats>>,
    definitions: Vec<ToolDefinition>,
    /// 执行轨迹（GEPA 引擎消费）。
    execution_history: Arc<Mutex<Vec<ExecutionHistory>>>,
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
    ) -> Vec<(String, Result<serde_json::Value, ToolExecutionError>)> {
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

    async fn execute_single(
        &self,
        call: &ToolCall,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let start = std::time::Instant::now();
        let arguments = &call.function.arguments;
        let result = match call.function.name.as_str() {
            "read_file" => {
                let params: ReadFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_path(std::path::Path::new(&params.path))
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                crate::executor::execute_read_file(&params.path)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "write_file" => {
                let params: WriteFileParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_file_write()
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                self.security_policy
                    .check_path(std::path::Path::new(&params.path))
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                self.security_policy
                    .check_file_size(params.content.len() as u64)
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
                // Approval workflow check — auto-approve safe paths,
                // deny critical paths (e.g. /etc/, .env), request human
                // approval for Medium/High risk paths.
                if let Some(ref approval) = self.approval_workflow {
                    let action = Action::WriteFile {
                        path: params.path.clone(),
                        content: params.content.clone(),
                    };
                    let resp = approval
                        .request_approval("tool-execution", &action)
                        .await
                        .map_err(|e| {
                            ToolExecutionError::ExecutionFailed(format!(
                                "审批工作流错误: {}",
                                e
                            ))
                        })?;
                    if resp.decision != ApprovalDecision::Approve {
                        return Err(ToolExecutionError::SecurityViolation(format!(
                            "操作被审批工作流拒绝: {}",
                            resp.reason.unwrap_or_default()
                        )));
                    }
                }
                crate::executor::execute_write_file(&params.path, &params.content)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "execute_command" => {
                let mut params: ExecuteCommandParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;

                // Check command against security policy.
                // If blocked but safety_mode is Transform, try rewriting to a safe equivalent.
                if let Err(security_err) = self.security_policy.check_command(&params.command) {
                    if let Some(transformed) =
                        self.security_policy.transform_command(&params.command)
                    {
                        tracing::info!(
                            original = %params.command,
                            transformed = %transformed,
                            "命令已由安全策略自动重写"
                        );
                        params.command = transformed;
                    } else {
                        return Err(ToolExecutionError::SecurityViolation(
                            security_err.to_string(),
                        ));
                    }
                }

                // Path sandbox for working directory
                if let Some(ref cwd) = params.cwd {
                    self.security_policy
                        .check_path(std::path::Path::new(cwd))
                        .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
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
                            ToolExecutionError::ExecutionFailed(format!(
                                "审批工作流错误: {}",
                                e
                            ))
                        })?;
                    if resp.decision != ApprovalDecision::Approve {
                        return Err(ToolExecutionError::SecurityViolation(format!(
                            "操作被审批工作流拒绝: {}",
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
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "search_code" => {
                let params: SearchCodeParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_search_code(&params.query, params.scope.as_deref())
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "search_knowledge" => {
                let params: SearchKnowledgeParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_search_knowledge(&params.query, params.top_k)
                    .await
            }
            "vfs_read" => {
                let params: VfsReadParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_vfs_read(&params.uri).await
            }
            "vfs_list" => {
                let params: VfsListParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_vfs_list(params.uri.as_deref()).await
            }
            "call_skill" => {
                let params: CallSkillParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_call_skill(&params.skill_id, &params.parameters)
                    .await
            }
            "run_tests" => {
                let params: RunTestsParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                crate::executor::execute_run_tests(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "verify_build" => {
                let params: VerifyBuildParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                // Use semantic verification if available, otherwise fall back
                // to exit code + pattern matching.
                if let Some(ref gate) = self.verification_gate {
                    let result = gate
                        .verify_build(
                            &params.command,
                            params.cwd.as_deref(),
                            params.timeout_secs,
                        )
                        .await
                        .map_err(|e| {
                            ToolExecutionError::ExecutionFailed(e.to_string())
                        })?;
                    Ok(serde_json::Value::from(result))
                } else {
                    crate::executor::execute_verify_build(
                        &params.command,
                        params.cwd.as_deref(),
                        params.timeout_secs,
                    )
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
                }
            }
            "ask_user" => {
                let params: AskUserParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                Err(ToolExecutionError::AskUser(params.question))
            }
            "self_check" => self.execute_self_check().await,
            "knowledge_ingest" => {
                let params: KnowledgeIngestParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_knowledge_ingest(&params.path, params.category.as_deref())
                    .await
            }
            "delegate_to_agent" => {
                let params: DelegateToAgentParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_delegate_to_agent(&params.task, params.system_prompt.as_deref())
                    .await
            }
            _ => Err(ToolExecutionError::UnknownTool(call.function.name.clone())),
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
                let err_msg = result.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
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

    async fn execute_call_skill(
        &self,
        skill_id: &str,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        if let Some(ref skill_executor) = self.skill_executor {
            let request = SkillExecutionRequest::new(skill_id.to_string(), parameters.clone());
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
                Err(e) => Err(ToolExecutionError::ExecutionFailed(e.to_string())),
            }
        } else {
            Err(ToolExecutionError::ExecutionFailed(
                "SkillExecutor not configured".to_string(),
            ))
        }
    }

    async fn execute_search_knowledge(
        &self,
        query: &str,
        top_k: Option<usize>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed(
                "VFS not configured for knowledge search".to_string(),
            )
        })?;
        let limit = top_k.unwrap_or(5);
        let results: Vec<SearchResult> = vfs
            .search(query, limit, None)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))?;
        let items = futures::future::join_all(results.into_iter().map(|r| {
            let uri = r.uri.to_string();
            let uri_clone = r.uri.clone();
            let score = r.score;
            async move {
                let (l0, l1) = tokio::join!(
                    vfs.read_content(&uri_clone, ContentLevel::Abstract),
                    vfs.read_content(&uri_clone, ContentLevel::Overview),
                );
                serde_json::json!({
                    "uri": uri,
                    "score": score,
                    "abstract": l0.unwrap_or_default(),
                    "overview": l1.unwrap_or_default(),
                })
            }
        }))
        .await;
        Ok(serde_json::json!({
            "query": query,
            "count": items.len(),
            "results": items,
        }))
    }

    async fn execute_vfs_read(&self, uri: &str) -> Result<serde_json::Value, ToolExecutionError> {
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed("VFS not configured".to_string()))?;
        let parsed = TianyanUri::parse(uri)
            .map_err(|e| ToolExecutionError::InvalidParams(format!("无效 URI: {}", e)))?;
        // 加载三层内容，让 LLM 按需使用
        let (l0, l1, l2) = tokio::join!(
            vfs.read_content(&parsed, ContentLevel::Abstract),
            vfs.read_content(&parsed, ContentLevel::Overview),
            vfs.read_content(&parsed, ContentLevel::Detail),
        );
        Ok(serde_json::json!({
            "uri": uri,
            "abstract": l0.unwrap_or_default(),
            "overview": l1.unwrap_or_default(),
            "detail": l2.unwrap_or_default(),
        }))
    }

    async fn execute_vfs_list(
        &self,
        uri: Option<&str>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed("VFS not configured".to_string()))?;
        let target_uri = match uri {
            Some(s) => TianyanUri::parse(s)
                .map_err(|e| ToolExecutionError::InvalidParams(format!("无效 URI: {}", e)))?,
            None => TianyanUri::new(ContextNamespace::Knowledge, vec![]),
        };
        let entries = vfs
            .list(&target_uri)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))?;
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "uri": e.uri().to_string(),
                    "is_directory": e.is_directory(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "uri": target_uri.to_string(),
            "count": items.len(),
            "entries": items,
        }))
    }

    async fn execute_self_check(&self) -> Result<serde_json::Value, ToolExecutionError> {
        let metrics = self.metrics.as_ref().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed("AgentMetrics not configured".to_string())
        })?;
        Ok(metrics.query_harness_health().await)
    }

    async fn execute_knowledge_ingest(
        &self,
        path: &str,
        category: Option<&str>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let ingestor = self.knowledge_ingestor.as_ref().ok_or_else(|| {
            ToolExecutionError::ExecutionFailed("KnowledgeIngestor not configured".to_string())
        })?;

        let meta = std::fs::metadata(path).map_err(|e| {
            ToolExecutionError::ExecutionFailed(format!("无法访问路径 '{}': {}", path, e))
        })?;

        if meta.is_dir() {
            self.ingest_directory(ingestor, path, category).await
        } else {
            self.ingest_single_file(ingestor, path, category).await
        }
    }

    /// 摄入单个文件。
    async fn ingest_single_file(
        &self,
        ingestor: &KnowledgeIngestor,
        path: &str,
        category: Option<&str>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let content = tokio::fs::read(path).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed(format!("读取文件失败 '{}': {}", path, e))
        })?;

        let filename = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        let mut req =
            IngestionRequest::new(content, &filename).with_source(ContentSource::UserUpload);

        if let Some(cat) = category {
            if let Some(kc) = KnowledgeCategory::parse(cat) {
                req = req.with_category(kc);
            }
        }

        let result = ingestor
            .ingest(req)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("知识导入失败: {}", e)))?;

        Ok(serde_json::json!({
            "status": "completed",
            "document_id": result.document_id,
            "uri": result.uri.to_string(),
            "tokens_processed": result.tokens_processed,
            "processing_time_ms": result.processing_time_ms,
            "warnings": result.warnings,
        }))
    }

    /// 摄入目录下的所有文件。
    async fn ingest_directory(
        &self,
        ingestor: &KnowledgeIngestor,
        path: &str,
        category: Option<&str>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(path).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed(format!("读取目录失败 '{}': {}", path, e))
        })?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("读取目录条目失败: {}", e)))?
        {
            if entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false)
            {
                entries.push(entry.path().to_string_lossy().to_string());
            }
        }

        entries.sort();

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut total_tokens: usize = 0;
        let mut total_files: usize = 0;
        let mut errors: Vec<String> = Vec::new();

        for filepath in &entries {
            match self.ingest_single_file(ingestor, filepath, category).await {
                Ok(res) => {
                    total_tokens += res["tokens_processed"].as_u64().unwrap_or(0) as usize;
                    total_files += 1;
                    results.push(res);
                }
                Err(e) => {
                    errors.push(format!("{}: {}", filepath, e));
                }
            }
        }

        Ok(serde_json::json!({
            "status": "completed",
            "total_files": total_files,
            "total_tokens": total_tokens,
            "results": results,
            "errors": errors,
            "path": path,
        }))
    }

    /// Execute a sub-task delegation loop.
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
    fn execute_delegate_to_agent<'a>(
        &'a self,
        task: &'a str,
        system_prompt: Option<&'a str>,
    ) -> BoxFuture<'a, Result<serde_json::Value, ToolExecutionError>> {
        Box::pin(async move {
            const MAX_DELEGATION_TURNS: usize = 200;

            let model_service = self.model_service.clone().ok_or_else(|| {
                ToolExecutionError::ExecutionFailed(
                    "ModelService not configured for delegation".to_string(),
                )
            })?;

            let mut sub_messages: Vec<Message> = Vec::new();
            if let Some(prompt) = system_prompt {
                if !prompt.is_empty() {
                    sub_messages.push(Message::system(prompt));
                }
            }
            sub_messages.push(Message::user(task));

            let mut total_tokens: usize = 0;

            for _turn in 0..MAX_DELEGATION_TURNS {
                let request = ChatCompletionRequest::new(&self.model, sub_messages.clone());
                let response = model_service
                    .chat_completion(request)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))?;

                total_tokens += response.usage.total_tokens;
                let choice = response.choices.into_iter().next().ok_or_else(|| {
                    ToolExecutionError::ExecutionFailed("Empty response".to_string())
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
                    sub_messages.push(Message::assistant(
                        assistant_msg.content.clone(),
                    ));
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
