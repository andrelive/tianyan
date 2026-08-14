use std::sync::Arc;

use axum::{
    extract::{Multipart, Query, State},
    Json,
};
use tracing::{debug, error, info, warn};

use crate::api::knowledge::services::KnowledgeService;
use crate::api::knowledge::types::{
    DeleteEntryRequest, DeleteEntryResponse, IngestRequest, IngestResponse,
    KnowledgeEntriesResponse, ListEntriesQuery, ReadEntryQuery, ReadEntryResponse, SearchQuery,
    SearchResponse, SearchSuggestionsResponse,
};
use crate::api::shared::error::ApiError;
use crate::state::AppState;

/// 处理文件上传和摄入
pub async fn ingest_handler(
    State(state): State<Arc<AppState>>,
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

    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);
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
        .inspect_err(|e| error!("摄入错误: {}", e))
        .map(Json)
}

/// 执行检索
pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    if let Err(e) = query.validate() {
        return Err(ApiError::BadRequest(e));
    }

    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);

    service
        .search(query)
        .await
        .inspect_err(|e| error!("搜索失败: {}", e))
        .map(Json)
}

/// 获取检索建议
pub async fn search_suggestions_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchSuggestionsResponse>, ApiError> {
    if let Err(e) = query.validate() {
        return Err(ApiError::BadRequest(e));
    }

    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);

    service
        .get_search_suggestions(&query.q)
        .await
        .inspect_err(|e| error!("获取搜索建议失败: {}", e))
        .map(Json)
}

/// 列出知识库条目（只读浏览）
pub async fn list_entries_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListEntriesQuery>,
) -> Result<Json<KnowledgeEntriesResponse>, ApiError> {
    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);

    let resp = service.list_entries(query.path.as_deref()).await?;
    Ok(Json(resp))
}

/// 读取知识库条目内容（只读浏览）
pub async fn read_entry_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ReadEntryQuery>,
) -> Result<Json<ReadEntryResponse>, ApiError> {
    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);

    let resp = service
        .read_entry(&query.uri, query.level.as_deref())
        .await?;
    Ok(Json(resp))
}

/// 删除知识库条目（递归删除子条目 + 同步清理向量索引）
pub async fn delete_entry_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeleteEntryRequest>,
) -> Result<Json<DeleteEntryResponse>, ApiError> {
    if request.uri.trim().is_empty() {
        return Err(ApiError::BadRequest("条目 URI 不能为空".to_string()));
    }

    let ingestor = state.create_knowledge_ingestor().await?;
    let vfs = state.vfs();
    let service = KnowledgeService::new(Arc::new(ingestor), vfs);

    service.delete_entry(&request.uri).await.map(Json)
}
