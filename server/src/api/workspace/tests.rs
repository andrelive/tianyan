//! workspace 领域测试
//!
//! RED → GREEN：本文件先行编写，验证 workspace 服务与路由的行为契约。
//! 服务层测试直接构造 [`WorkspaceService`]（跟随 knowledge 领域"组件注入"模式），
//! 处理器层测试通过 `create_app` 构建完整 AppState 后以 `tower::ServiceExt::oneshot`
//! 直击路由。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tempfile::tempdir;
use tianyan::config::{
    ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig, ProviderConfig,
    TianyanConfig,
};
use tianyan::snapshot::SnapshotManager;
use tower::ServiceExt;

use crate::api::shared::error::ApiError;
use crate::api::workspace::services::WorkspaceService;
use crate::api::workspace::types::TreeResponse;

// ---------------------------------------------------------------------------
// 测试夹具
// ---------------------------------------------------------------------------

/// 内存会话管理器 mock（服务层测试用；支持预置会话工作目录绑定）。
struct MockSessionManager {
    sessions: std::sync::Mutex<std::collections::HashMap<String, tianyan::session::Session>>,
}

impl MockSessionManager {
    fn new() -> Self {
        Self {
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 预置会话（可携带工作目录绑定）。
    fn with_sessions(sessions: Vec<tianyan::session::Session>) -> Self {
        let map = sessions
            .into_iter()
            .map(|s| (s.session_id.clone(), s))
            .collect();
        Self {
            sessions: std::sync::Mutex::new(map),
        }
    }
}

#[async_trait::async_trait]
impl tianyan::session::SessionManager for MockSessionManager {
    async fn create_session(
        &self,
        id: &str,
        _message: tianyan::Message,
    ) -> tianyan::Result<tianyan::session::Session> {
        let session = tianyan::session::Session::new(id);
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), session.clone());
        Ok(session)
    }

    async fn get_session(&self, id: &str) -> tianyan::Result<Option<tianyan::session::Session>> {
        Ok(self.sessions.lock().unwrap().get(id).cloned())
    }

    // ADR-039：破坏性写不在 trait 上（写路径唯一入口 = 会话工作集）
    async fn list_sessions(&self) -> tianyan::Result<Vec<tianyan::session::Session>> {
        Ok(self.sessions.lock().unwrap().values().cloned().collect())
    }
}

/// 使用临时工作目录构建服务（无快照管理器）。
fn service_with_workdir(workdir: PathBuf) -> WorkspaceService {
    WorkspaceService::new(Arc::new(MockSessionManager::new()), Some(workdir), None)
}

/// 未配置 working_directory 的服务。
fn service_without_workdir() -> WorkspaceService {
    WorkspaceService::new(Arc::new(MockSessionManager::new()), None, None)
}

/// 构建带 mock 模型提供商的测试配置（与 `server/tests/common/factory.rs` 同构；
/// `ModelServices::from_config` 强制要求 chat/embedding/vision 三者齐备）。
fn test_config(data_dir: &Path, workdir: Option<&Path>) -> TianyanConfig {
    let mut config = TianyanConfig::default();
    config.storage.data_dir = data_dir.to_path_buf();
    config.agent.working_directory = workdir.map(|p| p.to_string_lossy().into_owned());
    config.models = ModelsConfig {
        providers: vec![ProviderConfig {
            name: "mock".to_string(),
            endpoint: "http://localhost:11434/v1".to_string(),
            api_key: Some("test-key".to_string()),
            models: vec![
                ModelEntry {
                    name: "test-model".to_string(),
                    capabilities: vec![ModelCapability::Chat],
                    ..Default::default()
                },
                ModelEntry {
                    name: "embed".to_string(),
                    capabilities: vec![ModelCapability::TextEmbedding],
                    ..Default::default()
                },
                ModelEntry {
                    name: "vision".to_string(),
                    capabilities: vec![ModelCapability::Vision],
                    ..Default::default()
                },
            ],
            timeout: 30,
            enabled: true,
            headers: std::collections::HashMap::new(),
            thinking_field: None,
            dialect: None,
        }],
        preferences: ModelPreferences {
            chat: Some(ModelRef {
                provider: "mock".to_string(),
                model: "test-model".to_string(),
            }),
            embedding: Some(ModelRef {
                provider: "mock".to_string(),
                model: "embed".to_string(),
            }),
            vision: Some(ModelRef {
                provider: "mock".to_string(),
                model: "vision".to_string(),
            }),
        },
    };
    config
}

