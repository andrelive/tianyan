use std::sync::Arc;

use tracing::{debug, info};

use tianyan::common::types::{ContentLevel, ContentSource, ContextNamespace, TianyanUri};
use tianyan::knowledge::{IngestionRequest, KnowledgeCategory, KnowledgeIngestor};
use tianyan::vfs::VirtualFileSystem;

use crate::api::knowledge::types::{
    DeleteEntryResponse, FileIngestResult, IngestRequest, IngestResponse, KnowledgeEntriesResponse,
    KnowledgeEntryItem, ReadEntryResponse, SearchQuery, SearchResponse, SearchResult,
    SearchResultMetadata, SearchSuggestionsResponse,
};
use crate::api::shared::error::ApiError;

/// 知识库命名空间前缀。
const KNOWLEDGE_NAMESPACE: &str = "tianyan://knowledge";

/// 列表里 L0 摘录的最大字符数（按字符而非字节计，中文不会被切成半个字）。
///
/// 200 字符 ≈ 列表项一行半到两行；再长前端也只能靠 `ellipsis` 截断，
/// 徒增响应体积。整段 L0 仍可经 `/entries/read?level=abstract` 取全文。
const EXCERPT_MAX_CHARS: usize = 200;

/// 检索结果里 `content` 预览的最大字符数。
///
/// 比列表摘录（200）略长：搜索结果是要判断「这条是不是我想找的」，多两行正文
/// 比再点一次省事。再长前端也只能截断，徒增响应体积。
const SEARCH_PREVIEW_MAX_CHARS: usize = 300;

/// 把 L0 摘要压成列表能直接显示的一行：合并所有空白（换行 / 多空格）后限长。
fn collapse_excerpt(text: &str) -> String {
    collapse_excerpt_n(text, EXCERPT_MAX_CHARS)
}

/// 同 [`collapse_excerpt`]，但限长可配（检索预览与列表摘录的预算不同）。
fn collapse_excerpt_n(text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        // 按字符边界截断：`&text[..200]` 会在多字节字符中间切断并 panic
        let mut out: String = one_line.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}
