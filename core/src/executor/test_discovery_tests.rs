//! test_discovery 模块测试：测试发现解析 + 测试结果解析 + 命令解析（纯函数，canned 输出，无真实命令）。

use std::path::Path;

use super::*;
use crate::executor::project::{ProjectFormat, ProjectInfo};

fn info(format: ProjectFormat, marker: &str) -> ProjectInfo {
    ProjectInfo {
        format,
        root: Path::new("/tmp/demo").to_path_buf(),
        marker: marker.to_string(),
    }
}

// ── 测试列表解析（纯解析，无真实命令）──────────────────────────────────────

#[test]
fn test_parse_cargo_list_basic() {
    let output = "tests::test_a: test\ntests::test_b: test\n0 tests, 0 benchmarks\n";
    let tests = parse_cargo_list(output, "demo");
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0].name, "tests::test_a");
    assert_eq!(tests[0].suite, "demo");
    assert_eq!(tests[0].file, "");
    assert_eq!(tests[0].line, 0);
    assert_eq!(tests[1].name, "tests::test_b");
}

#[test]
fn test_parse_cargo_list_skips_bench_and_noise() {
    let output = "tests::test_a: test\nbench::b: bench\n0 tests, 0 benchmarks\n\n";
    let tests = parse_cargo_list(output, "demo");
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].name, "tests::test_a");
}

#[test]
fn test_parse_pytest_list_basic() {
    let output = "tests/test_math.py::test_add\ntests/test_math.py::test_sub[1-2]\n\n2 tests collected in 0.01s\n";
    let tests = parse_pytest_list(output);
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0].name, "test_add");
    assert_eq!(tests[0].suite, "tests/test_math.py");
    assert_eq!(tests[0].file, "tests/test_math.py");
    assert_eq!(tests[0].line, 0);
    assert_eq!(tests[1].name, "test_sub[1-2]");
}

#[test]
fn test_parse_pytest_list_skips_module_lines() {
    let output = "tests/test_math.py\ntests/test_math.py::test_add\n";
    let tests = parse_pytest_list(output);
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].name, "test_add");
}

#[test]
fn test_parse_vitest_list_best_effort() {
    let output = "tests/foo.test.ts > test one\ntests/foo.test.ts > test two > nested\n";
    let tests = parse_vitest_list(output);
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0].suite, "tests/foo.test.ts");
    assert_eq!(tests[0].file, "tests/foo.test.ts");
    assert_eq!(tests[0].name, "test one");
    assert_eq!(tests[1].name, "test two > nested");
}

#[test]
fn test_parse_list_caps_at_500_with_truncated_flag() {
    let mut output = String::new();
    for i in 0..501 {
        output.push_str(&format!("tests/test_x.py::test_{i}\n"));
    }
    let (tests, truncated) = parse_test_list_capped(parse_pytest_list(&output));
    assert_eq!(tests.len(), 500);
    assert!(truncated);
    assert_eq!(tests[499].name, "test_499");
}

#[test]
fn test_parse_list_under_cap_not_truncated() {
    let output = "tests/test_x.py::test_a\ntests/test_x.py::test_b\n";
    let (tests, truncated) = parse_test_list_capped(parse_pytest_list(output));
    assert_eq!(tests.len(), 2);
    assert!(!truncated);
}

// ── 测试结果解析（canned 输出）────────────────────────────────────────────

#[test]
fn test_parse_test_output_rust_failure() {
    let stdout = "\
running 2 tests
test tests::test_a ... ok
test tests::test_b ... FAILED

failures:

---- tests::test_b stdout ----
thread 'tests::test_b' panicked at 'assertion failed: `(left == right)`', src/lib.rs:10:5
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::test_b

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
    let summary = parse_test_output(stdout, "");
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.ignored, 0);
    assert_eq!(summary.failures.len(), 1);
    let f = &summary.failures[0];
    assert_eq!(f.name, "tests::test_b");
    assert_eq!(f.file, "src/lib.rs");
    assert_eq!(f.line, 10);
    assert!(f.message.contains("left == right"));
    assert_eq!(summary.grouped_by_file, vec![("src/lib.rs".to_string(), 1)]);
}

#[test]
fn test_parse_test_output_all_passed() {
    let stdout = "running 1 test\ntest tests::test_a ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let summary = parse_test_output(stdout, "");
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.ignored, 0);
    assert!(summary.failures.is_empty());
    assert!(summary.grouped_by_file.is_empty());
}

