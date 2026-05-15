use std::sync::Arc;

use axum::{
    extract::{Multipart, Query, State},
    Json,
};
use tracing::{debug, error, info, warn};

use crate::api::knowledge::services::KnowledgeService;
use crate::api::knowledge::types::{
    IngestRequest, IngestResponse, IngestStatusResponse, SearchQuery, SearchResponse,
    SearchSuggestionsResponse,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 处理文件上传和摄入
pub async fn ingest_handler(
    State(_state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, ApiError> {
    info!("收到文件摄入请求");

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut metadata: Option<IngestRequest> = None;

    // 处理 multipart 表单中的每个字段
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("unknown").to_string();

        if name == "files" {
            let filename = field.file_name().map(|s| s.to_string());

            match filename {
                Some(filename) => {
                    debug!("处理文件: {}", filename);

                    match field.bytes().await {
                        Ok(content) => {
                            info!("收到文件: {} ({} 字节)", filename, content.len());
                            files.push((filename, content.to_vec()));
                        }
                        Err(e) => {
                            error!("读取文件 {} 失败: {}", filename, e);
                        }
                    }
                }
                None => {
                    warn!("字段没有文件名，跳过");
                }
            }
        } else if name == "metadata" {
            match field.text().await {
                Ok(text) => {
                    debug!("收到元数据: {}", text);
                    if let Ok(req) = serde_json::from_str::<IngestRequest>(&text) {
                        metadata = Some(req);
                    }
                }
                Err(e) => {
                    warn!("读取元数据失败: {}", e);
                }
            }
        } else {
            match field.text().await {
                Ok(text) => {
                    debug!("收到字段 '{}'，值: {}", name, text);
                }
                Err(e) => {
                    warn!("读取字段 '{}' 失败: {}", name, e);
                }
            }
        }
    }

    let service = KnowledgeService::new();
    let request = metadata.unwrap_or(IngestRequest {
        source_type: None,
        tags: None,
        metadata: None,
    });

    if let Err(e) = request.validate() {
        return Err(ApiError::BadRequest(e));
    }

    service
        .ingest_files(files, request)
        .await
        .map(Json)
        .map_err(|e| {
            error!("摄入错误: {}", e);
            ApiError::Internal(format!("摄入失败: {}", e))
        })
}

/// 获取摄入任务状态
pub async fn get_ingest_status(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<Json<IngestStatusResponse>, ApiError> {
    info!("获取摄入任务状态: {}", job_id);

    let service = KnowledgeService::new();

    service
        .get_ingest_status(&job_id)
        .await
        .map(Json)
        .map_err(|e| {
            error!("获取摄入状态失败: {}", e);
            ApiError::Internal(format!("获取摄入状态失败: {}", e))
        })
}

/// 执行检索
pub async fn search_handler(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    if let Err(e) = query.validate() {
        return Err(ApiError::BadRequest(e));
    }

    let service = KnowledgeService::new();

    service.search(query).await.map(Json).map_err(|e| {
        error!("搜索失败: {}", e);
        ApiError::Internal(format!("搜索失败: {}", e))
    })
}

/// 获取检索建议
pub async fn search_suggestions_handler(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchSuggestionsResponse>, ApiError> {
    if let Err(e) = query.validate() {
        return Err(ApiError::BadRequest(e));
    }

    let service = KnowledgeService::new();

    service
        .get_search_suggestions(&query.q)
        .await
        .map(Json)
        .map_err(|e| {
            error!("获取搜索建议失败: {}", e);
            ApiError::Internal(format!("获取搜索建议失败: {}", e))
        })
}
