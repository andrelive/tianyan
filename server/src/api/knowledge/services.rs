use std::sync::Arc;

use tracing::{debug, info};

use tianyan::common::types::{
    ContentLevel, ContentSource, ContextNamespace, SearchResult as CoreSearchResult, TianyanUri,
};
use tianyan::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use tianyan::vfs::VirtualFileSystem;

use crate::api::knowledge::types::{
    FileIngestResult, IngestRequest, IngestResponse, KnowledgeEntriesResponse, KnowledgeEntryItem,
    ReadEntryResponse, SearchQuery, SearchResponse, SearchResult, SearchResultMetadata,
    SearchSuggestionsResponse,
};
use crate::api::shared::error::ApiError;

/// 知识库命名空间前缀。
const KNOWLEDGE_NAMESPACE: &str = "tianyan://knowledge";

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

    /// 列出知识库命名空间下指定路径的直接子条目（只读）。
    pub async fn list_entries(
        &self,
        path: Option<&str>,
    ) -> Result<KnowledgeEntriesResponse, ApiError> {
        let uri_str = match path {
            Some(p) if !p.trim().is_empty() => {
                format!("{}/{}", KNOWLEDGE_NAMESPACE, p.trim().trim_matches('/'))
            }
            _ => KNOWLEDGE_NAMESPACE.to_string(),
        };

        let uri = TianyanUri::parse(&uri_str)
            .map_err(|e| ApiError::BadRequest(format!("无效的知识库路径: {}", e)))?;

        let entries = self
            .vfs
            .list(&uri)
            .await
            .map_err(|e| ApiError::Internal(format!("列出知识库条目失败: {}", e)))?;

        let items: Vec<KnowledgeEntryItem> = entries
            .into_iter()
            .map(|entry| {
                let name = entry
                    .uri()
                    .path()
                    .last()
                    .cloned()
                    .unwrap_or_else(|| entry.uri().to_string());
                KnowledgeEntryItem {
                    uri: entry.uri().to_string(),
                    name,
                    is_directory: entry.is_directory(),
                    has_abstract: entry.has_content(ContentLevel::Abstract),
                    has_overview: entry.has_content(ContentLevel::Overview),
                    has_detail: entry.has_content(ContentLevel::Detail),
                }
            })
            .collect();

        debug!(path = %uri_str, count = items.len(), "列出知识库条目");
        Ok(KnowledgeEntriesResponse { entries: items })
    }

    /// 读取知识库条目的指定层级内容（只读，仅限 knowledge 命名空间）。
    pub async fn read_entry(
        &self,
        uri_str: &str,
        level: Option<&str>,
    ) -> Result<ReadEntryResponse, ApiError> {
        if !uri_str.starts_with(KNOWLEDGE_NAMESPACE) {
            return Err(ApiError::BadRequest(
                "仅允许读取知识库命名空间 (tianyan://knowledge/)".to_string(),
            ));
        }

        let uri = TianyanUri::parse(uri_str)
            .map_err(|e| ApiError::BadRequest(format!("无效的条目 URI: {}", e)))?;

        let level_str = level.unwrap_or("detail");
        let level_enum = match level_str {
            "abstract" => ContentLevel::Abstract,
            "overview" => ContentLevel::Overview,
            "detail" => ContentLevel::Detail,
            other => {
                return Err(ApiError::BadRequest(format!(
                    "未知内容层级: {}（可选: abstract/overview/detail）",
                    other
                )));
            }
        };

        let content = self
            .vfs
            .read(&uri, level_enum)
            .await
            .map_err(|e| ApiError::Internal(format!("读取条目内容失败: {}", e)))?;

        debug!(uri = %uri_str, level = %level_str, "读取知识库条目");
        Ok(ReadEntryResponse {
            uri: uri_str.to_string(),
            level: level_str.to_string(),
            content,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs mock KnowledgeIngestor and VFS setup"]
    fn test_knowledge_service_creation() {
        // KnowledgeService::new(ingestor, vfs) requires full app state to construct
    }
}
