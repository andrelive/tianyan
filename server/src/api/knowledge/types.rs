use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 摄入文件请求（用于 JSON 元数据）
#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 来源类型
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 标签列表
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 附加元数据
    pub metadata: Option<Value>,
}

impl IngestRequest {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref source) = self.source_type {
            if source.trim().is_empty() {
                return Err("来源类型不能为空".to_string());
            }
            if source.len() > 100 {
                return Err("来源类型长度不能超过 100 个字符".to_string());
            }
        }
        if let Some(ref tags) = self.tags {
            if tags.len() > 50 {
                return Err("标签数量不能超过 50 个".to_string());
            }
            for tag in tags {
                if tag.trim().is_empty() {
                    return Err("标签不能为空".to_string());
                }
                if tag.len() > 50 {
                    return Err("单个标签长度不能超过 50 个字符".to_string());
                }
            }
        }
        Ok(())
    }
}

/// 摄入文件响应
#[derive(Debug, Serialize)]
pub struct IngestResponse {
    /// 是否成功
    pub success: bool,
    /// 任务标识
    pub job_id: String,
    /// 响应消息
    pub message: String,
    /// 文件处理结果列表
    pub files: Vec<FileIngestResult>,
}

/// 单个文件摄入结果
#[derive(Debug, Serialize)]
pub struct FileIngestResult {
    /// 文件名
    pub filename: String,
    /// 处理状态
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 错误信息
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 文档标识
    pub document_id: Option<String>,
}

/// 摄入状态响应
#[derive(Debug, Serialize)]
pub struct IngestStatusResponse {
    /// 任务标识
    pub job_id: String,
    /// 任务状态
    pub status: String,
    /// 处理进度百分比
    pub progress: f32,
    /// 总文件数
    pub total_files: u32,
    /// 已处理文件数
    pub processed_files: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 错误信息
    pub error: Option<String>,
}

/// 检索请求查询参数
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// 搜索查询字符串
    pub q: String,
    #[serde(default = "default_limit")]
    /// 返回结果数量上限
    pub limit: usize,
    #[serde(default)]
    /// 偏移量
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 过滤条件
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 会话标识
    pub session_id: Option<String>,
}

fn default_limit() -> usize {
    10
}

impl SearchQuery {
    /// 验证请求参数
    pub fn validate(&self) -> Result<(), String> {
        let trimmed = self.q.trim();
        if trimmed.is_empty() {
            return Err("查询字符串不能为空".to_string());
        }
        if trimmed.len() > 1000 {
            return Err("查询字符串长度不能超过 1000 个字符".to_string());
        }
        if self.limit == 0 || self.limit > 100 {
            return Err("limit 必须在 1 到 100 之间".to_string());
        }
        Ok(())
    }
}

/// 检索响应
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    /// 搜索查询字符串
    pub query: String,
    /// 搜索结果列表
    pub results: Vec<SearchResult>,
    /// 结果总数
    pub total: usize,
    /// 返回数量上限
    pub limit: usize,
    /// 偏移量
    pub offset: usize,
}

/// 单个检索结果
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// 文档标识
    pub id: String,
    /// 匹配内容
    pub content: String,
    /// 来源信息
    pub source: String,
    /// 相关性评分
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 结果元数据
    pub metadata: Option<SearchResultMetadata>,
}

/// 检索结果元数据
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 文档标题
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 文档 URL
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 时间戳
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// 标签列表
    pub tags: Option<Vec<String>>,
}

/// 检索建议响应
#[derive(Debug, Serialize)]
pub struct SearchSuggestionsResponse {
    /// 查询字符串
    pub query: String,
    /// 建议列表
    pub suggestions: Vec<String>,
}

/// 知识库条目浏览查询参数。
#[derive(Debug, Deserialize)]
pub struct ListEntriesQuery {
    /// 相对知识库根目录的路径（斜杠分隔），为空表示根目录。
    #[serde(default)]
    pub path: Option<String>,
}

/// 知识库条目列表中的单个条目。
#[derive(Debug, Serialize)]
pub struct KnowledgeEntryItem {
    /// 条目 URI。
    pub uri: String,
    /// 条目名称（路径最后一段）。
    pub name: String,
    /// 是否为目录。
    pub is_directory: bool,
    /// 是否包含 L0 摘要。
    pub has_abstract: bool,
    /// 是否包含 L1 概览。
    pub has_overview: bool,
    /// 是否包含 L2 详情。
    pub has_detail: bool,
}

/// 知识库条目列表响应。
#[derive(Debug, Serialize)]
pub struct KnowledgeEntriesResponse {
    /// 当前目录下的条目。
    pub entries: Vec<KnowledgeEntryItem>,
}

/// 读取知识条目的查询参数。
#[derive(Debug, Deserialize)]
pub struct ReadEntryQuery {
    /// 条目 URI（必须位于 tianyan://knowledge/ 命名空间）。
    pub uri: String,
    /// 内容层级：abstract | overview | detail，缺省 detail。
    #[serde(default)]
    pub level: Option<String>,
}

/// 读取知识条目响应。
#[derive(Debug, Serialize)]
pub struct ReadEntryResponse {
    /// 条目 URI。
    pub uri: String,
    /// 实际读取的层级。
    pub level: String,
    /// 内容。
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingest_response_serialization() {
        let response = IngestResponse {
            success: true,
            job_id: "ingest-123".to_string(),
            message: "文件已加入队列".to_string(),
            files: vec![FileIngestResult {
                filename: "test.txt".to_string(),
                status: "queued".to_string(),
                error: None,
                document_id: Some("doc-456".to_string()),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("ingest-123"));
        assert!(json.contains("test.txt"));
    }

    #[test]
    fn test_search_response_serialization() {
        let response = SearchResponse {
            query: "rust".to_string(),
            results: vec![SearchResult {
                id: "doc-001".to_string(),
                content: "Rust is a programming language".to_string(),
                source: "docs".to_string(),
                score: 0.95,
                metadata: None,
            }],
            total: 1,
            limit: 10,
            offset: 0,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("rust"));
        assert!(json.contains("Rust is a programming language"));
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }
}