/// 创建含嵌套文件的工作目录：`src/nested`、`zeta`、`Cargo.toml`、`alpha.txt`。
fn create_workdir(root: &Path) -> PathBuf {
    let workdir = root.join("work");
    std::fs::create_dir_all(workdir.join("src/nested")).unwrap();
    std::fs::create_dir_all(workdir.join("zeta")).unwrap();
    std::fs::write(workdir.join("Cargo.toml"), "x".repeat(42)).unwrap();
    std::fs::write(workdir.join("alpha.txt"), "hello").unwrap();
    workdir
}

/// 去掉 Windows `\\?\` 逐字前缀（与服务端展示逻辑一致）。
fn strip_verbatim(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

// ---------------------------------------------------------------------------
// tree
// ---------------------------------------------------------------------------

/// 根目录单层列出：目录在前、文件在后，各自按名称升序；文件带 size/mtime。
#[tokio::test]
async fn tree_lists_entries_dirs_first_then_files_alpha() {
    let dir = tempdir().unwrap();
    let workdir = create_workdir(dir.path());
    let service = service_with_workdir(workdir.clone());

    let resp: TreeResponse = service.tree("", None).await.unwrap();

    assert_eq!(
        resp.root,
        strip_verbatim(&workdir.canonicalize().unwrap().to_string_lossy())
    );
    assert_eq!(resp.path, "");
    let names: Vec<&str> = resp.entries.iter().map(|e| e.name.as_str()).collect();
    // 目录（src、zeta）在前且升序；文件（Cargo.toml、alpha.txt）在后且按字节序升序
    assert_eq!(names, vec!["src", "zeta", "Cargo.toml", "alpha.txt"]);

    let dir_entry = &resp.entries[0];
    assert_eq!(dir_entry.entry_type, "dir");
    assert_eq!(dir_entry.path, "src");
    assert!(dir_entry.size.is_none(), "目录不应携带 size");
    assert!(dir_entry.mtime.is_none(), "目录不应携带 mtime");

    let file_entry = resp.entries.iter().find(|e| e.name == "alpha.txt").unwrap();
    assert_eq!(file_entry.entry_type, "file");
    assert_eq!(file_entry.path, "alpha.txt");
    assert_eq!(file_entry.size, Some(5));
    assert!(file_entry.mtime.is_some(), "文件应携带 mtime");
}

/// 子目录浏览：条目 path 为相对工作目录的完整相对路径。
#[tokio::test]
async fn tree_lists_subdir_with_rel_path() {
    let dir = tempdir().unwrap();
    let workdir = create_workdir(dir.path());
    let service = service_with_workdir(workdir);

    let resp: TreeResponse = service.tree("src", None).await.unwrap();

    assert_eq!(resp.path, "src");
    assert_eq!(resp.entries.len(), 1);
    assert_eq!(resp.entries[0].name, "nested");
    assert_eq!(resp.entries[0].path, "src/nested");
}

/// `..` 相对路径必须被拒绝（400）。
#[tokio::test]
async fn tree_rejects_dotdot_path() {
    let service = service_with_workdir(tempdir().unwrap().path().join("work"));

    let err = service.tree("..", None).await.unwrap_err();
    assert!(matches!(err, ApiError::BadRequest(_)), "err = {err:?}");
}

/// 含 `..` 组件的相对路径必须被拒绝（400）。
#[tokio::test]
async fn tree_rejects_parent_traversal() {
    let service = service_with_workdir(tempdir().unwrap().path().join("work"));

    let err = service.tree("../escape", None).await.unwrap_err();
    assert!(matches!(err, ApiError::BadRequest(_)), "err = {err:?}");
}

/// 未配置 working_directory → 400 且提示明确。
#[tokio::test]
async fn tree_missing_working_directory_returns_bad_request() {
    let service = service_without_workdir();

    let err = service.tree("", None).await.unwrap_err();
    assert!(
        matches!(&err, ApiError::BadRequest(msg) if msg.contains("working_directory")),
        "err = {err:?}"
    );
}

/// 不存在的相对路径 → 404。
#[tokio::test]
async fn tree_nonexistent_rel_returns_not_found() {
    let dir = tempdir().unwrap();
    let workdir = create_workdir(dir.path());
    let service = service_with_workdir(workdir);

    let err = service.tree("no-such-dir", None).await.unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "err = {err:?}");
}

