//! 端到端系统测试 — 驱动真实 `create_app()`：完整启动 → API 调用 → 验证流程

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use tempfile::tempdir;

use common::factory::test_tianyan_config_with_data_dir;
use common::harness::TestServer;

/// 使用独立临时数据目录构建真实应用路由（完整 VFS + AppState 初始化）。
async fn start_real_server() -> (TestServer, tempfile::TempDir) {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());

    let (router, _state) = tianyan_server::create_app(config)
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");
    (server, dir)
}

#[tokio::test]
async fn test_e2e_health_and_config() {
    let (server, _dir) = start_real_server().await;

    // 健康检查（真实 handler：status + version）
    let resp = server.get("/health").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());

    // 配置端点返回有效 JSON（真实 handler：完整 TianyanConfig 序列化）
    let resp = server.get("/api/v1/config").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("config").is_some(), "响应应包含 config 字段");

    // 模型服务列表（真实 handler：providers/models/preferences）
    let resp = server.get("/api/v1/config/models").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let providers = body["providers"].as_array().expect("providers 应为数组");
    assert!(!providers.is_empty());
    assert_eq!(providers[0]["name"], "mock-service");

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_sessions_endpoint_roundtrip() {
    let (server, _dir) = start_real_server().await;

    // 会话列表（真实 handler：读取 VFS 持久化会话）
    let resp = server.get("/api/v1/sessions").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sessions = body["sessions"].as_array().expect("sessions 应为数组");
    assert!(sessions.is_empty(), "新数据目录应无会话: {body}");

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_memory_browse_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 记忆浏览（真实 handler：VFS memory 命名空间读取）
    let resp = server.get("/api/v1/memory").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let memories = body["memories"].as_array().expect("memories 应为数组");
    assert!(memories.is_empty(), "新数据目录应无记忆: {body}");
    assert_eq!(body["total"], 0);

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_stats_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 统计摘要（真实 handler：UsageStats 查询，空库也应返回结构完整 JSON）
    let resp = server.get("/api/v1/stats").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.is_object() && !body.as_object().unwrap().is_empty(),
        "stats 应返回统计对象: {body}"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_retrieval_traces_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 检索轨迹（真实 handler：空库返回空数组 + limit 参数生效）
    let resp = server.get("/api/v1/retrieval/traces").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["total"], 0, "新数据目录应无轨迹: {body}");
    assert_eq!(body["traces"].as_array().unwrap().len(), 0);

    // limit 上限收敛
    let resp = server.get("/api/v1/retrieval/traces?limit=5000").await;
    assert_eq!(resp.status(), 200);

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_scheduler_status_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 调度器状态（无 Provider 环境未装配 → 空状态）
    let resp = server.get("/api/v1/scheduler/status").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["running"], false,
        "无 Provider 环境调度器应未运行: {body}"
    );
    assert_eq!(body["tasks"].as_array().unwrap().len(), 0);

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_approval_status_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 审批状态（真实 handler：Agent 审批链路，结构完整 JSON）
    let resp = server.get("/api/v1/approval/status").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // 配置节必须存在且含关键字段
    assert!(
        body["config"]["enable_auto_approval"].is_boolean(),
        "审批配置应包含 enable_auto_approval: {body}"
    );
    assert!(body["config"]["unattended_mode"].is_boolean());
    assert!(body["config"]["wait_for_approval"].is_boolean());
    // 空队列：pending_approvals / pending_confirmations / recent_records
    assert_eq!(body["pending_approvals"].as_array().unwrap().len(), 0);
    assert_eq!(body["pending_confirmations"].as_array().unwrap().len(), 0);
    assert_eq!(body["recent_records"].as_array().unwrap().len(), 0);
    assert_eq!(body["confirmed_action_count"], 0);

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_approval_respond_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 无效决策 → 400
    let resp = server
        .post(
            "/api/v1/approval/respond",
            &serde_json::json!({ "request_id": "req-x", "decision": "maybe" }),
        )
        .await;
    assert_eq!(resp.status(), 400, "无效决策应返回 400");

    // 不存在的请求 → 404（核心层"审批请求不存在或已超时"）
    let resp = server
        .post(
            "/api/v1/approval/respond",
            &serde_json::json!({ "request_id": "req-not-exist", "decision": "approve" }),
        )
        .await;
    assert_eq!(resp.status(), 404, "不存在的审批请求应返回 404");

    server.shutdown();
}

// ========== 核心链路补充（无 LLM 依赖的确定性路径） ==========

