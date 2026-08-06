//! 测试发现与测试结果解析（discover_tests 工具 + run_tests 结果增强，设计 D6）。
//!
//! 结构分三块：
//! - **列表解析**（纯函数）：`cargo test -- --list` / `pytest --collect-only -q` /
//!   `vitest --list` 输出 → 结构化测试条目，上限 [`MAX_TESTS`] 条。
//! - **结果解析**（纯函数）：[`parse_test_output`] 从 stdout/stderr 提取
//!   passed/failed/ignored、失败详情（消息 + 文件行号 + 回溯头尾）与按文件分组。
//! - **执行入口**（async）：[`discover_tests`] 与 [`run_tests_action`]，
//!   前者只读列出测试，后者解析默认命令模板并运行。

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};

use crate::common::error::TianyanError;
use crate::executor::execute_command_action;
use crate::executor::project::{probe_project, ProjectFormat, ProjectInfo};
use crate::executor::truncate;

/// 测试列表条数上限（超出标记 `truncated`）。
const MAX_TESTS: usize = 500;
/// 失败详情条数上限（超出仅保留前 N 条，统计仍用完整计数）。
const MAX_FAILURES: usize = 20;
/// 回溯头部保留行数。
const BACKTRACE_HEAD: usize = 30;
/// 回溯尾部保留行数（头部 + 尾部之间以 `…(省略 N 行)` 标记）。
const BACKTRACE_TAIL: usize = 20;
/// 内部发现命令（`cargo test -- --list` / `pytest --collect-only` / `vitest --list`）
/// 的显式超时（秒）。
///
/// cargo 必须先编译目标再列出测试，冷构建的中型项目可能远超默认命令超时
/// （[`DEFAULT_COMMAND_TIMEOUT_SECS`] = 30s）——发现命令使用大超时；用户可配置
/// 的 run_tests 正常执行路径保持默认（由 `timeout_secs` 参数控制）。
const DISCOVER_TIMEOUT_SECS: u64 = 300;

/// 单个测试条目（发现结果）。
#[derive(Debug, Clone, Serialize)]
struct TestCase {
    /// 套件名（cargo 为根目录名；pytest/vitest 为测试文件路径）。
    suite: String,
    /// 测试名（cargo 为完整路径如 `tests::test_a`；pytest 为 `::` 最后一段）。
    name: String,
    /// 源文件路径（cargo 列表不提供，恒为空字符串；pytest/vitest 为文件路径）。
    file: String,
    /// 源文件行号（列表输出不提供，恒为 0）。
    line: usize,
}

/// 单个失败测试详情。
#[derive(Debug, Clone)]
pub struct TestCaseFailure {
    /// 失败测试名（来自 `---- name stdout ----` 或 `test name ... FAILED` 行）。
    pub name: String,
    /// 源文件路径（未知时为空字符串）。
    pub file: String,
    /// 源文件行号（未知时为 0）。
    pub line: usize,
    /// 断言消息（Rust 为 `panicked at '...'` 内容；Python 为 `E ...` 行；否则首个非空行）。
    pub message: String,
    /// 回溯头部（前 [`BACKTRACE_HEAD`] 行）。
    pub backtrace_head: String,
    /// 回溯尾部（`…(省略 N 行)` 标记 + 后 [`BACKTRACE_TAIL`] 行；无省略时为空字符串）。
    pub backtrace_tail: String,
}

/// 测试结果汇总。
#[derive(Debug, Clone)]
pub struct TestSummary {
    /// 通过数（来自 `test result:` 行或 pytest 摘要行；无摘要时为 0）。
    pub passed: usize,
    /// 失败数（来自摘要行；无摘要行时退化为失败区块数）。
    pub failed: usize,
    /// 忽略数（cargo `ignored` / pytest `skipped`）。
    pub ignored: usize,
    /// 失败详情（最多 [`MAX_FAILURES`] 条）。
    pub failures: Vec<TestCaseFailure>,
    /// 按文件分组的失败数（未知文件归入 "(unknown)"），按失败数降序、文件名升序。
    pub grouped_by_file: Vec<(String, usize)>,
}

// ── 测试列表解析（纯函数）───────────────────────────────────────────────

