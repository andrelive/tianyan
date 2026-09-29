//! session_recall 工具执行器测试（输出治理：默认值 / clamp / 总预算截断）。
//!
//! 装置：内存库 + 真实会话消息（走 FTS 索引）+ SessionRecall + ToolRegistry。

use crate::common::types::{MessageRole, StructuredMessage};
use crate::config::SafetyMode;
use crate::db::Database;
use crate::executor::SecurityPolicy;

use super::*;

/// 默认严格策略（允许任意路径；回忆工具不涉路径规则）。
fn default_strict_policy() -> SecurityPolicy {
    SecurityPolicy {
        safety_mode: SafetyMode::Strict,
        trash_directory: std::env::temp_dir(),
        allowed_commands: None,
        blocked_commands: Vec::new(),
        allowed_directories: Vec::new(),
        blocked_directories: Vec::new(),
        allow_file_write: true,
        max_command_timeout_secs: 30,
        max_file_size: 1024 * 1024,
        block_interpreters: true,
    }
}

/// 建内存库 + 会话（消息经真实 FTS 索引）+ 装配 session_recall 的 registry。
async fn make_registry_with_recall(
    session_id: &str,
    texts: &[(MessageRole, String)],
) -> ToolRegistry {
    let db = Database::open_in_memory().unwrap();
    db.init_schemas().await.unwrap();
    let store = crate::session::store::SessionStore::new(db.clone()).unwrap();
    store
        .create(session_id, &crate::session::types::SessionHeader::default())
        .await
        .unwrap();
    for (i, (role, text)) in texts.iter().enumerate() {
        let mut msg = match role {
            MessageRole::Assistant => StructuredMessage::assistant(session_id, text.clone()),
            _ => StructuredMessage::user(session_id, text.clone()),
        };
        msg.id = format!("m{i}");
        store.append_message(session_id, &msg).await.unwrap();
    }
    let recall = crate::session::search::SessionRecall::new(db).unwrap();
    ToolRegistry::new(default_strict_policy()).with_session_recall(recall)
}

#[tokio::test]
async fn test_session_recall_defaults_are_3_hits_radius2() {
    // 输出治理（2026-09-29）：命中数默认 3、窗口半径默认 2（窗口 ≤ 5 条消息）。
    // 判别力：旧默认（limit 5 / radius 5）下 count=5、窗口可达 11 条——本测试必红。
    let mut msgs = Vec::new();
    for i in 0..12 {
        msgs.push((MessageRole::User, format!("第 {i} 条消息，关键词菠萝蜜")));
    }
    let registry = make_registry_with_recall("s1", &msgs).await;
    let out = registry
        .execute_session_recall(r#"{"query":"菠萝蜜"}"#)
        .await
        .unwrap();
    assert_eq!(out["count"].as_u64(), Some(3), "默认命中数应为 3");
    for r in out["results"].as_array().unwrap() {
        let win_len = r["window"].as_array().unwrap().len();
        assert!(win_len <= 5, "默认窗口应 ≤ 5 条（radius 2）：{win_len}");
    }
    assert!(out.get("truncated").is_none(), "默认组合不应触发截断");
}

#[tokio::test]
async fn test_session_recall_clamps_limit_and_radius() {
    // 预算决定权上收：limit 请求 50 → 钳 10；radius 请求 9 → 钳 5（窗口 ≤ 11 条）。
    let mut msgs = Vec::new();
    for i in 0..30 {
        msgs.push((MessageRole::User, format!("第 {i} 条消息，关键词火龙果")));
    }
    let registry = make_registry_with_recall("s1", &msgs).await;
    let out = registry
        .execute_session_recall(r#"{"query":"火龙果","limit":50,"radius":9}"#)
        .await
        .unwrap();
    let count = out["count"].as_u64().unwrap();
    assert!(count <= 10, "limit 应被钳到 10：{count}");
    for r in out["results"].as_array().unwrap() {
        let win_len = r["window"].as_array().unwrap().len();
        assert!(win_len <= 11, "窗口应 ≤ 11 条（radius 钳到 5）：{win_len}");
    }
}

#[tokio::test]
async fn test_session_recall_total_budget_truncates_with_message() {
    // 总预算：长消息 × 宽窗口 → 命中整条停加，附 truncated + message（首个保留）。
    // 判别力：无预算治理的旧实现会把 5 个命中全部输出（>30k 字符）。
    // 构造：30 条 ~790 字符的长消息全含关键词 → 每个命中窗口 11 条 ≈ 8.7k 字符，
    //       5 个命中远超 12k 预算。
    let long_tail = "x".repeat(760);
    let mut msgs = Vec::new();
    for i in 0..30 {
        msgs.push((
            MessageRole::User,
            format!("第 {i} 条，关键词香蕉奶昔 {long_tail}"),
        ));
    }
    let registry = make_registry_with_recall("s1", &msgs).await;
    let out = registry
        .execute_session_recall(r#"{"query":"香蕉奶昔","limit":5,"radius":5}"#)
        .await
        .unwrap();
    let count = out["count"].as_u64().unwrap();
    assert!(count >= 1, "首个命中必须保留：{count}");
    assert!(count < 5, "总预算应截断后续命中：{count}");
    assert_eq!(out["truncated"].as_bool(), Some(true));
    let msg = out["message"].as_str().unwrap_or_default();
    assert!(msg.contains("截断"), "应附续查指引：{msg}");
}
