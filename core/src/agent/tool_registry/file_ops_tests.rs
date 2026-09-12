//! 文件类工具执行器测试：read_file / write_file / vfs_read / vfs_list。
//!
//! 覆盖安全关键行为：安全策略拒绝、VFS 检索、未配置依赖的错误路径、成功路径。
//! 所有测试仅依赖 tempfile 与共享 Mock，不访问网络、真实 LLM 或真实 LanceDB。

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

// ── read_file ────────────────────────────────────────────────────────────

/// 将文件路径安全地编码进 JSON 参数（Windows 路径含反斜杠，必须转义）。
fn read_file_args(path: &std::path::Path) -> String {
    json!({ "path": path.to_string_lossy() }).to_string()
}

/// 将路径、offset、limit 编码进 read_file 的 JSON 参数（None 编码为 null）。
fn read_file_window_args(
    path: &std::path::Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> String {
    json!({ "path": path.to_string_lossy(), "offset": offset, "limit": limit }).to_string()
}

/// 将路径与内容编码进 write_file 的 JSON 参数。
fn write_file_args(path: &std::path::Path, content: &str) -> String {
    json!({ "path": path.to_string_lossy(), "content": content }).to_string()
}

#[tokio::test]
async fn test_read_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    std::fs::write(&path, "hello content").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![dir.path().to_path_buf()], vec![]));
    let result = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();
    // 新契约：结构化 JSON（不再是纯字符串）
    assert_eq!(result["path"].as_str().unwrap(), path.to_string_lossy());
    assert_eq!(result["truncated"].as_bool(), Some(false));
    assert_eq!(result["total_lines"].as_u64(), Some(1));
    let content = result["content"].as_str().unwrap();
    // 内容匹配编辑：read_file 输出纯内容（不再带行号/哈希前缀）
    assert_eq!(content, "hello content");
}

#[tokio::test]
async fn test_read_file_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_read_file(r#"{}"#, "test-session").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_read_file_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(dir.path().join("secret.txt"), "secret").unwrap();

    // `../` 逃逸路径：虽然 secret.txt 真实存在，但其规范化路径
    // 位于 allowed 目录之外，必须被安全策略拒绝。
    let escape = allowed.join("..").join("secret.txt");
    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry
        .execute_read_file(&read_file_args(&escape), "test-session")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_read_file_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();

    let registry = ToolRegistry::new(file_policy(vec![allowed.to_path_buf()], vec![]));
    let result = registry
        .execute_read_file(&read_file_args(&secret), "test-session")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_read_file_missing_file_returns_error() {
    // 无目录白名单限制：安全检查放行后，读取不存在文件应报执行错误。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let missing = dir.path().join("does_not_exist.txt");
    let result = registry
        .execute_read_file(&read_file_args(&missing), "test-session")
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("执行失败"));
    assert!(msg.contains("文件不存在"), "错误应说明文件不存在: {msg}");
}

#[tokio::test]
async fn test_read_file_missing_file_is_not_found() {
    // ADR-014：工具包装器必须保留底层 not_found 分类
    // （否则 server 层收到 Custom 消息映射为 500 而非 404）。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let missing = dir.path().join("does_not_exist.txt");
    let err = registry
        .execute_read_file(&read_file_args(&missing), "test-session")
        .await
        .unwrap_err();
    assert!(err.is_not_found(), "缺失文件应分类为 not_found：{err}");
}

#[tokio::test]
async fn test_read_file_offset_limit_slicing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    std::fs::write(&path, "l1\nl2\nl3\nl4").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(
            &read_file_window_args(&path, Some(2), Some(2)),
            "test-session",
        )
        .await
        .unwrap();
    assert_eq!(result["total_lines"].as_u64(), Some(4));
    assert_eq!(result["truncated"].as_bool(), Some(true));
    assert_eq!(result["showing"]["offset"].as_u64(), Some(2));
    assert_eq!(result["showing"]["limit"].as_u64(), Some(2));
    let content = result["content"].as_str().unwrap();
    // 窗口内容 = 文件第 2-3 行（内容匹配编辑：read_file 输出纯内容，无行号/哈希前缀）
    assert!(
        content.starts_with("l2\nl3"),
        "窗口应从第 2 行开始: {content}"
    );
    // 截断提示行是内容的最后一行，且同步暴露在 message 字段
    assert!(
        content
            .lines()
            .last()
            .unwrap()
            .contains("Showing lines 2-3 of 4"),
        "截断提示应为最后一行: {content}"
    );
    assert_eq!(
        result["message"].as_str().unwrap(),
        "(Showing lines 2-3 of 4. Use offset=4 to continue.)"
    );
}

