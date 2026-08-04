//! 工具执行器实现。
//!
//! 14 个 OpenAI function-calling 工具的具体执行逻辑，
//! 从 `ToolRegistry` 拆出以保持注册表聚焦状态与调度。

use std::collections::HashMap;

use futures::future::BoxFuture;

use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, ExecuteCommandParams,
    KnowledgeIngestParams, ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams,
    VerifyBuildParams, VfsListParams, VfsReadParams, WriteFileParams,
};
use crate::common::error::TianyanError;
use crate::common::types::{
    ContentLevel, ContentSource, ContextNamespace, Message, SearchResult, TianyanUri,
};
use crate::executor::approval::ApprovalDecision;
use crate::executor::Action;
use crate::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use crate::model::types::ChatCompletionRequest;
use crate::skills::SkillExecutionRequest;

use super::{vfs_content_field, ToolRegistry};

impl ToolRegistry {
    /// 执行 read_file 工具：读取文件内容。
    pub(crate) async fn execute_read_file(
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
    pub(crate) async fn execute_write_file(
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
                    "tool: 安全违规：操作需要用户确认：{}",
                    resp.reason.unwrap_or_default()
                )));
            }
        }
        crate::executor::execute_write_file(&params.path, &params.content)
            .await
            .map_err(|e| TianyanError::Custom(format!("tool: 执行失败：{}", e)))
    }

    /// 执行 execute_command 工具：运行 shell 命令（含安全策略 + 审批门控）。
    pub(crate) async fn execute_execute_command(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let mut params: ExecuteCommandParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;

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

    /// 执行 search_code 工具：使用 ripgrep 搜索代码模式。
    pub(crate) async fn execute_search_code(
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
    pub(crate) async fn execute_search_knowledge(
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
                    "abstract": vfs_content_field(l0),
                    "overview": vfs_content_field(l1),
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
    pub(crate) async fn execute_vfs_read(
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
            "abstract": vfs_content_field(l0),
            "overview": vfs_content_field(l1),
            "detail": vfs_content_field(l2),
        }))
    }

    /// 执行 vfs_list 工具：列出 VFS 目录条目。
    pub(crate) async fn execute_vfs_list(
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
    pub(crate) async fn execute_call_skill(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: CallSkillParams = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: 参数无效：{}", e)))?;
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

    /// 执行 run_tests 工具：运行测试命令。
    pub(crate) async fn execute_run_tests(
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
    pub(crate) async fn execute_verify_build(
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
    pub(crate) async fn execute_ask_user(
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
    pub(crate) async fn execute_self_check(&self) -> Result<serde_json::Value, TianyanError> {
        let metrics = self.metrics.as_ref().ok_or_else(|| {
            TianyanError::Custom(format!("tool: 执行失败：{}", "AgentMetrics not configured"))
        })?;
        Ok(metrics.query_harness_health().await)
    }

    /// 执行 knowledge_ingest 工具：导入文件或目录到知识库。
    pub(crate) async fn execute_knowledge_ingest(
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
    pub(crate) async fn ingest_single_file(
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
    pub(crate) async fn ingest_directory(
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
    pub(crate) fn execute_delegate_to_agent<'a>(
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
}
