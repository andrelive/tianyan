//! 文件类工具执行器测试：read_file / write_file / vfs_read / vfs_list。
//!
//! 覆盖安全关键行为：安全策略拒绝、VFS 检索、未配置依赖的错误路径、成功路径。
//! 所有测试仅依赖 tempfile 与共享 Mock，不访问网络、真实 LLM 或真实 LanceDB。

use crate::agent::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::config::{ApprovalMode, SafetyMode};
use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
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
fn file_policy(allowed: Vec<PathBuf>, blocked: Vec<PathBuf>) -> SecurityPolicy {
    let mut policy = default_strict_policy();
    policy.allowed_directories = allowed
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    policy.blocked_directories = blocked
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    policy
}

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

/// 将路径与编辑列表编码进 apply_edit 的 JSON 参数。
fn apply_edit_args(path: &std::path::Path, edits: serde_json::Value) -> String {
    json!({ "path": path.to_string_lossy(), "edits": edits }).to_string()
}

#[tokio::test]
async fn test_apply_edit_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    std::fs::write(&path, "let a = 1;\nlet b = 2;\nlet c = 3;\n").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let edits = json!([
        { "old_string": "let b = 2;", "new_string": "let b = 20;" }
    ]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await
        .unwrap();
    assert_eq!(result["edits_applied"].as_u64(), Some(1));
    assert_eq!(result["path"].as_str().unwrap(), path.to_string_lossy());
    // read_file 可验证新内容（哈希锚点对上新内容）
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "let a = 1;\nlet b = 20;\nlet c = 3;\n");
}

#[tokio::test]
async fn test_apply_edit_rejects_missing_edits() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_apply_edit(r#"{"path":"x.txt"}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_apply_edit_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(dir.path().join("secret.txt"), "a\nb\n").unwrap();

    // `../` 逃逸路径：规范化后位于 allowed 目录之外，必须被安全策略拒绝。
    let escape = allowed.join("..").join("secret.txt");
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let edits = json!([{ "old_string": "a", "new_string": "x" }]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&escape, edits), "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_apply_edit_rejects_when_file_write_disabled() {
    let mut policy = default_strict_policy();
    policy.allow_file_write = false;
    let registry = ToolRegistry::new(policy);
    let edits = json!([{ "old_string": "a", "new_string": "x" }]);
    let result = registry
        .execute_apply_edit(
            &apply_edit_args(std::path::Path::new("any.txt"), edits),
            "test-session",
            false,
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_apply_edit_approval_denied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.txt");
    std::fs::write(&path, "a\nb\n").unwrap();
    // 确认模式（ADR-033）：Medium 风险无确认时立即拒绝并降级为询问用户。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let edits = json!([{ "old_string": "a", "new_string": "x" }]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
    assert!(msg.contains("操作需要用户确认"), "应要求用户确认: {msg}");
    // 文件未被改写（原子拒绝）
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "a\nb\n");
}

#[tokio::test]
async fn test_apply_edit_approval_approved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.txt");
    std::fs::write(&path, "a\nb\n").unwrap();

    // 全自主模式（ADR-033）：Medium 风险自动批准。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let edits = json!([{ "old_string": "a", "new_string": "x" }]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await;
    assert!(result.is_ok(), "全自主应批准编辑: {result:?}");
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "x\nb\n");
}

#[tokio::test]
async fn test_apply_edit_not_found_is_conflict() {
    // ADR-014：old_string 未找到是 conflict 类错误（文件当前内容与模型预期不符），
    // 工具包装器必须保留分类（否则 server 层收到 Custom 映射为 500 而非 409）。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    std::fs::write(&path, "let a = 1;\nlet b = 2;\n").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let edits = json!([
        { "old_string": "完全不匹配的内容", "new_string": "let a = 10;" }
    ]);
    let err = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await
        .unwrap_err();
    assert!(
        err.is_conflict(),
        "old_string 未找到应分类为 conflict：{err}"
    );
}

// ── apply_patch ───────────────────────────────────────────────────────────

/// 将补丁文本编码进 apply_patch 的 JSON 参数。
fn apply_patch_args(patch: &str) -> String {
    json!({ "patch": patch }).to_string()
}

