use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    KnowledgeIngestParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    SelfCheckParams, VerifyBuildParams, VfsListParams, VfsReadParams, WriteFileParams,
};
use crate::common::error::TianyanError;
use crate::common::types::{
    ContentLevel, ContentSource, ContextNamespace, Message, SearchResult, TianyanUri,
};
use crate::executor::approval::{ApprovalDecision, ApprovalWorkflow};
use crate::executor::Action;
use crate::executor::SecurityPolicy;
use crate::executor::VerificationGate;
use crate::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use crate::model::types::{ChatCompletionRequest, FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ChatService;
use crate::observability::usage_stats::UsageStats;
use crate::observability::AgentMetrics;
use crate::scheduler::tasks::RuleRecorder;
use crate::skills::learning::ExecutionHistory;
use crate::skills::{SkillExecutionRequest, SkillExecutor};
use crate::vfs::VirtualFileSystem;

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
    /// 待用户确认的操作指纹（审批拒绝 → 询问用户 → 用户回答后确认/放弃）。
    pending_approval_fingerprints: Arc<Mutex<Vec<String>>>,
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
    /// 由"询问用户"降级链路调用：agent loop 检测到 `approval_required` 错误后
    /// 转为追问，用户回答后通过 [`Self::confirm_pending_approval`] 决定是否放行。
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
                    approval
                        .record_user_confirmation_by_fingerprint(fp)
                        .await;
                }
            }
            tracing::info!(count = fingerprints.len(), "用户已确认执行待审批操作");
        } else {
            tracing::info!(count = fingerprints.len(), "用户拒绝执行待审批操作");
        }
        true
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

    // ── 工具执行方法（每个工具一个独立方法，由 execute_single 分发） ──

    /// 执行 read_file 工具：读取文件内容。
    async fn execute_read_file(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: ReadFileParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        self.security_policy
            .check_path(std::path::Path::new(&params.path))
            .map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))?;
        crate::executor::execute_read_file(&params.path)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 write_file 工具：写入文件内容（含审批工作流门控）。
    async fn execute_write_file(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: WriteFileParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        self.security_policy
            .check_file_write()
            .map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))?;
        self.security_policy
            .check_path(std::path::Path::new(&params.path))
            .map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))?;
        self.security_policy
            .check_file_size(params.content.len() as u64)
            .map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))?;
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
                    TianyanError::Custom(format!("tool: 执行失败：审批工作流错误: {}", e))
                })?;
            if resp.decision != ApprovalDecision::Approve {
                // 记录待确认操作：用户通过"询问用户"链路批准后放行
                self.remember_pending_approval(&action).await;
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规：操作需要用户确认（approval_required）：{}",
                    resp.reason.unwrap_or_default()
                )));
            }
        }
        crate::executor::execute_write_file(&params.path, &params.content)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 execute_command 工具：运行 shell 命令（含安全策略 + 审批门控）。
    async fn execute_execute_command(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let mut params: ExecuteCommandParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

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
                return Err(TianyanError::Custom(format!(
                    "tool: 安全违规：{}",
                    security_err,
                )));
            }
        }

        // Path sandbox for working directory
        if let Some(ref cwd) = params.cwd {
            self.security_policy
                .check_path(std::path::Path::new(cwd))
                .map_err(|e| TianyanError::Custom(format!("tool: 安全违规：{}", e)))?;
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
                    "tool: 安全违规：操作需要用户确认（approval_required）：{}",
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

    /// 执行 search_code 工具：使用 ripgrep 搜索代码模式。
    async fn execute_search_code(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SearchCodeParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        crate::executor::execute_search_code(&params.query, params.scope.as_deref())
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 search_knowledge 工具：语义搜索知识库。
    async fn execute_search_knowledge(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SearchKnowledgeParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "VFS not configured for knowledge search",
            ))
        })?;
        let limit = params.top_k.unwrap_or(5);
        let results: Vec<SearchResult> = vfs
            .search(&params.query, limit, None)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
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
            "query": params.query,
            "count": items.len(),
            "results": items,
        }))
    }

    /// 执行 vfs_read 工具：读取 VFS 条目的三层内容。
    async fn execute_vfs_read(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VfsReadParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "VFS not configured"))
        })?;
        let parsed = TianyanUri::parse(&params.uri)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：无效 URI: {}", e)))?;
        // 加载三层内容，让 LLM 按需使用
        let (l0, l1, l2) = tokio::join!(
            vfs.read_content(&parsed, ContentLevel::Abstract),
            vfs.read_content(&parsed, ContentLevel::Overview),
            vfs.read_content(&parsed, ContentLevel::Detail),
        );
        Ok(serde_json::json!({
            "uri": params.uri,
            "abstract": l0.unwrap_or_default(),
            "overview": l1.unwrap_or_default(),
            "detail": l2.unwrap_or_default(),
        }))
    }

    /// 执行 vfs_list 工具：列出 VFS 目录条目。
    async fn execute_vfs_list(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VfsListParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        let vfs = self.vfs.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "VFS not configured"))
        })?;
        let target_uri = match params.uri.as_deref() {
            Some(s) => TianyanUri::parse(s)
                .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：无效 URI: {}", e)))?,
            None => TianyanUri::new(ContextNamespace::Knowledge, vec![]),
        };
        let entries = vfs
            .list(&target_uri)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
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

    /// 执行 call_skill 工具：调用已注册技能。
    async fn execute_call_skill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: CallSkillParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        if let Some(ref skill_executor) = self.skill_executor {
            let request =
                SkillExecutionRequest::new(params.skill_id, params.parameters);
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

    /// 执行 run_tests 工具：运行测试命令。
    async fn execute_run_tests(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: RunTestsParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        crate::executor::execute_run_tests(
            &params.command,
            params.cwd.as_deref(),
            params.timeout_secs,
        )
        .await
        .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 verify_build 工具：构建验证（语义验证或退出码回退）。
    async fn execute_verify_build(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: VerifyBuildParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        // Use semantic verification if available, otherwise fall back
        // to exit code + pattern matching.
        if let Some(ref gate) = self.verification_gate {
            let result = gate
                .verify_build(&params.command, params.cwd.as_deref(), params.timeout_secs)
                .await
                .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))?;
            Ok(serde_json::Value::from(result))
        } else {
            crate::executor::execute_verify_build(
                &params.command,
                params.cwd.as_deref(),
                params.timeout_secs,
            )
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
        }
    }

    /// 执行 ask_user 工具：向用户追问。
    async fn execute_ask_user(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: AskUserParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        Err(TianyanError::Custom(format!(
            "tool: 需要追问：{}",
            params.question
        )))
    }

    /// 执行 self_check 工具：查询内部指标。
    async fn execute_self_check(&self) -> Result<serde_json::Value, TianyanError> {
        let metrics = self.metrics.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "AgentMetrics not configured"))
        })?;
        Ok(metrics.query_harness_health().await)
    }

    /// 执行 knowledge_ingest 工具：导入文件或目录到知识库。
    async fn execute_knowledge_ingest(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: KnowledgeIngestParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
        let ingestor = self.knowledge_ingestor.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!(
                "tool: 执行失败：{}",
                "KnowledgeIngestor not configured"
            ))
        })?;

        let meta = std::fs::metadata(&params.path).map_err(|e| {
            TianyanError::Custom(format!(
                "tool: 执行失败：无法访问路径 '{}': {}",
                params.path, e
            ))
        })?;

        if meta.is_dir() {
            self.ingest_directory(ingestor, &params.path, params.category.as_deref())
                .await
        } else {
            self.ingest_single_file(ingestor, &params.path, params.category.as_deref())
                .await
        }
    }

    /// 摄入单个文件。
    async fn ingest_single_file(
        &self,
        ingestor: &KnowledgeIngestor,
        path: &str,
        category: Option<&str>,
    ) -> Result<serde_json::Value, TianyanError> {
        let content = tokio::fs::read(path).await.map_err(|e| {
            TianyanError::Custom(format!("tool: 执行失败：读取文件失败 '{}': {}", path, e))
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
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：知识导入失败: {}", e)))?;

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
    ) -> Result<serde_json::Value, TianyanError> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(path).await.map_err(|e| {
            TianyanError::Custom(format!("tool: 执行失败：读取目录失败 '{}': {}", path, e))
        })?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：读取目录条目失败: {}", e)))?
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
    fn execute_delegate_to_agent<'a>(
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
