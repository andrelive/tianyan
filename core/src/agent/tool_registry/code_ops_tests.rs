//! 代码类工具执行器测试：grep（内嵌搜索引擎）。

use std::sync::Arc;

use crate::config::SafetyMode;
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

#[tokio::test]
async fn test_search_code_invalid_output_mode_rejected_at_parse() {
    // output_mode 现为类型化枚举：非法值在**参数解析期**即被拒（旧形态由执行器
    // 运行期字符串解析报「输出模式无效」），错误消息列出合法取值。
    let err = ToolRegistry::new(default_strict_policy())
        .execute_search_code(r#"{"pattern":"foo","output_mode":"lines"}"#, "test-session")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("参数无效"), "{err}");
    assert!(err.to_string().contains("content"), "{err}");
}

#[tokio::test]
async fn test_search_code_content_mode_always_reports_line_number() {
    // 形态收敛：line_number 字段已删（默认 true，且每次调用都被显式传 true——
    // 零信息量）。content 模式**恒**输出行号；旧字段被忽略且不改变行为。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() {}\n").unwrap();
    let path = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = ToolRegistry::new(default_strict_policy())
        .execute_search_code(
            &format!(
                r#"{{"pattern":"foo","path":{path},"output_mode":"content","line_number":false}}"#
            ),
            "test-session",
        )
        .await
        .unwrap();
    assert_eq!(result["output_mode"].as_str(), Some("content"));
    assert_eq!(result["results"][0]["line_number"].as_u64(), Some(1));
}

#[tokio::test]
async fn test_search_code_context_param_is_ignored() {
    // 输出治理（2026-09-29）：context 已删——旧载荷传入被忽略且不改变行为：
    // content 恒为单行预览（不含上下文行）。判别力：旧实现下 text 会含第 2 行。
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() {\n    let x = 1;\n}\n").unwrap();
    let path = serde_json::to_string(&dir.path().to_string_lossy().into_owned()).unwrap();
    let result = ToolRegistry::new(default_strict_policy())
        .execute_search_code(
            &format!(r#"{{"pattern":"foo","path":{path},"output_mode":"content","context":5}}"#),
            "test-session",
        )
        .await
        .unwrap();
    let text = result["results"][0]["text"].as_str().unwrap();
    assert_eq!(text, "fn foo() {", "应为单行预览（context 被忽略）");
    assert!(!text.contains("let x = 1;"), "不得携带上下文行: {text}");
}

#[tokio::test]
async fn test_grep_schema_drops_legacy_fields_and_types_output_mode() {
    // 判别力：四个历史字段必须不在 schema（旧形态都在）；output_mode 必须是枚举
    // （退回 String 即失败——schema 里不会出现这三个取值）。
    let defs = ToolRegistry::new(default_strict_policy())
        .definitions()
        .await;
    let grep = defs
        .iter()
        .find(|d| d.function.name == "grep")
        .expect("grep 工具已注册");
    let schema = grep.function.parameters.to_string();
    for dropped in ["line_number", "before_context", "after_context", "context"] {
        assert!(
            !schema.contains(&format!("\"{dropped}\"")),
            "{dropped} 不应出现在 schema：{schema}"
        );
    }
    for mode in ["files_with_matches", "content", "count"] {
        assert!(
            schema.contains(&format!("\"{mode}\"")),
            "output_mode 缺取值 {mode}：{schema}"
        );
    }
}

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
