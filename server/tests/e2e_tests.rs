//! 端到端系统测试 — 驱动真实 `create_app()`：完整启动 → API 调用 → 验证流程

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use tempfile::tempdir;

use common::factory::{test_assembly, test_tianyan_config_with_data_dir};
use common::harness::TestServer;

/// 使用独立临时数据目录构建真实应用路由（完整 VFS + AppState 初始化）。
async fn start_real_server() -> (TestServer, tempfile::TempDir) {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());

    let (router, _state) = tianyan_server::create_app_with_assembly(config, test_assembly())
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
    assert_eq!(
        body["executing_task_id"],
        serde_json::Value::Null,
        "空闲时应无执行中任务: {body}"
    );

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
    // ADR-033：审批行为模式（autonomous / confirm / interactive）
    assert!(
        body["config"]["mode"].is_string(),
        "审批配置应包含 mode: {body}"
    );
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
            &serde_json::json!({ "message_id": "msg_missing" }),
        )
        .await;
    assert_eq!(resp.status(), 404, "删除消息：会话不存在应 404");

    let resp = server
        .post(
            "/api/v1/sessions/missing-001/messages/redo",
            &serde_json::json!({ "message_id": "msg_missing" }),
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

    // Provider 预置列表（ADR-046）→ 200 + 非空列表；路径须与前端一致
    // （前端调用 /config/provider-presets；路径写错会被 /config/{section} 吞成 null）
    let resp = server.get("/api/v1/config/provider-presets").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        !body.is_null(),
        "预置列表不得为 null（路径被 /config/{{section}} 吞掉的症状）: {body}"
    );
    let presets = body["presets"].as_array().expect("响应应含 presets 数组");
    assert!(!presets.is_empty(), "内置预置表不得为空: {body}");
    assert!(
        presets.iter().any(|p| p["id"] == "opencode-go"),
        "预置列表应含 opencode-go: {body}"
    );

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

/// System 通知实时推送——落库后补发 chat_stream 边界事件。
///
/// ADR-031 移除"落库即广播"（消息不再广播）；System 通知（后台任务/命令
/// 完成、定时提醒）作为输入类消息的例外：落库后补发 chat_stream 事件
/// （chunk_type=message），前端 applyServerMessage 按 id 查重追加。
/// 验证：订阅 /events → 经 state.session_manager 落库一条 System 消息 →
/// SSE 流收到 {type: chat_stream, chunk_type: message, session_id, message}。
#[tokio::test]
async fn test_e2e_unified_event_push_message() {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());
    let (router, state) = tianyan_server::create_app_with_assembly(config, test_assembly())
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");

    // 订阅统一事件流（SSE）
    let client = reqwest::Client::new();
    let mut resp = client
        .get(format!("{}/api/v1/events", server.base_url()))
        .send()
        .await
        .expect("订阅 /events 失败");
    assert_eq!(resp.status(), 200, "/events 应 200");

    // ADR-039：经工作集落库（唯一写入口）——边界消息（System）落库后由工作集
    // 推送回调补发 chat_stream 边界事件（不再经 SessionManager 直写）
    let session_id = "session-evt-1";
    state
        .session_manager()
        .create_session(session_id, tianyan::common::types::Message::user("你好"))
        .await
        .expect("创建会话失败");
    state
        .working_sets()
        .ensure(session_id)
        .await
        .expect("工作集加载失败")
        .append(
            &tianyan::common::types::StructuredMessage::system(
                session_id.to_string(),
                "[后台命令完成] ping（cmd_0）".to_string(),
            ),
            true,
        )
        .await
        .expect("落库失败");

    // 读取 SSE 流，等待 chat_stream 事件（超时 5s）；累积文本逐行解析
    // `data: {...}` 行，捕获完整事件体做结构断言。
    let mut sse_text = String::new();
    let mut parsed: Option<serde_json::Value> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && parsed.is_none() {
        let chunk = resp.chunk().await.expect("读 SSE 失败");
        if let Some(bytes) = chunk {
            sse_text.push_str(&String::from_utf8_lossy(&bytes));
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        for line in sse_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if v["type"] == "chat_stream" && v["chunk_type"] == "message" {
                        parsed = Some(v);
                        break;
                    }
                }
            }
        }
    }
    let ev = parsed.expect("SSE 应收到 chat_stream 事件（System 通知落库即推送）");
    // 事件契约：{type: chat_stream, chunk_type: message, session_id, message}
    assert_eq!(ev["type"], "chat_stream");
    assert_eq!(ev["chunk_type"], "message");
    assert_eq!(ev["session_id"].as_str(), Some(session_id));

    // 关键：message 必须是 ChatMessage（API 展示格式）——StructuredMessage
    // （存储格式，parts 数组）会让前端渲染崩溃（content/segments 缺失）。
    // 响应方向 content 为 None（正文在 segments 的 Text 段，单一事实源）。
    let msg = &ev["message"];
    assert_eq!(msg["role"], "system", "通知应为 system 角色：{msg}");
    assert!(msg["id"].is_string(), "message 应含 id：{msg}");
    assert!(
        msg.get("parts").is_none(),
        "不应是 StructuredMessage 格式（含 parts 字段）：{msg}"
    );
    let segments = msg["segments"].as_array().expect("segments 应为数组");
    assert_eq!(segments[0]["type"], "text", "首段应为 Text：{msg}");
    assert!(
        segments[0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("[后台命令完成]"),
        "Text 段应为通知文本：{msg}"
    );

    // 落库后 last_seq 正确（通知已持久化）
    let last_seq = state
        .session_store()
        .last_seq(session_id)
        .await
        .expect("last_seq 查询失败");
    assert!(last_seq >= 1, "落库后 last_seq 应 >= 1: {last_seq}");

    server.shutdown();
}