#[tokio::test]
async fn test_read_file_window_returns_plain_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.txt");
    std::fs::write(&path, "let x = 1;\nlet y = 2;\nlet z = 3;").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(
            &read_file_window_args(&path, Some(2), Some(1)),
            "test-session",
        )
        .await
        .unwrap();
    let content = result["content"].as_str().unwrap();
    // 内容匹配编辑：read_file 输出纯内容（无行号/哈希前缀）
    assert!(
        content.starts_with("let y = 2;"),
        "窗口应从第 2 行内容开始: {content}"
    );
}

#[tokio::test]
async fn test_read_file_offset_beyond_total_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.txt");
    std::fs::write(&path, "a\nb\nc").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_read_file(&read_file_window_args(&path, Some(5), None), "test-session")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("offset"), "错误应包含 offset: {msg}");
    assert!(msg.contains("超出文件总行数"), "错误应说明越界: {msg}");
}

#[tokio::test]
async fn test_read_file_missing_suggests_similar() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello2.txt"), "x").unwrap();
    std::fs::write(dir.path().join("world.txt"), "y").unwrap();
    let missing = dir.path().join("hello.txt");

    let registry = ToolRegistry::new(default_strict_policy());
    let err = registry
        .execute_read_file(&read_file_args(&missing), "test-session")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("文件不存在"), "错误应说明文件不存在: {msg}");
    assert!(msg.contains("hello2.txt"), "应建议相似文件名: {msg}");
    assert!(!msg.contains("world.txt"), "无关文件不应被建议: {msg}");
}

#[tokio::test]
async fn test_read_file_binary_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.bin");
    std::fs::write(&path, [0x00u8, 0x01, 0x02, 0xff]).unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();
    assert_eq!(result["binary"].as_bool(), Some(true));
    assert_eq!(result["size"].as_u64(), Some(4));
    assert!(result["preview"].is_string());
    assert!(
        result.get("content").is_none(),
        "二进制分支不应返回 content"
    );
}

#[tokio::test]
async fn test_read_file_directory_lists_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("zdir")).unwrap();
    std::fs::create_dir_all(root.join("adir")).unwrap();
    std::fs::write(root.join("apple.txt"), "a").unwrap();
    std::fs::write(root.join("banana.txt"), "b").unwrap();
    // 隐藏条目不忽略，仅参与排序
    std::fs::write(root.join(".hidden"), "h").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(&read_file_args(&root), "test-session")
        .await
        .unwrap();
    // 目录模式统一输出 list_dir 形状（ListDirOutput：entries/count/truncated/total）
    let entries = result["entries"].as_array().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["dir", "dir", "file", "file", "file"]);
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    // 目录名带尾部 /（list_dir 约定）
    assert_eq!(
        names,
        vec!["adir/", "zdir/", ".hidden", "apple.txt", "banana.txt"]
    );
    // 条目携带完整路径
    assert_eq!(
        entries[0]["path"].as_str().unwrap(),
        root.join("adir").to_str().unwrap()
    );
}

#[tokio::test]
async fn test_read_file_long_line_returned_fully() {
    // 行内不截断：模型需要完整行信息推理，长行应整体返回（体量由 offset/limit + 字节预算兜底）
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.txt");
    let long = "x".repeat(2500);
    std::fs::write(&path, format!("{long}\nshort")).unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();
    let content = result["content"].as_str().unwrap();
    let first_line = content.lines().next().unwrap();
    // 长行完整返回（行内不截断，内容匹配编辑无前缀）
    assert_eq!(first_line, &long, "长行应完整返回，而非截断");
    // 总行数仍是原始行数
    assert_eq!(result["total_lines"].as_u64(), Some(2));
}