/// 解析 `cargo test -- --list` 输出：`<完整路径>: test` 行。
///
/// 忽略 `: bench` 行与汇总行；`suite` 由调用方传入（探测根目录名）。
/// cargo 列表不提供文件/行号，故 `file` 为空、`line` 为 0。
fn parse_cargo_list(output: &str, suite: &str) -> Vec<TestCase> {
    output
        .lines()
        .filter_map(|line| {
            let name = line.trim_end().strip_suffix(": test")?;
            if name.trim().is_empty() {
                return None;
            }
            Some(TestCase {
                suite: suite.to_string(),
                name: name.to_string(),
                file: String::new(),
                line: 0,
            })
        })
        .collect()
}

/// 解析 `pytest --collect-only -q` 输出：`<文件>::<测试名>` 行（含参数化 `[..]` 后缀）。
///
/// 跳过无 `::` 的模块行与汇总行；`suite` 与 `file` 均为文件路径，`line` 为 0。
fn parse_pytest_list(output: &str) -> Vec<TestCase> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (file, name) = line.rsplit_once("::")?;
            if file.trim().is_empty() || name.trim().is_empty() {
                return None;
            }
            Some(TestCase {
                suite: file.to_string(),
                name: name.to_string(),
                file: file.to_string(),
                line: 0,
            })
        })
        .collect()
}

/// 解析 `vitest --list` 输出（尽力而为）：`<文件> > <测试名>` 行，按首个 ` > ` 切分。
///
/// vitest 输出格式不稳定（不同版本可能不同），无法解析的行直接跳过；
/// `suite` 与 `file` 均为文件路径，`line` 为 0。
fn parse_vitest_list(output: &str) -> Vec<TestCase> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (file, name) = line.split_once(" > ")?;
            let file = file.trim();
            let name = name.trim();
            if file.is_empty() || name.is_empty() {
                return None;
            }
            Some(TestCase {
                suite: file.to_string(),
                name: name.to_string(),
                file: file.to_string(),
                line: 0,
            })
        })
        .collect()
}

/// 对解析结果应用条数上限（[`MAX_TESTS`]），返回 `(列表, 是否截断)`。
fn parse_test_list_capped(tests: Vec<TestCase>) -> (Vec<TestCase>, bool) {
    let truncated = tests.len() > MAX_TESTS;
    let mut tests = tests;
    tests.truncate(MAX_TESTS);
    (tests, truncated)
}

// ── 测试结果解析（纯函数）───────────────────────────────────────────────

/// 解析测试运行输出（stdout + stderr 合并）为结构化摘要。
///
/// 计数策略（确定性、尽力而为）：
/// - 优先取 cargo `test result:` 行（多二进制时最后一条生效）；
/// - 无该行时取 pytest 摘要行（`N passed, M failed, K skipped`）；
/// - 两者皆无时 `failed` 退化为失败区块数。
///
/// 失败详情提取：以 `---- name stdout ----`（cargo）或 `____ name ____`（pytest）
/// 区块头为分隔，逐区块提取名称、文件行号、消息与回溯头尾，最多 [`MAX_FAILURES`] 条。
pub fn parse_test_output(stdout: &str, stderr: &str) -> TestSummary {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut ignored = 0usize;
    let mut counts_authoritative = false;

    for line in stdout.lines() {
        if line.contains("test result:") {
            counts_authoritative = true;
            parse_test_result_counts(line, &mut passed, &mut failed, &mut ignored);
        }
    }
    if !counts_authoritative {
        counts_authoritative = parse_pytest_summary(stdout, &mut passed, &mut failed, &mut ignored);
    }

    let blocks = extract_failure_blocks(stdout, stderr);
    let failed_total = if counts_authoritative {
        failed
    } else {
        blocks.len()
    };
    let failures: Vec<TestCaseFailure> = blocks
        .into_iter()
        .take(MAX_FAILURES)
        .map(failure_from_block)
        .collect();
    let grouped_by_file = group_by_file(&failures);

    TestSummary {
        passed,
        failed: failed_total,
        ignored,
        failures,
        grouped_by_file,
    }
}