/// 会话级工作区：带 session_id 时按会话绑定目录解析（覆盖全局默认）。
#[tokio::test]
async fn tree_resolves_session_bound_workdir() {
    let dir = tempdir().unwrap();
    let default_workdir = dir.path().join("default");
    let session_workdir = dir.path().join("session-ws");
    std::fs::create_dir_all(&default_workdir).unwrap();
    std::fs::create_dir_all(&session_workdir).unwrap();
    std::fs::write(session_workdir.join("only-in-session.txt"), "x").unwrap();

    let mut session = tianyan::session::Session::new("s1");
    session.header.working_directory = Some(session_workdir.to_string_lossy().into_owned());
    let manager = Arc::new(MockSessionManager::with_sessions(vec![session]));
    let service = WorkspaceService::new(manager, Some(default_workdir), None);

    let resp: TreeResponse = service.tree("", Some("s1")).await.unwrap();
    assert_eq!(
        resp.root,
        strip_verbatim(&session_workdir.canonicalize().unwrap().to_string_lossy()),
        "会话级绑定目录应覆盖全局默认"
    );
    assert_eq!(resp.entries.len(), 1);
    assert_eq!(resp.entries[0].name, "only-in-session.txt");
}

/// 会话级工作区：session_id 指向不存在的会话 → 404。
#[tokio::test]
async fn tree_session_not_found_returns_not_found() {
    let service = service_with_workdir(tempdir().unwrap().path().join("work"));

    let err = service.tree("", Some("ghost-session")).await.unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "err = {err:?}");
}

/// 会话级工作区：会话绑定目录已不存在 → 400 且提示明确。
#[tokio::test]
async fn tree_session_bound_workdir_missing_returns_bad_request() {
    let mut session = tianyan::session::Session::new("s1");
    session.header.working_directory = Some("Z:/gone-ws".to_string());
    let manager = Arc::new(MockSessionManager::with_sessions(vec![session]));
    let service = WorkspaceService::new(manager, None, None);

    let err = service.tree("", Some("s1")).await.unwrap_err();
    assert!(
        matches!(&err, ApiError::BadRequest(msg) if msg.contains("会话工作目录不存在")),
        "err = {err:?}"
    );
}

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

/// 文本文件读取：纯内容 + 总行数（内容匹配编辑：无行号/哈希前缀）。
#[tokio::test]
async fn read_returns_plain_content_for_text_file() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "alpha\nbeta\n").unwrap();
    let service = service_with_workdir(workdir.clone());

    let value: Value = service.read("a.txt", None, None, None).await.unwrap();

    assert_eq!(
        value["path"],
        strip_verbatim(
            &workdir
                .join("a.txt")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        )
    );
    let content = value["content"].as_str().unwrap();
    // 内容匹配编辑：read_file 输出纯内容（无行号/哈希前缀）
    assert_eq!(content, "alpha\nbeta");
    assert_eq!(value["total_lines"], 2);
    assert_eq!(value["truncated"], false);
}

/// 二进制文件 → binary: true。
#[tokio::test]
async fn read_marks_binary_file() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("blob.bin"), [0u8, 1u8, 2u8, 255u8]).unwrap();
    let service = service_with_workdir(workdir);

    let value: Value = service.read("blob.bin", None, None, None).await.unwrap();

    assert_eq!(value["binary"], true);
}

/// 不存在的文件 → 404。
#[tokio::test]
async fn read_missing_file_returns_not_found() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let service = service_with_workdir(workdir);

    let err = service
        .read("ghost.rs", None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "err = {err:?}");
}

// ---------------------------------------------------------------------------
// diff（快照模式）
// ---------------------------------------------------------------------------

