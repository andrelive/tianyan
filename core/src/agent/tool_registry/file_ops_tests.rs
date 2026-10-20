//! 文件类工具执行器测试：vfs_read / vfs_list（read/write/edit/patch 已迁至插件）。
//!
//! 覆盖安全关键行为：安全策略拒绝、VFS 检索、未配置依赖的错误路径、成功路径。
//! 所有测试仅依赖 tempfile 与共享 Mock，不访问网络、真实 LLM 或真实 LanceDB。

use crate::agent::ToolRegistry;
use std::sync::Arc;

use crate::config::SafetyMode;
use crate::executor::SecurityPolicy;
use crate::test_utils::MockVfs;

use super::*;

/// 默认严格策略：允许任意路径，但阻止常见破坏性命令。
fn default_strict_policy() -> SecurityPolicy {
    SecurityPolicy {
        safety_mode: SafetyMode::Strict,
        trash_directory: std::env::temp_dir(),
        allowed_commands: None,
        blocked_commands: vec![
            "rm".to_string(),
            "del".to_string(),
            "format".to_string(),
            "shutdown".to_string(),
            "taskkill".to_string(),
        ],
        allowed_directories: Vec::new(),
        blocked_directories: Vec::new(),
        allow_file_write: true,
        max_command_timeout_secs: 30,
        max_file_size: 1024 * 1024,
        block_interpreters: true,
    }
}

/// 带目录白/黑名单的严格策略。
///
/// 目录先规范化（与 `SecurityPolicy::check_path` 对目标路径的规范化保持一致，
/// 避免 Windows 上 Temp 目录 junction 导致的 `starts_with` 前缀失配）。

#[tokio::test]
async fn test_vfs_read_session_export_truncated_for_large_history() {
    // 回归保护（2658414）：会话 JSONL 导出经头部截断（50KB/2000 行）+ 检索提示，
    // 防止整读数 MB 会话压垮上下文。
    let db = crate::db::Database::open_in_memory().unwrap();
    db.init_schemas().await.unwrap();
    let store = crate::session::store::SessionStore::new(db).unwrap();
    store
        .create("s-big", &crate::session::types::SessionHeader::default())
        .await
        .unwrap();
    // 每条约 1KB × 120 条 ≈ 120KB，超过 50KB 截断上限。
    for i in 0..120 {
        let mut msg = crate::common::types::StructuredMessage::user("s-big", "y".repeat(1000));
        msg.id = format!("m{i}");
        store.append_message("s-big", &msg).await.unwrap();
    }

    let registry = ToolRegistry::new(default_strict_policy())
        .with_vfs(Arc::new(MockVfs::new()))
        .with_session_store(store);
    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://session/s-big"}"#)
        .await
        .unwrap();
    assert_eq!(
        result["truncated"].as_bool(),
        Some(true),
        "大体量会话导出应被截断: {}",
        result.to_string().chars().take(200).collect::<String>()
    );
    let total = result["total_bytes"].as_u64().unwrap();
    assert!(total > 50 * 1024, "total_bytes 应为原文体量: {total}");
    let detail = result["detail"].as_str().unwrap();
    assert!(
        detail.contains("输出已截断"),
        "应包含截断标记: {}",
        detail.chars().take(100).collect::<String>()
    );
    assert!(
        detail.contains("session_recall"),
        "应包含检索提示（session_recall）: {}",
        detail.chars().take(100).collect::<String>()
    );
}

// ── write_file ───────────────────────────────────────────────────────────

// ── write_file ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vfs_read_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1"}"#)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_vfs_read_success_with_content() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::builder()
        .with_content(&uri, ContentLevel::Abstract, "the abstract")
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1"}"#)
        .await
        .unwrap();
    assert_eq!(result["abstract"].as_str().unwrap(), "the abstract");
    // 未写入的层级返回空字符串（not_found 正常状态），而非错误。
    assert_eq!(result["overview"].as_str().unwrap(), "");
    assert_eq!(result["detail"].as_str().unwrap(), "");
}

#[tokio::test]
async fn test_vfs_read_rejects_invalid_uri() {
    let vfs = MockVfs::new();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));
    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://invalid_namespace/doc"}"#)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("无效 URI"));
}

/// 正文按行取段：目录（L1）里的章节行号 → offset/limit 窗口（1 起始、钳制 2000）。
#[tokio::test]
async fn test_vfs_read_detail_offset_limit_window() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::new();
    let detail: String = (1..=10)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    vfs.set_content(&uri, ContentLevel::Detail, &detail);
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1","offset":3,"limit":3}"#)
        .await
        .unwrap();
    assert_eq!(
        result["detail"].as_str().unwrap(),
        "l3\nl4\nl5\n... (输出已截断，共 10 行，使用 offset 6 继续)"
    );
    assert_eq!(result["offset"].as_u64(), Some(3));
    assert_eq!(result["total_lines"].as_u64(), Some(10));
    assert_eq!(result["truncated"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_vfs_read_detail_offset_beyond_total_errors() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::new();
    vfs.set_content(&uri, ContentLevel::Detail, "a\nb");
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let err = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1","offset":99}"#)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("超出正文总行数"),
        "offset 越界应报错: {err}"
    );
}