/// 解析 cargo `test result:` 行（`;` 分段中取 `N passed` / `N failed` / `N ignored`）。
fn parse_test_result_counts(
    line: &str,
    passed: &mut usize,
    failed: &mut usize,
    ignored: &mut usize,
) {
    for part in line.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_suffix(" passed") {
            *passed = parse_leading_number(rest);
        } else if let Some(rest) = part.strip_suffix(" failed") {
            *failed = parse_leading_number(rest);
        } else if let Some(rest) = part.strip_suffix(" ignored") {
            *ignored = parse_leading_number(rest);
        }
    }
}

/// 解析 pytest 摘要行（`N passed, M failed, K skipped`），识别到任一计数返回 true。
fn parse_pytest_summary(
    stdout: &str,
    passed: &mut usize,
    failed: &mut usize,
    ignored: &mut usize,
) -> bool {
    let mut found = false;
    for line in stdout.lines() {
        // pytest 摘要形如 `===== 2 passed, 1 failed in 0.12s =====`（含 " in " 与计数词）
        if !line.contains(" in ")
            || !(line.contains(" passed") || line.contains(" failed") || line.contains(" skipped"))
        {
            continue;
        }
        for part in line.split(',') {
            let part = part.trim();
            if let Some(rest) = part.strip_suffix(" passed") {
                *passed += parse_leading_number(rest);
                found = true;
            } else if let Some(rest) = part.strip_suffix(" failed") {
                *failed += parse_leading_number(rest);
                found = true;
            } else if let Some(rest) = part.strip_suffix(" skipped") {
                *ignored += parse_leading_number(rest);
                found = true;
            }
        }
    }
    found
}

/// 取字符串最后一个空白分隔 token 并解析为数字（失败回退 0）。
///
/// 兼容 `test result: FAILED. 1 passed`（token 前缀）与 `1 failed` 两种形态。
fn parse_leading_number(s: &str) -> usize {
    s.split_whitespace()
        .next_back()
        .and_then(|w| w.parse().ok())
        .unwrap_or(0)
}

/// 失败区块：区块头 + 后续内容行（空行剔除）。
struct FailureBlock {
    name: String,
    lines: Vec<String>,
}

/// 从合并输出中切分失败区块（cargo `---- name stdout ----` / pytest `____ name ____` 头）。
///
/// 结束条件：下一个区块头、`failures:` 摘要、`note: run with` 提示、`test result:` 行。
/// 无区块头时以 `test <name> ... FAILED` 摘要行兜底（去重后建块）。
fn extract_failure_blocks(stdout: &str, stderr: &str) -> Vec<FailureBlock> {
    let mut blocks: Vec<FailureBlock> = Vec::new();
    let mut current: Option<FailureBlock> = None;

    for line in stdout.lines().chain(stderr.lines()) {
        let trimmed = line.trim();
        if let Some(name) = section_header_name(trimmed) {
            push_block(&mut blocks, current.take());
            current = Some(FailureBlock {
                name,
                lines: Vec::new(),
            });
            continue;
        }
        if trimmed.starts_with("failures:")
            || trimmed.starts_with("note: run with")
            || trimmed.starts_with("test result:")
        {
            push_block(&mut blocks, current.take());
            continue;
        }
        if let Some(name) = failed_summary_name(trimmed) {
            // 摘要行兜底：仅当该失败尚无区块且当前不在区块内时建块，避免捕获尾部名称列表
            if current.is_none() && !blocks.iter().any(|b| b.name == name) {
                current = Some(FailureBlock {
                    name,
                    lines: Vec::new(),
                });
            }
            continue;
        }
        if let Some(block) = &mut current {
            if !trimmed.is_empty() {
                block.lines.push(line.to_string());
            }
        }
    }
    push_block(&mut blocks, current.take());
    blocks
}