/// 消息端点分页语义（ADR-035 §8 收尾）：**无参 = 最近一页 + 上滚游标**
/// （不再返回全量——大会话全量可达数十 MB，实测 1650 条消息 25 MB，
/// 是会话切换卡顿的根源）；带 `limit` 取显式页大小。
#[tokio::test]
async fn test_e2e_messages_default_is_latest_page() {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());
    let (router, state) = tianyan_server::create_app_with_assembly(config, test_assembly())
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");

    // 建会话并落 120 条消息（远超一页 50 条；user/assistant 交替）
    let session_id = "session-page-1";
    state
        .session_manager()
        .create_session(session_id, tianyan::common::types::Message::user("你好"))
        .await
        .expect("创建会话失败");
    let ws = state
        .working_sets()
        .ensure(session_id)
        .await
        .expect("工作集加载失败");
    for i in 0..120 {
        let text = format!("消息 {i}");
        let msg = if i % 2 == 0 {
            tianyan::common::types::StructuredMessage::user(session_id.to_string(), text)
        } else {
            tianyan::common::types::StructuredMessage::assistant(session_id.to_string(), text)
        };
        ws.append(&msg, false).await.expect("落库失败");
    }

    // 无参：最近一页（≤50 条）+ has_more + 上滚游标——全量口子回归即失败
    let resp = server
        .get(&format!("/api/v1/sessions/{session_id}/messages"))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let msgs = body["messages"].as_array().expect("messages 应为数组");
    assert!(
        msgs.len() <= 50,
        "无参应返回最近一页（≤50 条），实际 {} 条（全量口子回归？）: {body}",
        msgs.len()
    );
    assert!(!msgs.is_empty(), "有 120 条消息的会话不应返回空页: {body}");
    assert_eq!(
        msgs.first().map(|m| m["role"].as_str().unwrap_or("")),
        Some("user"),
        "页首恒为 user（页边界对齐，工具对不跨页切断）: {body}"
    );
    assert_eq!(
        body["has_more"].as_bool(),
        Some(true),
        "更早历史存在时 has_more 应为 true: {body}"
    );
    assert!(
        body["next_before_seq"].as_i64().unwrap_or(0) > 0,
        "分页应带上滚游标 next_before_seq: {body}"
    );

    // 显式 limit：按页大小取（页边界对齐 user，页长 ≤ limit）
    let resp = server
        .get(&format!("/api/v1/sessions/{session_id}/messages?limit=10"))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let msgs = body["messages"].as_array().expect("messages 应为数组");
    assert!(
        msgs.len() <= 10,
        "limit=10 页长应 ≤10，实际 {}: {body}",
        msgs.len()
    );

    server.shutdown();
}

