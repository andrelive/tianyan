//! 知识库类工具执行器：search_knowledge / knowledge_ingest（含文件与目录摄入）。

use crate::agent::tool_params::{KnowledgeIngestParams, SearchKnowledgeParams};
use crate::common::error::TianyanError;
use crate::common::types::{ContentLevel, ContentSource, SearchResult};
use crate::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};

use super::{parse_params, vfs_content_field, ToolRegistry};

impl ToolRegistry {
    /// 执行 search_knowledge 工具：语义搜索知识库。
    pub(crate) async fn execute_search_knowledge(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: SearchKnowledgeParams = parse_params(arguments)?;
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

    /// 执行 knowledge_ingest 工具：导入文件或目录到知识库。
    pub(crate) async fn execute_knowledge_ingest(
        &self,
        arguments: &str,
    ) -> Result<serde_json::Value, TianyanError> {
        let params: KnowledgeIngestParams = parse_params(arguments)?;
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
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "knowledge_ops_tests.rs"]
mod tests;