#[tokio::test]
async fn test_read_file_default_limit_2000() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let mut file_content = String::new();
    for i in 1..=2500 {
        file_content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&path, file_content).unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();
    assert_eq!(result["total_lines"].as_u64(), Some(2500));
    assert_eq!(result["truncated"].as_bool(), Some(true));
    let content = result["content"].as_str().unwrap();
    assert!(
        content.contains("(Showing lines 1-2000 of 2500. Use offset=2001 to continue.)"),
        "应提示续读偏移: {content}"
    );
    assert!(
        content.starts_with("line 1\n"),
        "窗口应从第 1 行开始: {content}"
    );
}

#[tokio::test]
async fn test_read_file_empty_file_returns_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.txt");
    std::fs::write(&path, "").unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();
    assert_eq!(result["content"].as_str().unwrap(), "");
    assert_eq!(result["truncated"].as_bool(), Some(false));
    assert_eq!(result["total_lines"].as_u64(), Some(0));
}

#[tokio::test]
async fn test_read_file_byte_truncation_continuation_no_skipped_lines() {
    // 回归保护（71511a6）：字节截断时续读偏移必须按「实际返回行数」计算——
    // 修复前按标称窗口（end-start）计算，续读会跳过被截掉的行（前端"加载更多"跳行）。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.txt");
    // 每行 100 字符 + 换行 = 101 字节；1500 行 ≈ 151KB > 50KB 截断上限 → 窗口内必然字节截断。
    let mut file_content = String::new();
    for i in 1..=1500 {
        file_content.push_str(&format!("line-{i:04}-{}\n", "x".repeat(90)));
    }
    std::fs::write(&path, file_content).unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let first = registry
        .execute_read_file(&read_file_args(&path), "test-session")
        .await
        .unwrap();

    assert_eq!(
        first["truncated"].as_bool(),
        Some(true),
        "应发生字节截断: {first}"
    );
    let returned = first["showing"]["limit"].as_u64().unwrap();
    assert!(
        returned > 0 && returned < 1500,
        "实际返回行数应为部分窗口（字节截断），实际: {returned}"
    );
    assert_eq!(first["showing"]["offset"].as_u64(), Some(1));
    let content = first["content"].as_str().unwrap();
    // 第一段：末数据行 = 第 `returned` 行；且不含第 `returned+1` 行（截断干净）。
    let last_data = content
        .lines()
        .filter(|l| l.starts_with("line-"))
        .last()
        .unwrap();
    assert!(
        last_data.starts_with(&format!("line-{returned:04}-")),
        "第一段末行应为第 {returned} 行: {last_data}"
    );
    let expect_next = returned + 1;
    assert!(
        !content.contains(&format!("line-{expect_next:04}-")),
        "第一段不应包含第 {expect_next} 行"
    );
    // 续读偏移 = 起始 offset + 实际返回行数（修复前取标称窗口 1500 → 1501）。
    assert!(
        content.contains(&format!("Use offset={expect_next} to continue")),
        "续读偏移应为 {expect_next}（按实际行数）: {content}"
    );

    // 续读必须以第 `expect_next` 行开头——修复前 offset=1501 会跳过中间所有行。
    let second = registry
        .execute_read_file(
            &read_file_window_args(&path, Some(expect_next as usize), None),
            "test-session",
        )
        .await
        .unwrap();
    let second_content = second["content"].as_str().unwrap();
    assert!(
        second_content.starts_with(&format!("line-{expect_next:04}-")),
        "续读应从第 {expect_next} 行开始（无跳行）: {}",
        &second_content.chars().take(60).collect::<String>()
    );
}

#[tokio::test]
async fn test_read_file_oversized_limit_clamped_with_total_bytes() {
    // 回归保护（2658414）：超大 limit 被钳制到 2000；total_bytes 为文件真实字节数。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let mut file_content = String::new();
    let mut expected_bytes = 0usize;
    for i in 1..=2500 {
        let line = format!("line {i}\n");
        expected_bytes += line.len();
        file_content.push_str(&line);
    }
    std::fs::write(&path, &file_content).unwrap();

    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_read_file(
            &read_file_window_args(&path, None, Some(100_000)),
            "test-session",
        )
        .await
        .unwrap();
    assert_eq!(
        result["showing"]["limit"].as_u64(),
        Some(2000),
        "超大 limit 应被钳制到 2000: {result}"
    );
    assert_eq!(result["total_bytes"].as_u64(), Some(expected_bytes as u64));
    assert_eq!(result["total_lines"].as_u64(), Some(2500));
    assert_eq!(result["truncated"].as_bool(), Some(true));
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
        &detail.chars().take(100).collect::<String>()
    );
    assert!(
        detail.contains("session_recall"),
        "应包含检索提示（session_recall）: {}",
        &detail.chars().take(100).collect::<String>()
    );
}

