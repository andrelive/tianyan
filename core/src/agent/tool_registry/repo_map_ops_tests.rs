//! repo_map 工具执行器测试：路径归属 / 安全门控 / 缓存命中 / focus 与预算。

use std::sync::Arc;

use serde_json::json;

use crate::executor::SecurityPolicy;

use super::*;

/// 默认严格策略：允许任意路径，但阻止常见破坏性命令。
fn default_strict_policy() -> SecurityPolicy {
    SecurityPolicy {
        safety_mode: crate::config::SafetyMode::Strict,
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

/// 构造绑定指定工作目录的会话管理器（扫描根归属测试用）。
async fn session_manager_with_cwd(
    session_id: &str,
    cwd: &Path,
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

fn write_source(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn test_repo_map_uses_session_working_directory_by_default() {
    // 缺省 path = 会话工作目录（与 grep 同归属规则）：不得落到进程 cwd。
    let dir = tempfile::tempdir().unwrap();
    write_source(dir.path(), "src/a.rs", "pub fn alpha_fn() {}\n");
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-map", dir.path()).await);
    let result = registry
        .execute_repo_map("{}", "s-map")
        .await
        .expect("repo_map 应成功");
    assert_eq!(result["files"].as_u64(), Some(1));
    assert!(
        result["map"].as_str().unwrap().contains("alpha_fn"),
        "地图应含符号：{result}"
    );
    assert_eq!(result["cached"].as_bool(), Some(false), "首次应为实际扫描");
    assert_eq!(
        result["root"].as_str(),
        Some(dir.path().to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn test_repo_map_second_call_hits_cache() {
    let dir = tempfile::tempdir().unwrap();
    write_source(dir.path(), "src/a.rs", "pub fn cached_fn() {}\n");
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-cache", dir.path()).await);

    let first = registry.execute_repo_map("{}", "s-cache").await.unwrap();
    assert_eq!(first["cached"].as_bool(), Some(false));
    let second = registry.execute_repo_map("{}", "s-cache").await.unwrap();
    assert_eq!(second["cached"].as_bool(), Some(true), "指纹未变应命中缓存");
    assert_eq!(
        first["symbols"].as_u64(),
        second["symbols"].as_u64(),
        "缓存命中结果应与首次一致"
    );
}

#[tokio::test]
async fn test_repo_map_focus_and_budget_take_effect() {
    let dir = tempfile::tempdir().unwrap();
    write_source(dir.path(), "src/a.rs", "pub fn unrelated_alpha() {}\n");
    write_source(dir.path(), "src/b.rs", "pub fn session_restore() {}\n");
    // 填充符号：让小预算（64 token → 256 字节）必然装不下
    let filler: String = (0..40)
        .map(|i| format!("pub fn filler_{i}() {{}}\n"))
        .collect();
    write_source(dir.path(), "src/filler.rs", &filler);
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-focus", dir.path()).await);

    let result = registry
        .execute_repo_map(r#"{"focus":"session","max_tokens":64}"#, "s-focus")
        .await
        .unwrap();
    let map = result["map"].as_str().unwrap();
    assert!(
        map.lines()
            .next()
            .unwrap_or_default()
            .contains("session_restore"),
        "focus 命中应排首行（即使 filler 数量占优）：{map}"
    );
    assert_eq!(result["truncated"].as_bool(), Some(true), "小预算应截断");
}

#[tokio::test]
async fn test_repo_map_include_tests_toggle() {
    let dir = tempfile::tempdir().unwrap();
    write_source(dir.path(), "src/a.rs", "pub fn prod_fn() {}\n");
    write_source(dir.path(), "src/a_tests.rs", "fn test_helper() {}\n");
    let registry = ToolRegistry::new(default_strict_policy())
        .with_session_manager(session_manager_with_cwd("s-tests", dir.path()).await);

    let excluded = registry.execute_repo_map("{}", "s-tests").await.unwrap();
    assert!(
        !excluded["map"].as_str().unwrap().contains("test_helper"),
        "默认排除测试文件：{excluded}"
    );
    let included = registry
        .execute_repo_map(r#"{"include_tests":true}"#, "s-tests")
        .await
        .unwrap();
    assert!(
        included["map"].as_str().unwrap().contains("test_helper"),
        "include_tests=true 应包含测试符号：{included}"
    );
    assert!(
        included["files"].as_u64() > excluded["files"].as_u64(),
        "开启后扫描文件数应增加"
    );
}

#[tokio::test]
async fn test_repo_map_rejects_path_outside_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let allowed = dir.path().join("allowed");
    std::fs::create_dir(&allowed).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    write_source(&outside, "a.rs", "pub fn secret_fn() {}\n");

    let policy = SecurityPolicy {
        allowed_directories: vec![allowed],
        ..default_strict_policy()
    };
    let registry = ToolRegistry::new(policy);
    let args = json!({ "path": outside.to_string_lossy() }).to_string();
    let err = registry
        .execute_repo_map(&args, "s-sandbox")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("安全违规"), "{err}");
}

#[tokio::test]
async fn test_repo_map_missing_directory_reports_not_found() {
    let registry = ToolRegistry::new(default_strict_policy());
    let args = json!({ "path": "Z:\\definitely-not-here-xyz" }).to_string();
    let err = registry
        .execute_repo_map(&args, "s-missing")
        .await
        .unwrap_err();
    assert!(err.is_not_found(), "应明确报未找到：{err}");
}

#[tokio::test]
async fn test_repo_map_tool_registered_with_approximation_note() {
    let registry = ToolRegistry::new(default_strict_policy());
    let defs = registry.definitions().await;
    let def = defs
        .iter()
        .find(|d| d.function.name == "repo_map")
        .expect("repo_map 应已注册");
    let description = &def.function.description;
    assert!(description.contains("verify_build"), "应指明精确验证路径");
    assert!(description.contains("近似"), "应显式声明引用度为近似");
}
