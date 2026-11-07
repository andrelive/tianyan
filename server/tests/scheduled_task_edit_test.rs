//! 定时任务**编辑**（`PUT /api/v1/scheduled-tasks/{id}`）端到端测试。
//!
//! 为什么必须走**真服务**而不是只测 `manager` 的纯函数：编辑的关键行为有一半在
//! 装配层——注销旧注册、用新 handler 重注册、把「已结算时刻点」带回调度器。
//! `create_app_with_assembly`（router 级 e2e）不启动调度器与定时任务管理器，那条
//! 路径上 POST/PUT 只会返回 503，验不到任何东西。
//!
//! 断言的分工：
//! - **HTTP 契约**：创建 → 部分字段更新 → 停用 → 错误映射（404 / 400）；
//! - **语义**：只改指令不得动调度规格与历史；停用后 `next_run_at` 清空；
//! - **落盘**：`scheduled_agent_tasks.json` 里是新值，且 `interval_secs` 双写仍在
//!   （旧版本读新文件仍能跑——可回退）。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::factory::{test_assembly, test_tianyan_config_with_data_dir};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn scheduled_task_edit_roundtrip_over_http() {
    let data_dir = tempdir().expect("创建临时数据目录");
    let config = test_tianyan_config_with_data_dir(data_dir.path().to_path_buf());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定临时端口");
    let port = listener.local_addr().expect("读取端口").port();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(
        tianyan_server::start_server_with_shutdown_signal_and_assembly(
            listener,
            config,
            shutdown_rx,
            test_assembly(),
        ),
    );

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..100 {
        if client
            .get(format!("{base}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "服务器应在 10 秒内就绪");

    // 一个**不会被误触发**的定点时刻（当前 +2 小时）：「错过就跳过」保证它不会
    // 在测试期间真的调起模型（否则测试会依赖 mock provider 的行为与耗时）。
    let at = (chrono::Local::now() + chrono::Duration::hours(2))
        .format("%H:%M")
        .to_string();

    // ---- 创建 ----
    let resp = client
        .post(format!("{base}/api/v1/scheduled-tasks"))
        .json(&json!({
            "name": "收盘复盘",
            "workspace": data_dir.path().to_string_lossy(),
            "prompt": "第一版指令",
            "schedule": { "kind": "daily", "at": at },
            "missed": { "kind": "skip" },
        }))
        .send()
        .await
        .expect("POST 失败");
    assert_eq!(
        resp.status(),
        200,
        "创建应成功（503 = 定时任务管理器未装配，说明服务未启动调度器）"
    );
    let created: serde_json::Value = resp.json().await.expect("解析创建响应");
    let id = created["id"].as_str().expect("响应应含任务 id").to_string();
    assert_eq!(created["prompt"], "第一版指令");
    assert_eq!(created["schedule"]["kind"], "daily");
    assert_eq!(created["schedule"]["at"], at);
    assert!(created["last_run_at"].is_null(), "新任务没有执行历史");

    // ---- 只改指令：调度规格 / 错过策略 / 历史都必须原样保留 ----
    let resp = client
        .put(format!("{base}/api/v1/scheduled-tasks/{id}"))
        .json(&json!({ "prompt": "第二版指令" }))
        .send()
        .await
        .expect("PUT 失败");
    assert_eq!(resp.status(), 200, "部分字段更新应成功");
    let updated: serde_json::Value = resp.json().await.expect("解析更新响应");
    assert_eq!(updated["prompt"], "第二版指令");
    assert_eq!(
        updated["schedule"]["kind"], "daily",
        "没传的调度规格不能被清掉（部分字段语义）"
    );
    assert_eq!(updated["schedule"]["at"], at);
    assert_eq!(updated["missed"]["kind"], "skip", "错过策略同理");
    assert_eq!(updated["name"], "收盘复盘", "名称同理");
    assert!(
        updated["last_run_at"].is_null(),
        "编辑不改写真实历史（它记录的是「上次真的跑过」）"
    );

    // ---- 停用：定义保留、下次时间清空 ----
    let resp = client
        .put(format!("{base}/api/v1/scheduled-tasks/{id}"))
        .json(&json!({ "enabled": false }))
        .send()
        .await
        .expect("PUT 失败");
    assert_eq!(resp.status(), 200);
    let stopped: serde_json::Value = resp.json().await.expect("解析停用响应");
    assert_eq!(stopped["enabled"], false);
    assert!(stopped["next_run_at"].is_null(), "停用后不应再有下次时间");
    assert_eq!(stopped["prompt"], "第二版指令", "停用不动定义");

    // ---- 错误映射：不存在 → 404；间隔过短 → 400 ----
    let resp = client
        .put(format!(
            "{base}/api/v1/scheduled-tasks/sched-task-does-not-exist"
        ))
        .json(&json!({ "prompt": "x" }))
        .send()
        .await
        .expect("PUT 失败");
    assert_eq!(resp.status(), 404, "不存在的任务应 404");

    let resp = client
        .put(format!("{base}/api/v1/scheduled-tasks/{id}"))
        .json(&json!({ "interval_secs": 10 }))
        .send()
        .await
        .expect("PUT 失败");
    assert_eq!(resp.status(), 400, "间隔下限 60 秒应 400");

    // ---- 列表：编辑后的值（且只有这一条） ----
    let list: Vec<serde_json::Value> = client
        .get(format!("{base}/api/v1/scheduled-tasks"))
        .send()
        .await
        .expect("GET 失败")
        .json()
        .await
        .expect("解析列表");
    assert_eq!(list.len(), 1, "只应有一条任务：{list:?}");
    assert_eq!(list[0]["id"], id);
    assert_eq!(list[0]["prompt"], "第二版指令");
    assert_eq!(list[0]["enabled"], false);

    // ---- 落盘：新值 + 可回退的 interval_secs 双写 ----
    let raw = std::fs::read_to_string(data_dir.path().join("scheduled_agent_tasks.json"))
        .expect("任务定义应已落盘");
    assert!(raw.contains("第二版指令"), "磁盘上应是编辑后的指令：{raw}");
    assert!(
        raw.contains("\"interval_secs\": 86400"),
        "日历类任务必须双写等价间隔（旧版本读新文件仍能跑）：{raw}"
    );

    shutdown_tx.send(true).expect("发送关闭信号");
    let joined = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("优雅关停超时");
    assert!(joined.is_ok(), "服务器任务 join 失败");
}
