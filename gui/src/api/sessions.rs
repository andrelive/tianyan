#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use super::{delete, get, post, ApiResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session: Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteSessionResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<super::chat::ChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
struct UpdateTitleRequest {
    title: String,
}

pub async fn list_sessions() -> ApiResult<ListSessionsResponse> {
    get("/sessions").await
}

pub async fn get_session(session_id: &str) -> ApiResult<SessionDetail> {
    get(&format!("/sessions/{}", session_id)).await
}

pub async fn create_session(title: impl Into<String>) -> ApiResult<CreateSessionResponse> {
    let request = CreateSessionRequest {
        title: title.into(),
        initial_message: None,
    };
    post("/sessions", &request).await
}

pub async fn delete_session(session_id: &str) -> ApiResult<DeleteSessionResponse> {
    delete(&format!("/sessions/{}", session_id)).await
}

pub async fn update_session_title(
    session_id: &str,
    title: impl Into<String>,
) -> ApiResult<Session> {
    let request = UpdateTitleRequest {
        title: title.into(),
    };
    post(&format!("/sessions/{}/title", session_id), &request).await
}