/// URI 的父路径（`tianyan://knowledge/projects/<sha>` → `knowledge/projects`）。
///
/// 检索结果里用这个值替代原来恒为空的 `source`：前端显示「这条落在哪个目录」，
/// 比一串 sha256 有用得多。字符串切分而非重新解析 URI —— 这里只想要展示用的段。
fn parent_path_of(uri: &TianyanUri) -> String {
    let s = uri.to_string();
    let body = s.strip_prefix("tianyan://").unwrap_or(&s);
    match body.rsplit_once('/') {
        Some((dir, _file)) => dir.to_string(),
        None => body.to_string(),
    }
}

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
            .await?;

        let total = results.len();
        let mut search_results: Vec<SearchResult> = Vec::new();
        for r in results.into_iter().skip(query.offset).take(query.limit) {
            // 每条都补上「一眼能认出这是什么」的三件套（标题 = L0 摘要、正文预览 =
            // L2 开头、来源 = 父目录路径）。
            //
            // 这里曾经把 `content` 留空、注释写「内容由前端按需调用 /entries/read
            // 加载」—— 但两个前端都没实现那一步，搜索结果就成了一排**只有分数、
            // 正文全空的卡片**（用户实测「知识条目显示不对」：20 张空卡片配
            // `-0.047` 这种负分）。服务端一次带回，比让每个前端各写一遍 N 次请求
            // 更省，也不会再各漂各的。
            let uri = r.uri.clone();
            let title = match self.vfs.read(&uri, ContentLevel::Abstract).await {
                Ok(text) => Some(collapse_excerpt(&text)).filter(|s| !s.is_empty()),
                Err(_) => None,
            };
            let preview = match self.vfs.read(&uri, ContentLevel::Detail).await {
                Ok(text) => Some(collapse_excerpt_n(&text, SEARCH_PREVIEW_MAX_CHARS))
                    .filter(|s| !s.is_empty()),
                Err(_) => None,
            };
            search_results.push(SearchResult {
                id: uri.to_string(),
                content: preview.unwrap_or_default(),
                source: parent_path_of(&uri),
                // 负分来自 `1 - 距离`（正交或反向即 ≤0）：对界面它只表示「有多像」，
                // 钳到 0..1 才能直接当百分比显示；向量层的原始分数保持不变
                // （注入链路另有 0.6 之类的绝对阈值，不能被这里改写）。
                score: r.score.clamp(0.0, 1.0),
                metadata: Some(SearchResultMetadata {
                    title,
                    url: None,
                    timestamp: None,
                    tags: None,
                }),
            });
        }

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

        let entries = self.vfs.list(&uri).await?;

        let items: Vec<KnowledgeEntryItem> = entries
            .into_iter()
            .map(|entry| {
                let name = entry
                    .uri()
                    .path()
                    .last()
                    .cloned()
                    .unwrap_or_else(|| entry.uri().to_string());
                // 摘要来自同一次 `list` 已读出的内存对象（`vfs.list` 返回完整
                // `ContextEntry`），因此带出 L0 摘录不增加任何存储往返。
                let abstract_excerpt = entry
                    .get_content(ContentLevel::Abstract)
                    .map(collapse_excerpt)
                    .filter(|s| !s.is_empty());
                KnowledgeEntryItem {
                    uri: entry.uri().to_string(),
                    name,
                    is_directory: entry.is_directory(),
                    has_abstract: entry.has_content(ContentLevel::Abstract),
                    has_overview: entry.has_content(ContentLevel::Overview),
                    has_detail: entry.has_content(ContentLevel::Detail),
                    abstract_excerpt,
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

        // ? 传播：条目不存在时由 core 返回 not_found 语义（404），不吞成 Internal
        let content = self.vfs.read(&uri, level_enum).await?;

        debug!(uri = %uri_str, level = %level_str, "读取知识库条目");
        Ok(ReadEntryResponse {
            uri: uri_str.to_string(),
            level: level_str.to_string(),
            content,
        })
    }

    /// 删除知识库条目（递归删除子条目 + 同步清理向量索引）。
    ///
    /// 仅允许删除 knowledge 命名空间下的条目；不存在的条目返回 404。
    pub async fn delete_entry(&self, uri_str: &str) -> Result<DeleteEntryResponse, ApiError> {
        if !uri_str.starts_with(KNOWLEDGE_NAMESPACE) {
            return Err(ApiError::BadRequest(
                "仅允许删除知识库命名空间 (tianyan://knowledge/) 条目".to_string(),
            ));
        }

        let uri = TianyanUri::parse(uri_str)
            .map_err(|e| ApiError::BadRequest(format!("无效的条目 URI: {}", e)))?;

        // 存在性检查：不存在的条目 → 404（幂等语义，避免 500）
        let exists = self.vfs.exists(&uri).await?;
        if !exists {
            return Err(ApiError::NotFound(format!("知识库条目未找到: {}", uri_str)));
        }

        // 递归删除：子条目 + SQLite vfs_entries + LanceDB 向量索引同步清理
        self.vfs.delete(&uri).await?;

        info!(uri = %uri_str, "知识库条目已删除");
        Ok(DeleteEntryResponse {
            uri: uri_str.to_string(),
            success: true,
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

    #[test]
    fn test_collapse_excerpt_merges_whitespace_and_truncates_on_char_boundary() {
        // 多行 + 连续空格的 L0 摘要压成一行（列表项只显示一行）
        assert_eq!(
            collapse_excerpt("A股策略基线：\n  纪律先行\n\n已验证 D4 为正"),
            "A股策略基线： 纪律先行 已验证 D4 为正"
        );

        // 超长按「字符」截断：`chars().take` 不会切出半个 UTF-8 字符
        let long = "中".repeat(EXCERPT_MAX_CHARS + 50);
        let out = collapse_excerpt(&long);
        assert_eq!(out.chars().count(), EXCERPT_MAX_CHARS + 1); // +1 = 省略号
        assert!(out.ends_with('…'));

        // 未超长不加省略号（避免"短摘要也带…"的视觉噪音）
        let short = collapse_excerpt("短摘要");
        assert_eq!(short, "短摘要");
    }

    #[test]
    fn test_collapse_excerpt_n_respects_custom_budget() {
        // 检索预览（300）与列表摘录（200）共用同一实现，预算必须各自生效
        let long = "中".repeat(500);
        assert_eq!(
            collapse_excerpt_n(&long, SEARCH_PREVIEW_MAX_CHARS)
                .chars()
                .count(),
            SEARCH_PREVIEW_MAX_CHARS + 1 // +1 = 省略号
        );
        assert_eq!(collapse_excerpt_n("短", SEARCH_PREVIEW_MAX_CHARS), "短");
    }

    #[test]
    fn test_parent_path_of_strips_file_segment() {
        // 检索结果的 `source`：落在哪个目录（前端显示这个，不是 sha256）
        let uri = TianyanUri::parse("tianyan://knowledge/projects/abc123").unwrap();
        assert_eq!(parent_path_of(&uri), "knowledge/projects");
    }

    #[test]
    fn test_parent_path_of_keeps_namespace_for_root_entries() {
        // 根级条目没有父目录段：退化成命名空间本身，前端不会显示空串
        let uri = TianyanUri::parse("tianyan://knowledge/abc123").unwrap();
        assert_eq!(parent_path_of(&uri), "knowledge");
    }
}