/// 构造带快照管理器的服务（快照根在 workdir 之外），返回 (服务, 管理器)。
fn service_with_snapshot(
    workdir: PathBuf,
    snap_root: PathBuf,
) -> (WorkspaceService, Arc<SnapshotManager>) {
    let manager = Arc::new(SnapshotManager::new(snap_root, workdir.clone()));
    // 快照 diff 按会话解析工作目录：预置绑定到 workdir 的会话 s1
    let mut session = tianyan::session::Session::new("s1");
    session.header.working_directory = Some(workdir.to_string_lossy().into_owned());
    let service = WorkspaceService::new(
        Arc::new(MockSessionManager::with_sessions(vec![session])),
        Some(workdir),
        Some(manager.clone()),
    );
    (service, manager)
}

/// 捕获快照后修改文件 → diff 显示 modified，行数变化正确。
#[tokio::test]
async fn diff_snapshot_reports_modified_file() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "v1\n").unwrap();
    let snap_root = dir.path().join("snaproot");
    let (service, manager) = service_with_snapshot(workdir.clone(), snap_root);
    manager.capture("s1", "msg_1").await.unwrap();

    std::fs::write(workdir.join("a.txt"), "v1\nv2\n").unwrap();

    let value: Value = service
        .diff_snapshot("s1", Some("msg_1"), None, Some("a.txt"))
        .await
        .unwrap();
    assert_eq!(value["path"], "a.txt");
    assert_eq!(value["status"], "modified");
    assert_eq!(value["old_lines"], 1);
    assert_eq!(value["new_lines"], 2);
    let hunks = value["hunks"].as_array().unwrap();
    assert!(!hunks.is_empty(), "modified 文件应有变更块");
    assert!(value["unified"].as_str().unwrap().contains("@@"));
}

/// 快照索引缺失 → 404。
#[tokio::test]
async fn diff_snapshot_missing_index_returns_not_found() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "v1\n").unwrap();
    let snap_root = dir.path().join("snaproot");
    let (service, manager) = service_with_snapshot(workdir, snap_root);
    manager.capture("s1", "msg_1").await.unwrap();

    let err = service
        .diff_snapshot("s1", Some("msg_missing"), None, Some("a.txt"))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "err = {err:?}");
}

/// 省略 path → 返回整个 `{ "files": [...] }` 结构。
#[tokio::test]
async fn diff_snapshot_without_path_returns_files_list() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "v1\n").unwrap();
    let snap_root = dir.path().join("snaproot");
    let (service, manager) = service_with_snapshot(workdir.clone(), snap_root);
    manager.capture("s1", "msg_1").await.unwrap();

    std::fs::write(workdir.join("a.txt"), "v1\nv2\n").unwrap();

    let value: Value = service
        .diff_snapshot("s1", Some("msg_1"), None, None)
        .await
        .unwrap();
    let files = value["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "仅修改的文件应出现在列表中: {value}");
    assert_eq!(files[0]["path"], "a.txt");
    assert_eq!(files[0]["status"], "modified");
}

// ---------------------------------------------------------------------------
// diff（文件间模式）
// ---------------------------------------------------------------------------

/// 两个不同文件 → status modified，hunks 与 unified 非空。
#[tokio::test]
async fn diff_files_reports_modified_with_hunks() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(workdir.join("b.txt"), "one\ntwo\nfour\n").unwrap();
    let service = service_with_workdir(workdir);

    let value: Value = service.diff_files("a.txt", "b.txt", None).await.unwrap();

    assert_eq!(value["path"], "a.txt");
    assert_eq!(value["status"], "modified");
    assert!(
        !value["hunks"].as_array().unwrap().is_empty(),
        "内容不同应有变更块: {value}"
    );
    assert!(
        value["unified"].as_str().unwrap().contains("@@"),
        "unified 应包含块头: {value}"
    );
    assert_eq!(value["old_lines"], 3);
    assert_eq!(value["new_lines"], 3);
}

/// 两个相同文件 → status unchanged，hunks 为空。
#[tokio::test]
async fn diff_files_unchanged_for_identical_content() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(workdir.join("b.txt"), "one\ntwo\nthree\n").unwrap();
    let service = service_with_workdir(workdir);

    let value: Value = service.diff_files("a.txt", "b.txt", None).await.unwrap();

    assert_eq!(value["status"], "unchanged");
    assert!(value["hunks"].as_array().unwrap().is_empty());
    assert_eq!(value["old_lines"], 3);
    assert_eq!(value["new_lines"], 3);
}