// ─────────────────────────────────────────────────────────────────────────────
// SPA 静态回退与 API 404（修复：前端子路由刷新 404 白屏 / API 未知路径被 SPA 吞掉）
// ─────────────────────────────────────────────────────────────────────────────

/// SPA 子路由直接访问 / 刷新必须返回外壳 HTML（history 路由语义）。
///
/// 判别力：删掉 `static_service` 的 `not_found_service(ServeFile::new(index.html))`
/// 后，本用例立刻变红（`ServeDir` 对 `/chat` 无对应文件 → 404）。
#[tokio::test]
async fn test_e2e_spa_subroute_serves_index_html() {
    // 静态目录用绝对路径显式指定（测试进程 cwd 是 crate 目录，靠 get_static_dir 的
    // 猜测列表虽然能命中 `../gui-vite/dist`，但那是隐式的；显式设定让本用例与 cwd 解耦）。
    // 用 `parent()` 拼出**不含 `..` 的绝对路径**（不能用 canonicalize：Windows 上会变成
    // `\\?\F:\…` UNC 前缀，`ServeFile` 的路径校验同样会拒）。
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR 应有父目录")
        .join("gui-vite/dist");
    assert!(
        dist.join("index.html").exists(),
        "本用例需要已构建的前端产物：{}/index.html 不存在（先执行 npm run build）",
        dist.display()
    );
    std::env::set_var("TIANYAN_STATIC_DIR", &dist);

    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());
    let (router, _state) = tianyan_server::create_app_with_assembly(config, test_assembly())
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");

    for path in ["/chat", "/settings", "/chat/session-does-not-exist"] {
        let resp = server.get(path).await;
        assert_eq!(resp.status(), 200, "{path} 应回 SPA 外壳（不是 404）");
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("id=\"root\""),
            "{path} 应返回含 #root 的外壳 HTML，实际：{}",
            &body[..body.len().min(200)]
        );
    }

    // 反向对照：真实存在的静态资源仍按 200 返回（fallback 不劫持正常路径）
    let css = server.get("/assets/index-CYF_pX8c.css").await;
    assert_eq!(css.status(), 200, "既有静态资源不应被 SPA fallback 影响");

    server.shutdown();
}

/// 未知 `/api/v1/**` 必须 404 **JSON**，不能被 SPA fallback 变成 index.html。
///
/// 判别力：删掉 `api_router` 的 `api_not_found` 兜底后，本用例会拿到 200 + HTML
/// （前端 `resp.json()` 报 `Unexpected token <`，把「路径写错」伪装成解析错误）。
#[tokio::test]
async fn test_e2e_unknown_api_returns_404_json_not_html() {
    let dir = tempdir().expect("创建临时目录失败");
    let config = test_tianyan_config_with_data_dir(dir.path().to_path_buf());
    let (router, _state) = tianyan_server::create_app_with_assembly(config, test_assembly())
        .await
        .expect("create_app 失败");
    let server = TestServer::start(router).await.expect("启动测试服务器失败");

    let resp = server.get("/api/v1/definitely-not-a-route").await;
    assert_eq!(resp.status(), 404, "未知 API 端点应 404");
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("application/json"),
        "未知 API 端点应回 JSON（实际 Content-Type: {content_type}）"
    );
    let body: serde_json::Value = resp.json().await.expect("应为可解析 JSON");
    assert_eq!(body["error"], "not_found", "错误类别应为 not_found: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("/api/v1/definitely-not-a-route"),
        "错误消息应含请求路径（便于定位）: {body}"
    );

    server.shutdown();
}