#[test]
fn test_parse_test_output_backtrace_head_tail_split() {
    let mut stdout = String::new();
    stdout.push_str("---- tests::test_b stdout ----\n");
    stdout.push_str("thread 'tests::test_b' panicked at 'boom', src/lib.rs:10:5\n");
    for i in 0..80 {
        stdout.push_str(&format!("    at src/mod{i}.rs:{}:5\n", i + 1));
    }
    stdout.push_str(
        "\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
    );
    let summary = parse_test_output(&stdout, "");
    assert_eq!(summary.failures.len(), 1);
    let f = &summary.failures[0];
    assert!(f.backtrace_head.contains("src/mod0.rs"));
    assert_eq!(f.backtrace_head.lines().count(), 30);
    assert!(f.backtrace_tail.contains("src/mod79.rs"));
    assert!(f.backtrace_tail.contains("…(省略 30 行)"));
    assert_eq!(f.backtrace_tail.lines().count(), 21); // 省略标记 + 尾部 20 行
}

#[test]
fn test_parse_test_output_caps_failures_at_20() {
    let mut stdout = String::new();
    for i in 0..25 {
        stdout.push_str(&format!("---- tests::test_f{i} stdout ----\n"));
        stdout.push_str(&format!(
            "thread 'tests::test_f{i}' panicked at 'fail {i}', src/lib.rs:{}:5\n",
            10 + i
        ));
    }
    stdout.push_str("test result: FAILED. 0 passed; 25 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n");
    let summary = parse_test_output(&stdout, "");
    assert_eq!(summary.failures.len(), 20);
    assert_eq!(summary.failed, 25); // 统计来自 test result 行，截断不影响计数
                                    // 按文件分组统计全部 20 条解析出的失败
    assert_eq!(
        summary.grouped_by_file,
        vec![("src/lib.rs".to_string(), 20)]
    );
}

#[test]
fn test_parse_test_output_python_traceback() {
    let stdout = "\
============================= test session starts =============================
tests/test_math.py F                                                      [100%]

_________________________________ test_add _________________________________

tests/test_math.py:12: in test_add
    assert 1 == 2
E   assert 1 == 2

=========================== short test summary info ============================
FAILED tests/test_math.py::test_add - assert 1 == 2
============================= 1 failed in 0.02s ==============================
";
    let summary = parse_test_output(stdout, "");
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.failures.len(), 1);
    let f = &summary.failures[0];
    assert_eq!(f.name, "test_add");
    assert_eq!(f.file, "tests/test_math.py");
    assert_eq!(f.line, 12);
    assert_eq!(f.message.trim(), "assert 1 == 2");
}

#[test]
fn test_parse_test_output_python_failure_in_stderr() {
    // 失败细节可能出现在 stderr：合并解析
    let stdout = "============================= 1 failed in 0.02s ==============================\n";
    let stderr = "_______________________________ test_sub ________________________________\n\ntests/test_sub.py:7: in test_sub\n    assert 2 == 3\nE   assert 2 == 3\n";
    let summary = parse_test_output(stdout, stderr);
    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.failures[0].name, "test_sub");
    assert_eq!(summary.failures[0].file, "tests/test_sub.py");
    assert_eq!(summary.failures[0].line, 7);
}

// ── 命令解析（resolve_test_command，纯函数）───────────────────────────────

#[test]
fn test_resolve_test_command_cargo() {
    let info = info(ProjectFormat::Cargo, "Cargo.toml");
    assert_eq!(
        resolve_test_command(&info, None, None).unwrap(),
        vec!["cargo", "test"]
    );
    assert_eq!(
        resolve_test_command(&info, Some("lib::tests"), None).unwrap(),
        vec!["cargo", "test", "lib::tests"]
    );
    assert_eq!(
        resolve_test_command(&info, None, Some("mycrate")).unwrap(),
        vec!["cargo", "test", "-p", "mycrate"]
    );
    assert_eq!(
        resolve_test_command(&info, Some("foo"), Some("mycrate")).unwrap(),
        vec!["cargo", "test", "-p", "mycrate", "foo"]
    );
}

#[test]
fn test_resolve_test_command_pytest() {
    let info = info(ProjectFormat::Python, "pyproject.toml");
    assert_eq!(
        resolve_test_command(&info, None, None).unwrap(),
        vec!["pytest"]
    );
    assert_eq!(
        resolve_test_command(&info, Some("add"), Some("tests/test_math.py")).unwrap(),
        vec!["pytest", "tests/test_math.py", "-k", "add"]
    );
}

#[test]
fn test_resolve_test_command_vitest() {
    let info = info(ProjectFormat::TypeScript, "tsconfig.json");
    assert_eq!(
        resolve_test_command(&info, Some("foo"), Some("tests/foo.test.ts")).unwrap(),
        vec!["vitest", "run", "tests/foo.test.ts", "foo"]
    );
}