/// 文件间对比中任一文件缺失 → 404。
#[tokio::test]
async fn diff_files_missing_path_returns_not_found() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "one\n").unwrap();
    let service = service_with_workdir(workdir);

    let err = service
        .diff_files("a.txt", "ghost.txt", None)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "err = {err:?}");
}

// ---------------------------------------------------------------------------
// 处理器层：oneshot 路由测试
// ---------------------------------------------------------------------------

/// tree 端点 → 200 + 约定 JSON 结构（完整 create_app 状态）。
#[tokio::test]
async fn tree_endpoint_returns_200_with_expected_shape() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("main.rs"), "fn main() {}\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/workspace/tree?path=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["root"],
        strip_verbatim(&workdir.canonicalize().unwrap().to_string_lossy())
    );
    assert_eq!(body["path"], "");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "main.rs");
    assert_eq!(entries[0]["type"], "file");
    assert!(entries[0]["size"].is_number());
    assert!(entries[0]["mtime"].is_number());
}

/// tree 端点的 `..` 请求 → 400。
#[tokio::test]
async fn tree_endpoint_rejects_dotdot_with_400() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/workspace/tree?path=..")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"].is_string());
}

/// read 端点 → 200 + 纯内容（端到端验证 core 委托链路）。
#[tokio::test]
async fn read_endpoint_returns_plain_content() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "alpha\nbeta\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/workspace/read?path=a.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["content"].as_str().unwrap(), "alpha\nbeta");
    assert_eq!(body["total_lines"], 2);
}

/// 未配置 working_directory 时 tree 端点 → 400。
#[tokio::test]
async fn tree_endpoint_missing_working_directory_returns_400() {
    let dir = tempdir().unwrap();
    let config = test_config(dir.path(), None);
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/workspace/tree?path=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("working_directory"));
}

// ---------------------------------------------------------------------------
// 处理器层：apply-patch / apply-edit（编程工作台 Phase 2）
// ---------------------------------------------------------------------------

/// 构造 JSON POST 请求。
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// apply-patch 成功路径：补丁落盘 + 响应 total_files=1 与变更摘要。
#[tokio::test]
async fn apply_patch_endpoint_applies_patch_and_updates_file() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let patch = "*** Update File: a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+two2\n three\n";
    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-patch",
            json!({ "patch": patch }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["total_files"], 1);
    let files = body["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "a.txt");
    assert_eq!(files[0]["hunks_applied"], 1);
    assert_eq!(files[0]["lines_changed"], 2);
    let disk = std::fs::read_to_string(workdir.join("a.txt")).unwrap();
    assert_eq!(disk, "one\ntwo2\nthree\n");
}

/// apply-patch 拒绝 `..` 路径（core 沙箱违规）→ 400。
#[tokio::test]
async fn apply_patch_endpoint_rejects_dotdot_path() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let patch = "*** Update File: ../escape.txt\n@@ -1,1 +1,1 @@\n-a\n+b\n";
    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-patch",
            json!({ "patch": patch }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("非法路径"));
}

/// apply-patch 拒绝**工作目录外**的绝对路径目标 → 403。
///
/// core 解析接受绝对路径（对齐 apply_edit / read_file，ADR-009 补记）——边界
/// 因此由本 API 显式校验（`ensure_patch_within_base`）；本测试锁定该防线。
#[tokio::test]
async fn apply_patch_endpoint_rejects_absolute_outside_workdir() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let outside = dir.path().join("outside.txt");
    std::fs::write(&outside, "safe\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-safe\n+pwned\n",
        outside.to_string_lossy()
    );
    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-patch",
            json!({ "patch": patch }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "safe\n",
        "越界目标不得被改写"
    );
}

