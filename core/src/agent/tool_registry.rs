use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    KnowledgeIngestParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    SelfCheckParams, VerifyBuildParams, VfsListParams, VfsReadParams, WriteFileParams,
};
use crate::common::types::{ContentLevel, ContentSource, ContextNamespace, Message, SearchResult, TianyanUri};
use crate::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use crate::executor::SecurityPolicy;
use crate::model::types::{ChatCompletionRequest, FunctionDefinition, ToolCall, ToolDefinition};
use crate::model::ChatService;
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
    metrics: Option<Arc<AgentMetrics>>,
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
            metrics: None,
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

    /// 设置可观测性指标（self_check 工具依赖）。
    pub fn with_metrics(mut self, metrics: Arc<AgentMetrics>) -> Self {
        self.metrics = Some(metrics);
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
                crate::executor::execute_write_file(&params.path, &params.content)
                    .await
                    .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "execute_command" => {
                let params: ExecuteCommandParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.security_policy
                    .check_command(&params.command)
                    .map_err(|e| ToolExecutionError::SecurityViolation(e.to_string()))?;
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
                self.execute_vfs_read(&params.uri)
                    .await
            }
            "vfs_list" => {
                let params: VfsListParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_vfs_list(params.uri.as_deref())
                    .await
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
                crate::executor::execute_verify_build(
                    &params.command,
                    params.cwd.as_deref(),
                    params.timeout_secs,
                )
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))
            }
            "ask_user" => {
                let params: AskUserParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                Err(ToolExecutionError::AskUser(params.question))
            }
            "self_check" => {
                self.execute_self_check()
                    .await
            }
            "knowledge_ingest" => {
                let params: KnowledgeIngestParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_knowledge_ingest(&params.path, params.category.as_deref())
                    .await
            }
            "delegate_to_agent" => {
                let params: DelegateToAgentParams = serde_json::from_str(arguments)
                    .map_err(|e| ToolExecutionError::InvalidParams(e.to_string()))?;
                self.execute_delegate_to_agent(&params.task, params.system_prompt.as_deref(), params.max_turns)
                    .await
            },
            _ => Err(ToolExecutionError::UnknownTool(call.function.name.clone())),
        };

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
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "VFS not configured for knowledge search".to_string(),
            ))?;
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

    async fn execute_vfs_read(
        &self,
        uri: &str,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "VFS not configured".to_string(),
            ))?;
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
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "VFS not configured".to_string(),
            ))?;
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

    async fn execute_self_check(
        &self,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let metrics = self
            .metrics
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "AgentMetrics not configured".to_string(),
            ))?;
        Ok(metrics.query_harness_health().await)
    }

    async fn execute_knowledge_ingest(
        &self,
        path: &str,
        category: Option<&str>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let ingestor = self
            .knowledge_ingestor
            .as_ref()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "KnowledgeIngestor not configured".to_string(),
            ))?;

        let meta = std::fs::metadata(path)
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("无法访问路径 '{}': {}", path, e)))?;

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
        let content = tokio::fs::read(path)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("读取文件失败 '{}': {}", path, e)))?;

        let filename = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        let mut req = IngestionRequest::new(content, &filename)
            .with_source(ContentSource::UserUpload);

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
        let mut dir = tokio::fs::read_dir(path)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("读取目录失败 '{}': {}", path, e)))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed(format!("读取目录条目失败: {}", e)))?
        {
            if entry.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
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

    async fn execute_delegate_to_agent(
        &self,
        task: &str,
        system_prompt: Option<&str>,
        max_turns: Option<usize>,
    ) -> Result<serde_json::Value, ToolExecutionError> {
        let model_service = self
            .model_service
            .clone()
            .ok_or_else(|| ToolExecutionError::ExecutionFailed(
                "ModelService not configured for delegation".to_string(),
            ))?;

        let mut sub_messages: Vec<Message> = Vec::new();
        if let Some(prompt) = system_prompt {
            if !prompt.is_empty() {
                sub_messages.push(Message::system(prompt));
            }
        }
        sub_messages.push(Message::user(task));

        let max_turns = max_turns.unwrap_or(5);
        let mut total_tokens: usize = 0;

        for _turn in 0..max_turns {
            let request = ChatCompletionRequest::new("default", sub_messages.clone());
            let response = model_service
                .chat_completion(request)
                .await
                .map_err(|e| ToolExecutionError::ExecutionFailed(e.to_string()))?;

            total_tokens += response.usage.total_tokens;
            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| ToolExecutionError::ExecutionFailed("Empty response".to_string()))?;

            let content = choice.message.content;
            if content.is_empty() && choice.message.tool_calls.is_some() {
                // Sub-task has tool calls — cannot execute them in isolated context,
                // pass through the call request as result
                let tc_names: Vec<String> = choice.message.tool_calls.as_ref()
                    .map(|tcs| tcs.iter().map(|tc| tc.function.name.clone()).collect())
                    .unwrap_or_default();
                return Ok(serde_json::json!({
                    "status": "tool_calls_required",
                    "requested_tools": tc_names,
                    "total_tokens": total_tokens,
                }));
            }

            return Ok(serde_json::json!({
                "result": content,
                "total_tokens": total_tokens,
            }));
        }

        Ok(serde_json::json!({
            "result": "sub-task reached max turns without final answer",
            "total_tokens": total_tokens,
        }))
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