/// 无 offset/limit：正文头部截断至 2000 行 + "使用 offset 继续"提示
/// ——正文不再无上限倾泻。
#[tokio::test]
async fn test_vfs_read_detail_head_truncated_without_offset() {
    let uri = TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]);
    let vfs = MockVfs::new();
    let long: String = (1..=3000)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    vfs.set_content(&uri, ContentLevel::Detail, &long);
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_read(r#"{"uri":"tianyan://knowledge/doc1"}"#)
        .await
        .unwrap();
    let detail = result["detail"].as_str().unwrap();
    assert!(detail.contains("使用 offset 继续"), "应附续读提示");
    assert_eq!(result["truncated"].as_bool(), Some(true));
    assert_eq!(result["total_lines"].as_u64(), Some(3000));
}

// ── vfs_list ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vfs_list_not_configured() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_vfs_list(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("VFS not configured"));
}

#[tokio::test]
async fn test_vfs_list_success() {
    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let vfs = MockVfs::new();
    vfs.add_entry(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]),
    );
    vfs.add_directory(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["subdir".to_string()]),
    );
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_list(r#"{"uri":"tianyan://knowledge"}"#)
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(2));
    assert_eq!(result["entries"][0]["is_directory"].as_bool(), Some(false));
    assert_eq!(result["entries"][1]["is_directory"].as_bool(), Some(true));
    // 未生成摘要的条目省略简介字段（目录节点/未生成时同此）
    assert!(result["entries"][0].get("abstract").is_none());
}

#[tokio::test]
async fn test_vfs_list_defaults_to_knowledge_root() {
    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let vfs = MockVfs::new();
    vfs.add_entry(
        &dir,
        &TianyanUri::new(ContextNamespace::Knowledge, vec!["doc1".to_string()]),
    );
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry.execute_vfs_list(r#"{}"#).await.unwrap();
    assert_eq!(result["uri"].as_str().unwrap(), "tianyan://knowledge");
    assert_eq!(result["count"].as_u64(), Some(1));
}

/// 目录条目的"含义"来源 = 子条目 L0 简介（多文件技能/文件夹的现遍历目录，
/// ADR-001 修订：L1 语义 = 目录）。
#[tokio::test]
async fn test_vfs_list_entries_include_abstract() {
    use crate::vfs::ContextEntry;

    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let mut doc_entry = ContextEntry::new_file(TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["alipay".to_string()],
    ));
    doc_entry.abstract_content = Some("支付宝支付接入指南".to_string());
    let mut sub_entry = ContextEntry::new_directory(TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["wechat".to_string()],
    ));
    sub_entry.abstract_content = Some("微信支付接入".to_string());

    let vfs = MockVfs::builder()
        .with_entries(&dir, vec![doc_entry, sub_entry])
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));

    let result = registry
        .execute_vfs_list(r#"{"uri":"tianyan://knowledge"}"#)
        .await
        .unwrap();
    assert_eq!(
        result["entries"][0]["abstract"].as_str(),
        Some("支付宝支付接入指南")
    );
    assert_eq!(
        result["entries"][1]["abstract"].as_str(),
        Some("微信支付接入")
    );
    assert_eq!(result["entries"][1]["is_directory"].as_bool(), Some(true));
}

/// 空简介省略字段；超长简介按字节截断（防御生成失败回退路径的长文本）。
#[tokio::test]
async fn test_vfs_list_omits_empty_abstract_and_truncates_long() {
    use crate::vfs::ContextEntry;

    let dir = TianyanUri::new(ContextNamespace::Knowledge, vec![]);
    let plain = ContextEntry::new_file(TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["no-summary".to_string()],
    ));
    let mut long = ContextEntry::new_file(TianyanUri::new(
        ContextNamespace::Knowledge,
        vec!["long-summary".to_string()],
    ));
    long.abstract_content = Some("长".repeat(1000)); // 3000 字节 > 480

    let vfs = MockVfs::builder()
        .with_entries(&dir, vec![plain, long])
        .build();
    let registry = ToolRegistry::new(default_strict_policy()).with_vfs(Arc::new(vfs));
    let result = registry
        .execute_vfs_list(r#"{"uri":"tianyan://knowledge"}"#)
        .await
        .unwrap();
    assert!(
        result["entries"][0].get("abstract").is_none(),
        "无简介应省略字段"
    );
    let got = result["entries"][1]["abstract"].as_str().unwrap();
    assert!(got.ends_with('…'), "超长简介应截断附省略号");
    assert!(got.len() <= 480 + '…'.len_utf8());
}

// ── apply_edit ────────────────────────────────────────────────────────────
