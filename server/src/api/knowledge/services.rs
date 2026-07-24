use std::sync::Arc;

use tracing::{debug, info};

use tianyan::common::types::{ContentSource, ContextNamespace, SearchResult as CoreSearchResult};
use tianyan::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use tianyan::vfs::VirtualFileSystem;

use crate::api::knowledge::types::{
    FileIngestResult, IngestRequest, IngestResponse, SearchQuery, SearchResponse, SearchResult,
    SearchResultMetadata, SearchSuggestionsResponse,
};
use crate::api::shared::error::ApiError;

/// 知识服务，管理文件摄入和检索
pub struct KnowledgeService {
    ingestor: Arc<KnowledgeIngestor>,
    vfs: Arc<dyn VirtualFileSystem>,
}

impl KnowledgeService {
    /// 创建新的知识服务
    pub fn new(ingestor: Arc<KnowledgeIngestor>, vfs: Arc<dyn VirtualFileSystem>) -> Self {
        Self { ingestor, vfs }
    }

    /// 摄入文件
    pub async fn ingest_files(
        &self,
        files: Vec<(String, Vec<u8>)>,
        request: IngestRequest,
    ) -> Result<IngestResponse, ApiError> {
        let mut results = Vec::new();
        let mut total_size: usize = 0;

        for (filename, content) in files {
            total_size += content.len();
            info!("处理文件: {} ({} 字节)", filename, content.len());

            let mut ingestion = IngestionRequest::new(content, &filename)
                .with_tags(request.tags.clone().unwrap_or_default())
                .with_source(ContentSource::UserUpload);

            if let Some(ref source_type) = request.source_type {
                let category = if source_type == "code" {
                    KnowledgeCategory::CodeSnippets
                } else {
                    KnowledgeCategory::Technical
                };
                ingestion = ingestion.with_category(category);
            }

            match self.ingestor.ingest(ingestion).await {
                Ok(result) => {
                    results.push(FileIngestResult {
                        filename: filename.clone(),
                        status: "completed".to_string(),
                        error: None,
                        document_id: Some(result.document_id),
                    });
                }
                Err(e) => {
                    results.push(FileIngestResult {
                        filename: filename.clone(),
                        status: "failed".to_string(),
                        error: Some(e.to_string()),
                        document_id: None,
                    });
                }
            }
        }

        let success = results.iter().any(|r| r.status == "completed");
        let message = format!("处理了 {} 个文件 (共 {} 字节)", results.len(), total_size);

        Ok(IngestResponse {
            success,
            job_id: format!(
                "ingest-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("job")
            ),
            message,
            files: results,
        })
    }

    /// 执行检索
    pub async fn search(&self, query: SearchQuery) -> Result<SearchResponse, ApiError> {
        info!(
            "检索查询: '{}' (限制: {}, 偏移: {})",
            query.q, query.limit, query.offset
        );

        if query.q.is_empty() {
            return Ok(SearchResponse {
                query: query.q,
                results: vec![],
                total: 0,
                limit: query.limit,
                offset: query.offset,
            });
        }

        let results = self
            .vfs
            .search(
                &query.q,
                query.limit + query.offset,
                Some(ContextNamespace::Knowledge),
            )
            .await
            .map_err(|e| ApiError::Internal(format!("搜索失败: {}", e)))?;

        let total = results.len();
        let search_results: Vec<SearchResult> = results
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .map(|r: CoreSearchResult| SearchResult {
                id: r.uri.to_string(),
                content: r.content.unwrap_or_default(),
                source: String::new(),
                score: r.score,
                metadata: Some(SearchResultMetadata {
                    title: None,
                    url: None,
                    timestamp: None,
                    tags: None,
                }),
            })
            .collect();

        Ok(SearchResponse {
            query: query.q,
            results: search_results,
            total,
            limit: query.limit,
            offset: query.offset,
        })
    }

    /// 获取检索建议
    pub async fn get_search_suggestions(
        &self,
        query: &str,
    ) -> Result<SearchSuggestionsResponse, ApiError> {
        debug!("检索建议: '{}'", query);

        let suggestions = if query.len() < 2 {
            vec![]
        } else {
            vec![
                format!("{} 是什么", query),
                format!("{} 怎么用", query),
                format!("{} 示例", query),
                format!("{} 教程", query),
            ]
        };

        Ok(SearchSuggestionsResponse {
            query: query.to_string(),
            suggestions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs mock KnowledgeIngestor and VFS setup"]
    fn test_knowledge_service_creation() {
        // KnowledgeService::new(ingestor, vfs) requires full app state to construct
    }
}