#[tokio::test]
async fn test_e2e_sessions_error_paths() {
    let (server, _dir) = start_real_server().await;

    // 不存在的会话 → 404（详情 / 消息 / 删除）
    assert_eq!(
        server.get("/api/v1/sessions/missing-001").await.status(),
        404
    );
    assert_eq!(
        server
            .get("/api/v1/sessions/missing-001/messages")
            .await
            .status(),
        404
    );
    let resp = server
        .client()
        .delete(format!("{}/api/v1/sessions/missing-001", server.base_url()))
        .send()
        .await
        .expect("DELETE 请求失败");
    assert_eq!(resp.status(), 404, "删除不存在的会话应返回 404");

    // 消息操作（会话不存在 → 404）
    let resp = server
        .post(
            "/api/v1/sessions/missing-001/messages/delete",
            &serde_json::json!({ "message_index": 0 }),
        )
        .await;
    assert_eq!(resp.status(), 404, "删除消息：会话不存在应 404");

    let resp = server
        .post(
            "/api/v1/sessions/missing-001/messages/redo",
            &serde_json::json!({ "message_index": 0 }),
        )
        .await;
    assert_eq!(resp.status(), 404, "重做消息：会话不存在应 404");

    let resp = server
        .post(
            "/api/v1/sessions/missing-001/title",
            &serde_json::json!({ "title": "新标题" }),
        )
        .await;
    assert_eq!(resp.status(), 404, "更新标题：会话不存在应 404");

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_skills_endpoints() {
    let (server, _dir) = start_real_server().await;

    // 技能列表（registry 装配，无 LLM 依赖）
    let resp = server.get("/api/v1/skills").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["skills"].is_array(), "skills 应为数组: {body}");

    // 不存在的技能 → 同步执行器语义：200 + success=false
    let resp = server
        .post(
            "/api/v1/skills/not-a-real-skill/execute",
            &serde_json::json!({ "parameters": {} }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["success"], false,
        "不存在的技能应返回 success=false: {body}"
    );

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_knowledge_read_paths() {
    let (server, _dir) = start_real_server().await;

    // 空库条目列表（VFS 命名空间浏览，无 LLM 依赖）
    let resp = server.get("/api/v1/knowledge/entries").await;
    assert_eq!(resp.status(), 200, "空知识库条目列表应 200");

    // search 缺 q → 400（参数校验）
    let resp = server.get("/api/v1/knowledge/search").await;
    assert_eq!(resp.status(), 400, "search 缺 q 应 400");

    // entries/read 缺 uri → 400（参数校验）
    let resp = server.get("/api/v1/knowledge/entries/read").await;
    assert_eq!(resp.status(), 400, "entries/read 缺 uri 应 400");

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_config_misc_endpoints() {
    let (server, _dir) = start_real_server().await;

    // 配置状态
    let resp = server.get("/api/v1/config/status").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.get("configured").is_some(),
        "config/status 应含 configured 字段: {body}"
    );

    // soul（当前配置 + 默认人格）
    assert_eq!(server.get("/api/v1/config/soul").await.status(), 200);
    assert_eq!(
        server.get("/api/v1/config/soul/default").await.status(),
        200
    );

    // MCP 服务器列表
    let resp = server.get("/api/v1/config/mcp/servers").await;
    assert_eq!(resp.status(), 200);

    // 未知配置节 → 200 + null（节不存在语义）
    let resp = server.get("/api/v1/config/unknown-section").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_null(), "未知配置节应返回 null: {body}");

    server.shutdown();
}

#[tokio::test]
async fn test_e2e_knowledge_delete_entry_endpoint() {
    let (server, _dir) = start_real_server().await;

    // 空 URI → 400
    let resp = server
        .post(
            "/api/v1/knowledge/entries/delete",
            &serde_json::json!({ "uri": "   " }),
        )
        .await;
    assert_eq!(resp.status(), 400, "空 URI 应 400");

    // 非 knowledge 命名空间 → 400（越权保护）
    let resp = server
        .post(
            "/api/v1/knowledge/entries/delete",
            &serde_json::json!({ "uri": "tianyan://memory/facts/user_name" }),
        )
        .await;
    assert_eq!(resp.status(), 400, "非知识库命名空间应 400");

    // 无效 URI 格式 → 400
    let resp = server
        .post(
            "/api/v1/knowledge/entries/delete",
            &serde_json::json!({ "uri": "not-a-uri" }),
        )
        .await;
    assert_eq!(resp.status(), 400, "无效 URI 应 400");

    // 不存在的条目 → 404
    let resp = server
        .post(
            "/api/v1/knowledge/entries/delete",
            &serde_json::json!({ "uri": "tianyan://knowledge/nonexistent-doc" }),
        )
        .await;
    assert_eq!(resp.status(), 404, "不存在的知识条目应 404");

    server.shutdown();
}