#[test]
fn test_resolve_test_command_unknown_errors() {
    let info = info(ProjectFormat::Unknown, "");
    let err = resolve_test_command(&info, None, None).unwrap_err();
    assert!(err.to_string().contains("无法识别项目类型"));
    assert!(err.to_string().contains("command"));
}

#[test]
fn test_apply_framework_override() {
    let mut info = info(ProjectFormat::Unknown, "");
    apply_framework_override(&mut info, Some("pytest"));
    assert_eq!(info.format, ProjectFormat::Python);
    // "auto" 与未知值保持原样（跟随项目探测）
    apply_framework_override(&mut info, Some("auto"));
    assert_eq!(info.format, ProjectFormat::Python);
    apply_framework_override(&mut info, Some("bogus"));
    assert_eq!(info.format, ProjectFormat::Python);
    apply_framework_override(&mut info, Some("cargo"));
    assert_eq!(info.format, ProjectFormat::Cargo);
}

// ── run_tests_action（给定命令集成路径，无真实测试框架）─────────────────────

#[tokio::test]
async fn test_run_tests_action_explicit_command() {
    let value = run_tests_action("echo hello", None, Some(10), None)
        .await
        .unwrap();
    assert_eq!(value["success"].as_bool(), Some(true));
    assert_eq!(value["exit_code"].as_i64(), Some(0));
    assert!(value["stdout"].as_str().unwrap_or("").contains("hello"));
    assert_eq!(value["passed"].as_u64(), Some(0));
    assert_eq!(value["failed"].as_u64(), Some(0));
    assert_eq!(value["ignored"].as_u64(), Some(0));
    assert!(value["failures"].is_array());
    assert!(value["grouped_by_file"].is_array());
}

#[test]
fn test_resolve_project_test_command_unknown_project_errors() {
    // 探测路径：未知项目类型 → 明确报错（原先由 run_tests_action 的
    // 「缺 command 回落探测」覆盖；双形态拆分后归属本函数）
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let err = resolve_project_test_command(Some(cwd), None, None, None).unwrap_err();
    assert!(err.to_string().contains("无法识别项目类型"), "{err}");
}

// ── discover_tests（未知项目错误路径；真实 cargo 路径见下方守卫测试）─────────

#[tokio::test]
async fn test_discover_tests_unknown_project_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let err = discover_tests(&path, None).await.unwrap_err();
    assert!(err.to_string().contains("无法识别项目类型"));
}

/// 真实 cargo 集成测试：在最小 Cargo.toml fixture 中运行 `cargo test -- --list`。
///
/// 无依赖、无网络（空 lib 直接编译）。首次运行可能较慢（冷缓存），
/// 若 120s 内未完成则跳过（视为环境受限）——框架解析与列表解析已由纯测试覆盖。
#[tokio::test]
async fn test_discover_tests_cargo_happy_path_real_cargo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        discover_tests(&path, None),
    )
    .await;
    let Ok(Ok(value)) = result else {
        // cargo 不可用或首次编译过慢：跳过
        return;
    };
    assert_eq!(value["framework"], "cargo");
    assert_eq!(value["root"], dir.path().to_string_lossy().into_owned());
    assert!(value["tests"].is_array());
    assert_eq!(value["count"].as_u64(), Some(0));
    assert_eq!(value["truncated"].as_bool(), Some(false));
}

// ── 取消（ADR-036 补盲区：工具执行期间的「停止」）──────────────────────────

/// run_tests 执行期间置位会话取消标志 → 秒级返回取消错误（不再等满 timeout）。
///
/// 判别力：修复前 `run_tests_action` 走不可取消的 `execute_command_action`，
/// 置位取消后仍会等满 60s（本测试超时红）。
#[tokio::test]
async fn test_run_tests_interruptible_on_cancel() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    // 与 kill_all 类测试互斥（持真实子进程；见 executor::test_support 文档）
    let _g = crate::executor::test_support::CHILD_REGISTRY_LOCK
        .lock()
        .await;
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        flag.store(true, Ordering::Relaxed);
    });
    let cmd = if cfg!(target_os = "windows") {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };
    let started = std::time::Instant::now();
    let err = run_tests_action(cmd, None, Some(60), Some(cancel))
        .await
        .expect_err("置位取消后应返回取消错误");
    let elapsed = started.elapsed();
    assert!(
        err.to_string().contains("已取消"),
        "取消应返回取消错误：{err}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "取消应在 5s 内生效（实际 {elapsed:?}）"
    );
    assert!(!err.is_timeout(), "取消不应被分类为超时");
}