/// apply_patch 的补丁路径相对测试进程 CWD 解析（registry 层以 current_dir
/// 为基准做逐文件路径安全检查，executor 层以 current_dir 为 base_dir）。
/// 在 CWD 下创建唯一临时目录承载测试文件，结束后删除；断言失败残留时，
/// 下次同 tag 运行先删后建，不影响其它测试。
fn cwd_scoped_dir(tag: &str) -> PathBuf {
    let base = std::env::current_dir().expect("测试进程应有 CWD");
    let dir = base.join(format!(".apply_patch_test_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建 CWD 临时目录");
    dir
}

#[tokio::test]
async fn test_apply_patch_success_multi_file() {
    let dir = cwd_scoped_dir("multi");
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, "old\n").unwrap();
    std::fs::write(&b, "bee\n").unwrap();
    let patch = "\
*** Update File: .apply_patch_test_multi/a.txt
@@ -1,1 +1,1 @@
-old
+NEW
*** Update File: .apply_patch_test_multi/b.txt
@@ -1,1 +1,1 @@
-bee
+B
";
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_apply_patch(&apply_patch_args(patch), "test-session", false)
        .await
        .unwrap();
    assert_eq!(result["total_files"].as_u64(), Some(2));
    assert_eq!(result["files"][0]["hunks_applied"].as_u64(), Some(1));
    // 双文件内容均被改写
    assert_eq!(tokio::fs::read_to_string(&a).await.unwrap(), "NEW\n");
    assert_eq!(tokio::fs::read_to_string(&b).await.unwrap(), "B\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_apply_patch_rejects_missing_patch() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_apply_patch(r#"{}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_apply_patch_rejects_path_traversal() {
    // 补丁路径含 `..`：在 `resolve_patch_path` 即被拒绝（早于白名单判定）——
    // 深度防御：未经 `check_path` 的调用方（如 server workspace 入口）同样被拦。
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(dir.path().join("secret.txt"), "a\nb\n").unwrap();
    let escape = allowed.join("..").join("secret.txt");
    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-a\n+b\n",
        escape.to_string_lossy()
    );
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry
        .execute_apply_patch(&apply_patch_args(&patch), "test-session", false)
        .await;
    assert!(result.is_err());
    // 拒绝点前移（`..` 检查先于白名单判定）→ 错误类别为"非法路径"；逃逸同样未发生
    assert!(result.unwrap_err().to_string().contains("非法路径"));
}

#[tokio::test]
async fn test_apply_patch_allows_absolute_path_inside_allowlist() {
    // 路径口径统一后：白名单内**绝对路径**应放行——此前 executor 层无条件拒绝
    // 绝对路径，形成"检查通过但执行失败"的分裂（apply_edit / read_file 均接受）。
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let target = allowed.join("a.txt");
    std::fs::write(&target, "old\n").unwrap();
    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-old\n+NEW\n",
        target.to_string_lossy()
    );
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry
        .execute_apply_patch(&apply_patch_args(&patch), "test-session", false)
        .await;
    assert!(result.is_ok(), "白名单内绝对路径应放行: {result:?}");
    assert_eq!(tokio::fs::read_to_string(&target).await.unwrap(), "NEW\n");
}

#[tokio::test]
async fn test_apply_patch_rejects_when_file_write_disabled() {
    let mut policy = default_strict_policy();
    policy.allow_file_write = false;
    let registry = ToolRegistry::new(policy);
    let result = registry
        .execute_apply_patch(
            &apply_patch_args("*** Update File: x.txt\n@@ -1,1 +1,1 @@\n-a\n+b\n"),
            "test-session",
            false,
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_apply_patch_approval_denied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.txt");
    std::fs::write(&path, "a\nb\n").unwrap();
    // 确认模式（ADR-033）：Medium 风险无确认时立即拒绝并降级为询问用户。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-a\n+b\n",
        path.to_string_lossy()
    );
    let result = registry
        .execute_apply_patch(&apply_patch_args(&patch), "test-session", false)
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
    assert!(msg.contains("操作需要用户确认"), "应要求用户确认: {msg}");
    // 文件未被改写（审批门控在落盘前拦截）
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "a\nb\n");
}

#[tokio::test]
async fn test_apply_patch_approval_approved() {
    let dir = cwd_scoped_dir("approved");
    let path = dir.join("code.txt");
    std::fs::write(&path, "a\nb\n").unwrap();

    // 全自主模式（ADR-033）：Medium 风险自动批准。
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        ..Default::default()
    };
    let workflow = Arc::new(ApprovalWorkflow::new(config));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let patch = "\
*** Update File: .apply_patch_test_approved/code.txt
@@ -1,2 +1,1 @@
-a
-b
+c
";
    let result = registry
        .execute_apply_patch(&apply_patch_args(patch), "test-session", false)
        .await;
    assert!(result.is_ok(), "全自主应批准补丁: {result:?}");
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "c\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_apply_patch_rejects_second_file_absolute_outside_allowlist() {
    // 沙箱绕过回归：首个文件合法、第二个文件为白名单外绝对路径 → 必须在
    // 审批门控与落盘之前被逐文件路径检查拦截（旧实现只检查首个文件路径）。
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let victim = allowed.join("a.txt");
    std::fs::write(&victim, "old\n").unwrap();
    let outside = dir.path().join("outside").join("b.txt");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "bee\n").unwrap();
    let patch = format!(
        "*** Update File: {}\n@@ -1,1 +1,1 @@\n-old\n+NEW\n*** Update File: {}\n@@ -1,1 +1,1 @@\n-bee\n+B\n",
        victim.to_string_lossy(),
        outside.to_string_lossy()
    );
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry
        .execute_apply_patch(&apply_patch_args(&patch), "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
    // 首个文件未被改写（门控在落盘前拦截）
    assert_eq!(tokio::fs::read_to_string(&victim).await.unwrap(), "old\n");
}
