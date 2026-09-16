//! 代码类工具执行器测试：grep / run_tests / verify_build。

use std::sync::Arc;

use serde_json::json;

use crate::config::{ApprovalMode, SafetyMode};
use crate::executor::approval::{ApprovalWorkflow, ApprovalWorkflowConfig};
use crate::executor::SecurityPolicy;

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

// ── grep ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_code_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_search_code(r#"{}"#, "test-session").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

/// 带目录白名单的严格策略（目录规范化与 `check_path_rules` 保持一致）。
fn file_policy(allowed: Vec<std::path::PathBuf>) -> SecurityPolicy {
    SecurityPolicy {
        safety_mode: SafetyMode::Strict,
        trash_directory: std::env::temp_dir(),
        allowed_commands: None,
        blocked_commands: Vec::new(),
        allowed_directories: allowed,
        blocked_directories: Vec::new(),
        allow_file_write: true,
        max_command_timeout_secs: 30,
        max_file_size: 1024 * 1024,
        block_interpreters: true,
    }
}

#[tokio::test]
async fn test_search_code_basic_success() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() {}\n").unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let path = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = registry
        .execute_search_code(
            &format!(r#"{{"pattern":"foo","path":{path}}}"#),
            "test-session",
        )
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(1));
    assert!(result["results"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("a.rs"));
}

#[tokio::test]
async fn test_search_code_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("a.rs"), "fn foo() {}\n").unwrap();
    let registry = ToolRegistry::new(file_policy(vec![allowed]));
    let path = serde_json::to_string(&outside.to_string_lossy().into_owned()).unwrap();
    let result = registry
        .execute_search_code(
            &format!(r#"{{"pattern":"foo","path":{path}}}"#),
            "test-session",
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("安全违规"));
}

#[tokio::test]
async fn test_search_code_serde_alias_old_payload() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() {}\n").unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    // 旧字段名 query/scope 仍可解析（serde alias 兼容）。
    let scope = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = registry
        .execute_search_code(
            &format!(r#"{{"query":"foo","scope":{scope}}}"#),
            "test-session",
        )
        .await
        .unwrap();
    assert_eq!(result["count"].as_u64(), Some(1));
}

// ── run_tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_run_tests_success() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_run_tests(r#"{"command":"echo hello"}"#, "test-session", false)
        .await
        .unwrap();
    // 设计 D6 新输出形状：success + 计数 + 结构化 failures + grouped_by_file
    assert_eq!(result["success"].as_bool(), Some(true));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
    assert_eq!(result["passed"].as_u64(), Some(0));
    assert_eq!(result["failed"].as_u64(), Some(0));
    assert_eq!(result["ignored"].as_u64(), Some(0));
    assert!(result["failures"].is_array());
    assert!(result["failures"].as_array().unwrap().is_empty());
    assert!(result["grouped_by_file"].is_array());
    assert!(result["stdout"].as_str().unwrap_or("").contains("hello"));
}

