use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 摄入文件请求（用于 JSON 元数据）
#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub success: bool,
    pub job_id: String,
    pub message: String,
    pub files: Vec<FileIngestResult>,
}

/// 单个文件摄入结果
#[derive(Debug, Serialize)]
pub struct FileIngestResult {
    pub filename: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
}

/// 摄入状态响应
#[derive(Debug, Serialize)]
pub struct IngestStatusResponse {
    pub job_id: String,
    pub status: String,
    pub progress: f32,
    pub total_files: u32,
    pub processed_files: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 检索请求查询参数
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

/// 单个检索结果
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub source: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SearchResultMetadata>,
}

/// 检索结果元数据
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// 检索建议响应
#[derive(Debug, Serialize)]
pub struct SearchSuggestionsResponse {
    pub query: String,
    pub suggestions: Vec<String>,
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