/// apply-edit 成功路径：内容匹配定位 + 落盘 + 响应 edits_applied=1。
#[tokio::test]
async fn apply_edit_endpoint_applies_content_edit() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "alpha\nbeta\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-edit",
            json!({
                "path": "a.txt",
                "edits": [{ "old_string": "beta", "new_string": "BETA" }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["path"], "a.txt");
    assert_eq!(body["edits_applied"], 1);
    let disk = std::fs::read_to_string(workdir.join("a.txt")).unwrap();
    assert_eq!(disk, "alpha\nBETA\n");
}

/// apply-edit old_string 未找到（内容漂移）→ 409（前端提示"文件已被修改"）。
#[tokio::test]
async fn apply_edit_endpoint_not_found_returns_409() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(workdir.join("a.txt"), "alpha\nbeta\n").unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-edit",
            json!({
                "path": "a.txt",
                "edits": [{ "old_string": "完全不存在的原文", "new_string": "x" }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("未找到"));
}

/// apply-patch 接受 git 风格补丁（jsdiff `createTwoFilesPatch` 产物）：
/// 服务端归一化为 codex 信封后落盘。
#[tokio::test]
async fn apply_patch_endpoint_accepts_git_style_patch() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(workdir.join("src")).unwrap();
    std::fs::write(
        workdir.join("src/lib.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    // jsdiff createTwoFilesPatch("a/src/lib.rs", "b/src/lib.rs", old, new) 的输出形状
    let patch = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"hello\");\n+    println!(\"hello world\");\n }\n";
    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-patch",
            json!({ "patch": patch }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["total_files"], 1);
    assert_eq!(body["files"][0]["path"], "src/lib.rs");
    let disk = std::fs::read_to_string(workdir.join("src/lib.rs")).unwrap();
    assert_eq!(disk, "fn main() {\n    println!(\"hello world\");\n}\n");
}

/// apply-patch 拒绝既非 git 也非 codex 的补丁文本 → 400。
#[tokio::test]
async fn apply_patch_endpoint_rejects_unknown_format() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-patch",
            json!({ "patch": "这不是一个补丁" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("补丁格式无效"));
}

/// apply-edit 目标文件不存在 → 404（resolve 沙箱）。
#[tokio::test]
async fn apply_edit_endpoint_missing_file_returns_404() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(post_json(
            "/api/v1/workspace/apply-edit",
            json!({
                "path": "ghost.rs",
                "edits": [{ "old_string": "a", "new_string": "x" }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["success"], false);
}

// ---------------------------------------------------------------------------
// dirs（目录选择器）
// ---------------------------------------------------------------------------

/// 服务层：列出指定目录的子目录（只列目录，不列文件）。
#[tokio::test]
async fn dirs_lists_subdirectories_only() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(workdir.join("sub-a")).unwrap();
    std::fs::create_dir_all(workdir.join("sub-b")).unwrap();
    std::fs::write(workdir.join("file.txt"), "x").unwrap();
    let service = service_with_workdir(workdir.clone());

    let resp = service
        .list_dirs(Some(workdir.to_string_lossy().as_ref()))
        .await
        .unwrap();
    // 只列目录：sub-a、sub-b；file.txt 不出现
    let names: Vec<&str> = resp.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["sub-a", "sub-b"]);
    assert!(resp.parent.is_some());
    // 条目路径为绝对路径（可继续传入 path 参数）
    assert!(resp.entries[0].path.starts_with(&strip_verbatim(
        &workdir.canonicalize().unwrap().to_string_lossy()
    )));
}

/// 服务层：不存在的目录 → 404。
#[tokio::test]
async fn dirs_missing_dir_returns_404() {
    let service = service_with_workdir(tempdir().unwrap().path().join("work"));
    let err = service
        .list_dirs(Some("Z:/definitely-not-exists-tianyan-test"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApiError::NotFound(_)),
        "应返回 NotFound: {err:?}"
    );
}

/// 端点层：GET /workspace/dirs 返回子目录；缺省 path 时（浏览根）不报错。
#[tokio::test]
async fn dirs_endpoint_lists_subdirs() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(workdir.join("src")).unwrap();
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let uri = format!("/api/v1/workspace/dirs?path={}", workdir.display());
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["src"]);
}

/// 端点层：GET /workspace/dirs（无 path）返回浏览根（Windows 盘符/家目录），HTTP 200。
#[tokio::test]
async fn dirs_endpoint_root_without_path_ok() {
    let dir = tempdir().unwrap();
    let workdir = dir.path().join("work");
    let config = test_config(dir.path(), Some(&workdir));
    let (app, _state) = crate::create_app(config).await.unwrap();

    let response = app
        .oneshot(
            Request::get("/api/v1/workspace/dirs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