/// 区块头名称：`---- name stdout ----`（cargo）或 `____ name ____`（pytest）。
fn section_header_name(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("---- ") {
        if let Some(inner) = rest.strip_suffix(" ----") {
            let name = inner
                .strip_suffix(" stdout")
                .or_else(|| inner.strip_suffix(" stderr"))
                .unwrap_or(inner)
                .trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
        return None;
    }
    if line.starts_with("____") && line.ends_with("____") {
        let name = line.trim_matches('_').trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// cargo 失败摘要行名称：`test <name> ... FAILED`。
fn failed_summary_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("test ")?;
    let name = rest
        .strip_suffix("... FAILED")
        .or_else(|| rest.strip_suffix(" FAILED"))?
        .trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// 将已收尾的区块压入列表：相邻同名区块合并（`test ... FAILED` 摘要行兜底建块后，
/// 同名 `---- name stdout ----` 区块头会把内容合并进同一区块，避免重复计数）。
fn push_block(blocks: &mut Vec<FailureBlock>, block: Option<FailureBlock>) {
    if let Some(block) = block {
        if let Some(last) = blocks.last_mut() {
            if last.name == block.name {
                last.lines.extend(block.lines);
                return;
            }
        }
        blocks.push(block);
    }
}

/// 从区块内容提取失败详情：文件行号 → 消息 → 回溯头尾。
fn failure_from_block(block: FailureBlock) -> TestCaseFailure {
    let (file, line) = locate_file_line(&block.lines);
    let message = extract_message(&block.lines);
    let (backtrace_head, backtrace_tail) = split_backtrace(&block.lines);
    TestCaseFailure {
        name: block.name,
        file,
        line,
        message,
        backtrace_head,
        backtrace_tail,
    }
}

/// 从区块行中定位首个文件行号线索：
/// Python `  File "...", line N, ...` → Rust `panicked at '...', path:N:C` → pytest `path:N: in fn`。
fn locate_file_line(lines: &[String]) -> (String, usize) {
    for line in lines {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("File \"") {
            if let Some(path) = rest.split('"').next() {
                if let Some(line_part) = t.split(", line ").nth(1) {
                    if let Some(n) = line_part
                        .split(',')
                        .next()
                        .and_then(|s| s.trim().parse::<usize>().ok())
                    {
                        return (path.to_string(), n);
                    }
                }
            }
        }
        if t.contains("panicked at '") {
            // 取最后一个引号后的定位片段（消息内可能含引号，从右解析更稳）
            if let Some(after_msg) = t.rsplit('\'').next() {
                if let Some(loc) = after_msg.strip_prefix(", ") {
                    let mut parts = loc.trim().rsplitn(3, ':');
                    let _col = parts.next();
                    if let (Some(line_s), Some(path)) = (parts.next(), parts.next()) {
                        if let Ok(n) = line_s.parse::<usize>() {
                            return (path.to_string(), n);
                        }
                    }
                }
            }
        }
        if t.contains(": in ") {
            let mut parts = t.rsplitn(3, ':');
            let _fn = parts.next();
            if let (Some(line_s), Some(path)) = (parts.next(), parts.next()) {
                if let Ok(n) = line_s.parse::<usize>() {
                    return (path.to_string(), n);
                }
            }
        }
    }
    (String::new(), 0)
}

/// 提取断言消息：Rust `panicked at '...'` 内容 → pytest `E ...` 行 → 首个非空行。
fn extract_message(lines: &[String]) -> String {
    for line in lines {
        if line.contains("panicked at '") {
            if let Some(after) = line.split("panicked at '").nth(1) {
                if let Some(msg) = after.split('\'').next() {
                    let msg = msg.trim();
                    if !msg.is_empty() {
                        return msg.to_string();
                    }
                }
            }
        }
    }
    for line in lines {
        let t = line.trim_start();
        let rest = t
            .strip_prefix("E ")
            .or_else(|| t.strip_prefix("E\t"))
            .map(str::trim)
            .filter(|m| !m.is_empty());
        if let Some(msg) = rest {
            return msg.to_string();
        }
    }
    for line in lines {
        let m = line.trim();
        if !m.is_empty() {
            return m.to_string();
        }
    }
    String::new()
}

/// 回溯行判定：Python `  File "..."` / Rust `    at path:N:C` / pytest `path:N: in fn`。
fn is_backtrace_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("File \"")
        || (t.starts_with("at ") && t.contains(':'))
        || (t.contains(": in ") && t.contains(':'))
}

/// 切分回溯为头部/尾部（中间以省略标记连接至尾部字段）。
fn split_backtrace(lines: &[String]) -> (String, String) {
    let backtrace: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| is_backtrace_line(l))
        .collect();
    if backtrace.len() <= BACKTRACE_HEAD + BACKTRACE_TAIL {
        return (backtrace.join("\n"), String::new());
    }
    let head = backtrace[..BACKTRACE_HEAD].join("\n");
    let skipped = backtrace.len() - BACKTRACE_HEAD - BACKTRACE_TAIL;
    let tail = format!(
        "…(省略 {skipped} 行)\n{}",
        backtrace[backtrace.len() - BACKTRACE_TAIL..].join("\n")
    );
    (head, tail)
}

/// 按文件分组失败数（未知文件归入 "(unknown)"），按失败数降序、文件名升序。
fn group_by_file(failures: &[TestCaseFailure]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for f in failures {
        let key = if f.file.is_empty() {
            "(unknown)"
        } else {
            &f.file
        };
        *counts.entry(key.to_string()).or_insert(0) += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

// ── 命令解析（纯函数）───────────────────────────────────────────────────

/// 按项目格式解析默认测试命令模板。
///
/// - Cargo → `cargo test`（`suite` → `-p <suite>`；`filter` → 位置参数）
/// - Python → `pytest`（`suite` → 文件路径位置参数；`filter` → `-k <filter>`）
/// - TypeScript → `vitest run`（`suite` / `filter` → 位置参数）
/// - Unknown → Err，提示提供 `command`
pub fn resolve_test_command(
    info: &ProjectInfo,
    filter: Option<&str>,
    suite: Option<&str>,
) -> Result<Vec<String>, TianyanError> {
    let mut cmd: Vec<String> = match info.format {
        ProjectFormat::Cargo => vec!["cargo".to_string(), "test".to_string()],
        ProjectFormat::Python => vec!["pytest".to_string()],
        ProjectFormat::TypeScript => vec!["vitest".to_string(), "run".to_string()],
        ProjectFormat::Unknown => {
            return Err(TianyanError::Custom(
                "executor: run_tests: 无法识别项目类型，请提供 command".to_string(),
            ));
        }
    };
    if let Some(suite) = suite {
        if info.format == ProjectFormat::Cargo {
            cmd.push("-p".to_string());
        }
        cmd.push(suite.to_string());
    }
    if let Some(filter) = filter {
        if info.format == ProjectFormat::Python {
            cmd.push("-k".to_string());
        }
        cmd.push(filter.to_string());
    }
    Ok(cmd)
}

/// 应用 `framework` 参数对探测格式的覆盖："cargo" / "pytest" / "vitest" 显式指定，
/// "auto" 与未知值保持探测结果不变。
fn apply_framework_override(info: &mut ProjectInfo, framework: Option<&str>) {
    match framework.map(str::trim) {
        Some("cargo") => info.format = ProjectFormat::Cargo,
        Some("pytest") => info.format = ProjectFormat::Python,
        Some("vitest") => info.format = ProjectFormat::TypeScript,
        _ => {}
    }
}

// ── 执行入口（async）────────────────────────────────────────────────────

/// 发现项目测试：探测项目格式 → 运行只读列出命令 → 结构化返回。
///
/// - Cargo → `cargo test -- --list`，`suite` 取根目录名，`name` 为完整路径
/// - Python → `pytest --collect-only -q`，`suite`/`file` 为文件路径
/// - TypeScript → `vitest --list`（尽力而为解析）
/// - Unknown → Err `无法识别项目类型`
///
/// 返回 `{ framework, root, tests: [{suite, name, file, line}], count, truncated }`，
/// 列表超过 [`MAX_TESTS`] 条时截断并标记 `truncated`。只读命令，无安全策略依赖。
pub async fn discover_tests(path: &str) -> Result<Value, TianyanError> {
    let info = probe_project(Path::new(path))?;
    let root = info.root.to_string_lossy().into_owned();

    let (framework, tests) = match info.format {
        ProjectFormat::Cargo => {
            let output = execute_command_action(
                "cargo test -- --list",
                Some(&root),
                Some(DISCOVER_TIMEOUT_SECS),
            )
            .await?;
            let stdout = output["stdout"].as_str().unwrap_or("");
            let suite = info
                .root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            ("cargo", parse_cargo_list(stdout, &suite))
        }
        ProjectFormat::Python => {
            let output = execute_command_action(
                "pytest --collect-only -q",
                Some(&root),
                Some(DISCOVER_TIMEOUT_SECS),
            )
            .await?;
            let stdout = output["stdout"].as_str().unwrap_or("");
            ("pytest", parse_pytest_list(stdout))
        }
        ProjectFormat::TypeScript => {
            let output =
                execute_command_action("vitest --list", Some(&root), Some(DISCOVER_TIMEOUT_SECS))
                    .await?;
            let stdout = output["stdout"].as_str().unwrap_or("");
            ("vitest", parse_vitest_list(stdout))
        }
        ProjectFormat::Unknown => {
            return Err(TianyanError::Custom(
                "executor: discover_tests: 无法识别项目类型（需要 Cargo.toml / pyproject.toml / tsconfig.json）"
                    .to_string(),
            ));
        }
    };

    let (tests, truncated) = parse_test_list_capped(tests);
    let count = tests.len();
    Ok(json!({
        "framework": framework,
        "root": root,
        "tests": tests,
        "count": count,
        "truncated": truncated,
    }))
}

/// 解析 run_tests 实际执行的命令字符串（显式 `command` 优先原样使用；缺省时
/// 探测 `cwd`（默认当前目录）项目格式，按 [`resolve_test_command`] 构建默认
/// 命令模板，`framework` 可覆盖探测结果）。
///
/// 供 registry 层（code_ops）在安全门控（check_command）之前复用同一解析逻辑，
/// 保证"门控的命令"与"实际执行的命令"完全一致。
pub(crate) fn resolve_run_tests_command(
    command: Option<&str>,
    cwd: Option<&str>,
    framework: Option<&str>,
    filter: Option<&str>,
    suite: Option<&str>,
) -> Result<String, TianyanError> {
    match command {
        Some(cmd) if !cmd.trim().is_empty() => Ok(cmd.trim().to_string()),
        _ => {
            let mut info = probe_project(Path::new(cwd.unwrap_or(".")))?;
            apply_framework_override(&mut info, framework);
            resolve_test_command(&info, filter, suite).map(|parts| parts.join(" "))
        }
    }
}

/// 运行测试：解析命令 → 执行 → 解析结果（设计 D6 增强）。
///
/// 命令解析规则（`command` 缺省时，向后兼容：显式 `command` 优先原样使用）：
/// 探测 `cwd`（默认当前目录）项目格式，按 [`resolve_test_command`] 构建命令；
/// `framework` 可覆盖探测结果（"auto" 默认 / "cargo" / "pytest" / "vitest"）。
///
/// 返回 `{ success, passed, failed, ignored, failures: [{name, file, line, message,
/// backtrace_head, backtrace_tail}], grouped_by_file, stdout, stderr, exit_code }`。
pub async fn run_tests_action(
    command: Option<&str>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    framework: Option<&str>,
    filter: Option<&str>,
    suite: Option<&str>,
) -> Result<Value, TianyanError> {
    let resolved = resolve_run_tests_command(command, cwd, framework, filter, suite)?;

    let output = execute_command_action(&resolved, cwd, timeout_secs).await?;
    let stdout = output["stdout"].as_str().unwrap_or("").to_string();
    let stderr = output["stderr"].as_str().unwrap_or("").to_string();
    let exit_code = output["exit_code"].as_i64().unwrap_or(-1);

    // 先解析完整输出，再截断展示字段（解析不依赖截断）
    let summary = parse_test_output(&stdout, &stderr);
    let out_trunc = truncate::truncate_tail(&stdout);
    let err_trunc = truncate::truncate_tail(&stderr);

    let failures: Vec<Value> = summary
        .failures
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "file": f.file,
                "line": f.line,
                "message": f.message,
                "backtrace_head": f.backtrace_head,
                "backtrace_tail": f.backtrace_tail,
            })
        })
        .collect();
    let grouped: Vec<Value> = summary
        .grouped_by_file
        .iter()
        .map(|(file, count)| json!([file, count]))
        .collect();

    Ok(json!({
        "success": exit_code == 0,
        "passed": summary.passed,
        "failed": summary.failed,
        "ignored": summary.ignored,
        "failures": failures,
        "grouped_by_file": grouped,
        "stdout": out_trunc.text,
        "stderr": err_trunc.text,
        "exit_code": exit_code,
    }))
}

#[cfg(test)]
#[path = "test_discovery_tests.rs"]
mod tests;