// ── write_file ───────────────────────────────────────────────────────────

// ── write_file ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_write_file_success() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let path = dir.path().join("out.txt");
    let result = registry
        .execute_write_file(
            &write_file_args(&path, "written content"),
            "test-session",
            false,
        )
        .await;
    assert!(result.is_ok());
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk, "written content");
}

#[tokio::test]
async fn test_write_file_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_write_file(r#"{}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_write_file_rejects_blocked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    // 预创建目标文件，使规范化路径能命中黑名单目录（与生产 check_path 行为一致）。
    let path = blocked.join("x.txt");
    std::fs::write(&path, "existing").unwrap();
    let registry = ToolRegistry::new(file_policy(vec![], vec![blocked.to_path_buf()]));
    let result = registry
        .execute_write_file(&write_file_args(&path, "x"), "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_write_file_rejects_when_file_write_disabled() {
    let mut policy = default_strict_policy();
    policy.allow_file_write = false;
    let registry = ToolRegistry::new(policy);
    let result = registry
        .execute_write_file(r#"{"path":"any.txt","content":"x"}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

// ── vfs_read ─────────────────────────────────────────────────────────────

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
    // 补丁首个文件路径逃逸白名单目录 → registry check_path 拒绝。
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
    assert!(result.unwrap_err().to_string().contains("安全违规"));
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

// ── LSP 诊断附加 ──────────────────────────────────────────────────────────

/// 在 LspManager 中预置一条诊断（无需真实服务器）。
fn seeded_manager(path: &std::path::Path) -> Arc<crate::lsp::diagnostics::LspManager> {
    let manager = Arc::new(crate::lsp::diagnostics::LspManager::new());
    manager.store.insert(
        path.to_string_lossy().to_string(),
        vec![crate::executor::verification::StructuredDiagnostic {
            file: path.to_string_lossy().to_string(),
            line: 1,
            column: 1,
            level: "error".to_string(),
            code: Some("E0308".to_string()),
            message: "类型不匹配".to_string(),
            suggestion: None,
        }],
    );
    manager
}

#[tokio::test]
async fn test_apply_edit_without_lsp_manager_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.txt");
    std::fs::write(&path, "a\nb\n").unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let edits = json!([{ "old_string": "a", "new_string": "x" }]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await
        .expect("编辑成功");
    assert!(
        result.get("diagnostics").is_none(),
        "未配置 LSP 管理器时结果不应包含 diagnostics: {result}"
    );
}

#[tokio::test]
async fn test_apply_edit_with_lsp_manager_appends_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let registry =
        ToolRegistry::new(default_strict_policy()).with_lsp_manager(seeded_manager(&path));
    let edits = json!([{ "old_string": "fn main() {}", "new_string": "fn main() {}" }]);
    let result = registry
        .execute_apply_edit(&apply_edit_args(&path, edits), "test-session", false)
        .await
        .expect("编辑成功");
    let diagnostics = result
        .get("diagnostics")
        .expect("配置 LSP 管理器后结果应包含 diagnostics");
    assert_eq!(diagnostics[0]["message"], "类型不匹配");
    assert_eq!(diagnostics[0]["line"], 1);
}

#[tokio::test]
async fn test_apply_patch_with_lsp_manager_appends_diagnostics() {
    let dir = cwd_scoped_dir("lsp");
    let rel = ".apply_patch_test_lsp/code.rs";
    std::fs::write(dir.join("code.rs"), "fn main() {}\n").unwrap();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_lsp_manager(seeded_manager(std::path::Path::new(rel)));
    let patch = "\
*** Update File: .apply_patch_test_lsp/code.rs
@@ -1,1 +1,1 @@
-fn main() {}
+fn main() { let x: u32 = 1; }
";
    let result = registry
        .execute_apply_patch(&apply_patch_args(patch), "test-session", false)
        .await
        .expect("补丁成功");
    let diagnostics = result
        .get("diagnostics")
        .expect("配置 LSP 管理器后 apply_patch 结果应包含 diagnostics");
    assert_eq!(diagnostics[0]["level"], "error");
    let _ = std::fs::remove_dir_all(&dir);
}