#[tokio::test]
async fn test_run_tests_missing_command_unknown_project_errors() {
    // 缺省 command + 无项目探测（空目录）→ 提示提供 command
    let registry = ToolRegistry::new(default_strict_policy());
    let dir = tempfile::tempdir().unwrap();
    let cwd = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = registry
        .execute_run_tests(&format!(r#"{{"cwd":{cwd}}}"#), "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("command"));
}

#[tokio::test]
async fn test_run_tests_rejects_malformed_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_run_tests(r#"{"command":"echo"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_run_tests_framework_suite_filter_backward_compat_command_wins() {
    // 显式 command 优先于 framework/suite/filter（向后兼容，LLM 完全控制）
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_run_tests(
            r#"{"command":"echo hi","framework":"cargo","suite":"x","filter":"y"}"#,
            "test-session",
            false,
        )
        .await
        .unwrap();
    assert_eq!(result["success"].as_bool(), Some(true));
}

/// 全自主审批工作流：黑名单外全放行（与 file_ops_tests 同款；ADR-033）。
fn autonomous_workflow() -> Arc<ApprovalWorkflow> {
    let config = ApprovalWorkflowConfig {
        mode: ApprovalMode::Autonomous,
        ..Default::default()
    };
    Arc::new(ApprovalWorkflow::new(config))
}

#[tokio::test]
async fn test_run_tests_approval_denied_without_confirmation() {
    // 确认模式（ADR-033）：run_tests 命令属 Medium 风险，立即拒绝
    // 并降级为询问用户——与 execute_command 的门控一致。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let result = registry
        .execute_run_tests(r#"{"command":"echo hello"}"#, "test-session", false)
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
    assert!(msg.contains("操作需要用户确认"), "应要求用户确认: {msg}");
}

#[tokio::test]
async fn test_run_tests_approval_approved_unattended() {
    let registry =
        ToolRegistry::new(default_strict_policy()).with_approval_workflow(autonomous_workflow());
    let result = registry
        .execute_run_tests(r#"{"command":"echo hello"}"#, "test-session", false)
        .await;
    let result = result.expect("全自主应批准测试命令");
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_run_tests_command_metacharacters_blocked() {
    // 命令链元字符（&& 等）→ check_command 拦截，不执行。
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_run_tests(
            r#"{"command":"echo hi && echo bye"}"#,
            "test-session",
            false,
        )
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
}

/// 构造绑定指定工作目录的持久化会话管理器（grep 搜索根解析测试用）。
async fn session_manager_with_cwd(
    session_id: &str,
    cwd: &std::path::Path,
) -> Arc<dyn crate::session::SessionManager> {
    let db = crate::db::Database::open_in_memory().unwrap();
    db.init_schemas().await.unwrap();
    let store = crate::session::store::SessionStore::new(db).unwrap();
    let header = crate::session::types::SessionHeader {
        working_directory: Some(cwd.to_string_lossy().into_owned()),
        ..Default::default()
    };
    store.create(session_id, &header).await.unwrap();
    let sm: Arc<dyn crate::session::SessionManager> =
        Arc::new(crate::session::PersistentSessionManager::new(store));
    sm
}

// ── T0-1：resolve_tool_path 词法归一化（判定与执行同一口径）─────────────

#[tokio::test]
async fn test_resolve_tool_path_normalizes_parent_dir_components() {
    // T0-1：相对路径 join 会话工作目录后必须折叠 `.`/`..`——修复前原样
    // 输出 `sub/../target.txt`，下游沙箱前缀匹配与执行语义可能被 `..` 欺骗。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-norm", dir.path()).await);

    let resolved = registry
        .resolve_tool_path("s-norm", "sub/../target.txt")
        .await;

    assert_eq!(
        std::path::Path::new(&resolved),
        dir.path().join("target.txt").as_path(),
        "`..` 必须折叠为词法等价路径"
    );
    assert!(
        !resolved.contains(".."),
        "输出路径不得含 `..` 组件: {resolved}"
    );
}

#[tokio::test]
async fn test_resolve_tool_path_normalizes_absolute_parent_dir() {
    // 绝对路径同样归一化（判定与执行同一口径，含 `..` 的绝对输入不留存）。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-abs", dir.path()).await);

    let abs = dir.path().join("a").join("..").join("b.txt");
    let resolved = registry
        .resolve_tool_path("s-abs", &abs.to_string_lossy())
        .await;

    assert_eq!(
        std::path::Path::new(&resolved),
        dir.path().join("b.txt").as_path()
    );
}

#[tokio::test]
async fn test_search_code_relative_path_resolves_against_session_working_directory() {
    // 回归保护（grep 失效根因）：相对 path 必须按会话工作目录解析——修复前
    // 直接落进程 cwd（Tauri 安装目录），遍历不到目标时静默 0 条假阴性。
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/a.rs"), "fn needle_rel() {}\n").unwrap();

    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-rel", dir.path()).await);
    let result = registry
        .execute_search_code(r#"{"pattern":"needle_rel","path":"sub"}"#, "s-rel")
        .await
        .unwrap();
    assert_eq!(
        result["count"].as_u64(),
        Some(1),
        "相对路径应命中会话工作目录的子目录: {result}"
    );
    assert!(result["results"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("a.rs"));
}

#[tokio::test]
async fn test_search_code_default_path_uses_session_working_directory() {
    // 回归保护：缺省 path 必须落在会话工作目录（修复前落进程 cwd 静默 0 条），
    // 且输出回显解析后的搜索根（透明化，不再无声落错目录）。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn needle_default() {}\n").unwrap();

    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-def", dir.path()).await);
    let result = registry
        .execute_search_code(r#"{"pattern":"needle_default"}"#, "s-def")
        .await
        .unwrap();
    assert_eq!(
        result["count"].as_u64(),
        Some(1),
        "缺省 path 应命中会话工作目录: {result}"
    );
    let expected_root = dir.path().to_string_lossy().into_owned();
    assert_eq!(result["path"].as_str(), Some(expected_root.as_str()));
}

#[tokio::test]
async fn test_run_tests_filter_metacharacters_blocked() {
    // filter 拼接进默认命令（cargo test <filter>）后含命令链元字符（&&）→
    // 对解析后的完整命令执行 check_command 拦截，阻止注入。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    let registry = ToolRegistry::new(default_strict_policy());
    let args = json!({
        "cwd": dir.path().to_string_lossy(),
        "filter": "x\" && del *",
    })
    .to_string();
    let result = registry
        .execute_run_tests(&args, "test-session", false)
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
}

// ── discover_tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_discover_tests_rejects_missing_path() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_discover_tests(r#"{}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_discover_tests_rejects_empty_path() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry.execute_discover_tests(r#"{"path":"  "}"#).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_discover_tests_unknown_project_errors() {
    let registry = ToolRegistry::new(default_strict_policy());
    let dir = tempfile::tempdir().unwrap();
    let path = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = registry
        .execute_discover_tests(&format!(r#"{{"path":{path}}}"#))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("无法识别项目类型"));
}

#[tokio::test]
async fn test_discover_tests_registered_in_definitions() {
    let registry = ToolRegistry::new(default_strict_policy());
    let defs = registry.definitions().await;
    let def = defs
        .iter()
        .find(|d| d.function.name == "discover_tests")
        .expect("discover_tests 应已注册");
    assert!(def.function.description.contains("发现项目中的测试"));
    assert!(def.function.description.contains("不执行测试"));
}

// ── verify_build ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_verify_build_success_fallback() {
    // 未配置 verification_gate 时回退到退出码校验路径。
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_verify_build(r#"{"command":"echo hello"}"#, "test-session", false)
        .await
        .unwrap();
    // 统一输出形状：passed + structured_diagnostics（可能为空数组）+ judge_method
    // （与门控路径一致的字符串标记：echo 无诊断 → pattern 回退）。
    assert_eq!(result["passed"].as_bool(), Some(true));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
    assert!(result["structured_diagnostics"].is_array());
    assert!(result["structured_diagnostics"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(result["judge_method"].as_str(), Some("pattern"));
    assert!(result.get("success").is_none());
}

#[tokio::test]
async fn test_verify_build_rejects_missing_arguments() {
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_verify_build(r#"{}"#, "test-session", false)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("参数无效"));
}

#[tokio::test]
async fn test_verify_build_approval_denied_without_confirmation() {
    // 确认模式（ADR-033）：verify_build 命令属 Medium 风险，立即拒绝。
    let workflow = Arc::new(ApprovalWorkflow::new(ApprovalWorkflowConfig {
        mode: ApprovalMode::Confirm,
        ..Default::default()
    }));
    let registry = ToolRegistry::new(default_strict_policy()).with_approval_workflow(workflow);
    let result = registry
        .execute_verify_build(r#"{"command":"echo hello"}"#, "test-session", false)
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
    assert!(msg.contains("操作需要用户确认"), "应要求用户确认: {msg}");
}

#[tokio::test]
async fn test_verify_build_approval_approved_unattended() {
    let registry =
        ToolRegistry::new(default_strict_policy()).with_approval_workflow(autonomous_workflow());
    let result = registry
        .execute_verify_build(r#"{"command":"echo hello"}"#, "test-session", false)
        .await;
    let result = result.expect("全自主应批准构建命令");
    assert_eq!(result["passed"].as_bool(), Some(true));
}

#[tokio::test]
async fn test_verify_build_command_metacharacters_blocked() {
    // 命令链元字符（&&）→ check_command 拦截，不执行。
    let registry = ToolRegistry::new(default_strict_policy());
    let result = registry
        .execute_verify_build(
            r#"{"command":"echo hi && echo bye"}"#,
            "test-session",
            false,
        )
        .await;
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("安全违规"), "应报安全违规: {msg}");
}

// ── T1-1：run_tests / verify_build 缺省 cwd 归属会话工作目录 ──────────────

#[tokio::test]
async fn test_tool_cwd_defaults_to_session_working_directory() {
    // T1-1：run_tests/verify_build 缺省 cwd 必须是**会话工作目录**——旧实现
    // 落进程 cwd（桌面应用 = 安装目录 → 项目探测与执行都在错误目录）。
    let dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-cwd", dir.path()).await);

    let default = registry.resolve_tool_cwd("s-cwd", None).await.unwrap();
    assert_eq!(
        std::path::Path::new(&default),
        dir.path(),
        "缺省 cwd 必须是会话工作目录（T1-1：旧实现落进程 cwd）"
    );

    // 显式相对 cwd 按会话工作目录解析（与 grep/glob/read_file 同一套规则）
    let rel = registry
        .resolve_tool_cwd("s-cwd", Some("sub"))
        .await
        .unwrap();
    assert_eq!(
        std::path::Path::new(&rel),
        dir.path().join("sub").as_path(),
        "显式相对 cwd 必须按会话工作目录解析"
    );
}

#[tokio::test]
async fn test_tool_cwd_follows_explicit_absolute_cwd() {
    // 显式绝对 cwd 原样使用（不因缺省归属而被改写）。
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-abs-cwd", dir.path()).await);

    let resolved = registry
        .resolve_tool_cwd("s-abs-cwd", Some(&other.path().to_string_lossy()))
        .await
        .unwrap();
    assert_eq!(std::path::Path::new(&resolved), other.path());
}
