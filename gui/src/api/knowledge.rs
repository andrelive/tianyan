use serde::{Deserialize, Serialize};

use super::{get, get_api_base, parse_response_text, ApiError, ApiResult};

/// 知识库搜索结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeSearchResult {
    pub id: String,
    pub content: String,
    pub source: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<KnowledgeResultMetadata>,
}

/// 搜索结果元数据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeResultMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// 搜索响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeSearchResponse {
    pub query: String,
    pub results: Vec<KnowledgeSearchResult>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

/// 搜索建议响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KnowledgeSuggestionsResponse {
    pub query: String,
    pub suggestions: Vec<String>,
}

/// 文件摄入结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileIngestResultInfo {
    pub filename: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
}

/// 摄入响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestResponse {
    pub success: bool,
    pub job_id: String,
    pub message: String,
    pub files: Vec<FileIngestResultInfo>,
}

/// 摄入状态响应
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestStatusResponse {
    pub job_id: String,
    pub status: String,
    pub progress: f32,
    pub total_files: u32,
    pub processed_files: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn search(
    query: &str,
    limit: usize,
    offset: usize,
    filter: Option<&str>,
    session_id: Option<&str>,
) -> ApiResult<KnowledgeSearchResponse> {
    let mut url = format!(
        "/knowledge/search?q={}&limit={}&offset={}",
        urlencoding::encode(query),
        limit,
        offset,
    );
    if let Some(f) = filter {
        url.push_str(&format!("&filter={}", urlencoding::encode(f)));
    }
    if let Some(s) = session_id {
        url.push_str(&format!("&session_id={}", urlencoding::encode(s)));
    }
    get(&url).await
}

pub async fn get_suggestions(query: &str) -> ApiResult<KnowledgeSuggestionsResponse> {
    let url = format!(
        "/knowledge/search/suggestions?q={}",
        urlencoding::encode(query)
    );
    get(&url).await
}

/// 摄入文件（multipart/form-data）
pub async fn ingest_files(
    files: Vec<web_sys::File>,
    tags: Option<Vec<String>>,
    source_type: Option<String>,
) -> ApiResult<IngestResponse> {
    let form_data = web_sys::FormData::new().map_err(|e| ApiError {
        message: format!("Failed to create FormData: {:?}", e),
        code: None,
    })?;

    for file in &files {
        form_data
            .append_with_blob_and_filename("files", file, &file.name())
            .map_err(|e| ApiError {
                message: format!("Failed to append file: {:?}", e),
                code: None,
            })?;
    }

    let metadata = serde_json::json!({
        "tags": tags,
        "source_type": source_type,
    });
    form_data
        .append_with_str("metadata", &metadata.to_string())
        .map_err(|e| ApiError {
            message: format!("Failed to append metadata: {:?}", e),
            code: None,
        })?;

    let url = format!("{}/knowledge/ingest", get_api_base());
    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_body(&wasm_bindgen::JsValue::from(form_data));

    let request = web_sys::Request::new_with_str_and_init(&url, &opts).map_err(|e| ApiError {
        message: format!("Failed to create request: {:?}", e),
        code: None,
    })?;

    let window = web_sys::window().ok_or_else(|| ApiError {
        message: "No window object".to_string(),
        code: None,
    })?;

    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| ApiError {
            message: format!("Request failed: {:?}", e),
            code: None,
        })?;

    let response = web_sys::Response::from(resp_value);
    let status = response.status();
    let text = wasm_bindgen_futures::JsFuture::from(response.text().map_err(|e| ApiError {
        message: format!("Failed to read response: {:?}", e),
        code: None,
    })?)
    .await
    .map_err(|e| ApiError {
        message: format!("Failed to read response text: {:?}", e),
        code: None,
    })?;

    let text_str = text.as_string().unwrap_or_default();
    parse_response_text(status, &text_str)
}

pub async fn get_ingest_status(job_id: &str) -> ApiResult<IngestStatusResponse> {
    get(&format!("/knowledge/ingest/{}/status", job_id)).await
}
