//! 验证门控模块。
//!
//! 将命令执行与 LLM-as-Judge 语义验证结合起来，
//! 为 `verify_build` 工具提供深度质量门控。
//!
//! 判定优先级：结构化诊断（cargo JSON 输出）→ LLM 语义判断 → 模式匹配回退。

use crate::common::error::TianyanError;
use crate::executor::actions::execute_command_action;
use crate::executor::judge::LlmJudge;
use crate::executor::output_parse::extract_build_errors;
use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::Message;
use serde_json::json;

/// 结构化诊断数量上限（超出部分丢弃）。
const MAX_DIAGNOSTICS: usize = 100;
/// 单条诊断消息字符上限（按字符截断，UTF-8 安全）。
const MAX_DIAGNOSTIC_CHARS: usize = 2000;

/// 结构化诊断信息（从 cargo `--message-format=json` 输出解析）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StructuredDiagnostic {
    /// 源文件路径。
    pub file: String,
    /// 起始行（1 起始）。
    pub line: usize,
    /// 起始列（1 起始）。
    pub column: usize,
    /// 诊断级别（"error" / "warning" / "note" / "help" 等）。
    pub level: String,
    /// 错误码（如 "E0308"）。
    pub code: Option<String>,
    /// 顶层诊断消息。
    pub message: String,
    /// 修复建议（首个 help 子诊断的文本）。
    pub suggestion: Option<String>,
}

/// 验证结果。
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// 是否通过验证。
    pub passed: bool,
    /// 退出码。
    pub exit_code: i32,
    /// 判断理由。
    pub reason: String,
    /// 修复建议（仅当 verdict 为 NeedsChanges 时有值）。
    pub suggestion: Option<String>,
    /// 标准输出。
    pub stdout: String,
    /// 标准错误输出。
    pub stderr: String,
    /// 判断方式：json_diagnostics / pattern / llm。
    pub judge_method: String,
    /// 结构化诊断（从 cargo JSON 输出解析，可能为空）。
    pub structured_diagnostics: Vec<StructuredDiagnostic>,
}

/// 从 cargo JSON 输出中解析结构化诊断。
///
/// 使用 `cargo_metadata::Message::parse_stream` 逐行解析：非 JSON 行由
/// `parse_stream` 回退为 `TextLine` 消息，与其它非 compiler-message 消息一起
/// 被静默跳过（仅底层读取错误产生 `Err`，同样跳过）。最多保留
/// [`MAX_DIAGNOSTICS`] 条，单条消息截断到 [`MAX_DIAGNOSTIC_CHARS`] 字符。
pub fn parse_json_diagnostics(stdout: &str) -> Vec<StructuredDiagnostic> {
    Message::parse_stream(stdout.as_bytes())
        .filter_map(|r| r.ok())
        .filter_map(|msg| match msg {
            Message::CompilerMessage(cm) => Some(map_diagnostic(cm.message)),
            _ => None,
        })
        .take(MAX_DIAGNOSTICS)
        .collect()
}

/// 将 cargo_metadata 的 Diagnostic 映射为结构化诊断。
fn map_diagnostic(diag: cargo_metadata::diagnostic::Diagnostic) -> StructuredDiagnostic {
    let (file, line, column) = diag
        .spans
        .first()
        .map(|s| (s.file_name.clone(), s.line_start, s.column_start))
        .unwrap_or_else(|| (String::new(), 0, 0));
    let suggestion = diag
        .children
        .iter()
        .find(|c| matches!(c.level, DiagnosticLevel::Help) && !c.message.trim().is_empty())
        .map(|c| truncate_chars(&c.message, MAX_DIAGNOSTIC_CHARS));
    let code = diag.code.map(|c| c.code);
    let message = truncate_chars(&diag.message, MAX_DIAGNOSTIC_CHARS);
    StructuredDiagnostic {
        file,
        line,
        column,
        level: diagnostic_level_str(diag.level),
        code,
        message,
        suggestion,
    }
}

/// 将 cargo_metadata 的 DiagnosticLevel 转为字符串。
///
/// `DiagnosticLevel` 为 `#[non_exhaustive]`，未知级别回退为 "unknown"。
fn diagnostic_level_str(level: DiagnosticLevel) -> String {
    match level {
        DiagnosticLevel::Ice => "error: internal compiler error",
        DiagnosticLevel::Error => "error",
        DiagnosticLevel::Warning => "warning",
        DiagnosticLevel::FailureNote => "failure-note",
        DiagnosticLevel::Note => "note",
        DiagnosticLevel::Help => "help",
        _ => "unknown",
    }
    .to_string()
}

/// 诊断级别字符串是否表示错误。
pub(crate) fn is_error_level(level: &str) -> bool {
    level == "error" || level == "error: internal compiler error"
}

/// 按字符截断字符串（UTF-8 安全，不产生字节边界 panic）。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// 验证门控 — 执行命令并对输出进行语义判断。
#[derive(Clone)]
pub struct VerificationGate {
    judge: Option<LlmJudge>,
}

impl VerificationGate {
    /// 创建一个新的验证门控。
    ///
    /// 如果提供了 `LlmJudge`，则使用语义判断；
    /// 否则回退到退出码 + 关键词匹配。
    pub fn new(judge: Option<LlmJudge>) -> Self {
        Self { judge }
    }

    /// 验证构建命令的结果。
    ///
    /// 执行给定的命令，然后：
    /// 1. 解析 stdout 中的结构化诊断（cargo JSON 输出），非空则据此判定
    /// 2. 使用 LLM 对输出进行语义判断（如果 LlmJudge 可用）
    /// 3. 回退到基于模式的错误检测
    pub async fn verify_build(
        &self,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<VerificationResult, TianyanError> {
        // Execute the command
        let output = execute_command_action(command, cwd, timeout_secs, None).await?;
        let stdout = output["stdout"].as_str().unwrap_or("");
        let stderr = output["stderr"].as_str().unwrap_or("");
        let exit_code = output["exit_code"].as_i64().unwrap_or(-1) as i32;

        // Step 1: 结构化诊断优先（cargo --message-format=json 输出可精确定位到文件/行列）
        let diagnostics = parse_json_diagnostics(stdout);
        if !diagnostics.is_empty() {
            let error_count = diagnostics
                .iter()
                .filter(|d| is_error_level(&d.level))
                .count();
            let passed = exit_code == 0 && error_count == 0;
            let reason = if passed {
                format!("退出码 0，{} 条结构化诊断，无错误级别", diagnostics.len())
            } else {
                format!(
                    "退出码 {exit_code}，{} 条结构化诊断（其中错误级别 {error_count} 条）",
                    diagnostics.len()
                )
            };
            return Ok(VerificationResult {
                passed,
                exit_code,
                reason,
                suggestion: diagnostics.iter().find_map(|d| d.suggestion.clone()),
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
                judge_method: "json_diagnostics".to_string(),
                structured_diagnostics: diagnostics,
            });
        }

        // Step 2: Pattern-based error extraction (always run, fast)
        let pattern_errors = extract_build_errors(stdout, stderr);

        // Step 3: LLM judgment (if available)
        if let Some(ref judge) = self.judge {
            let judgment = judge
                .judge_build_result(command, stdout, stderr, exit_code)
                .await;

            return Ok(VerificationResult {
                passed: judgment.verdict.is_pass(),
                exit_code,
                reason: format!("{}（语义判断）", judgment.reason),
                suggestion: match &judgment.verdict {
                    crate::executor::judge::Verdict::NeedsChanges { suggestion } => {
                        Some(suggestion.clone())
                    }
                    _ => None,
                },
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
                judge_method: "llm".to_string(),
                structured_diagnostics: Vec::new(),
            });
        }

        // Step 4: Fallback — exit code + pattern matching
        let passed = exit_code == 0 && pattern_errors.is_empty();
        let reason = if exit_code != 0 {
            format!(
                "退出码 {}（发现 {} 个可能的错误）",
                exit_code,
                pattern_errors.len()
            )
        } else if !pattern_errors.is_empty() {
            format!("发现 {} 个可能的错误（退出码正常）", pattern_errors.len())
        } else {
            "退出码 0，无检测到的错误".to_string()
        };

        Ok(VerificationResult {
            passed,
            exit_code,
            reason,
            suggestion: None,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            judge_method: "pattern".to_string(),
            structured_diagnostics: Vec::new(),
        })
    }
}

/// 将 VerificationResult 转换为 JSON（供工具返回使用）。
impl From<VerificationResult> for serde_json::Value {
    fn from(r: VerificationResult) -> Self {
        json!({
            "passed": r.passed,
            "exit_code": r.exit_code,
            "reason": r.reason,
            "suggestion": r.suggestion,
            "structured_diagnostics": r.structured_diagnostics,
            "stdout": r.stdout,
            "stderr": r.stderr,
            "judge_method": r.judge_method,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造的 rustc JSON-render-diagnostics 输出（对齐 cargo_metadata 0.23.1
    /// `Message`/`Diagnostic` serde 字段名：reason 标签、target 最小字段、level 枚举）。
    const RUSTC_JSON_FIXTURE: &str = r#"{"reason":"compiler-message","package_id":"demo 0.1.0 (path+file:///C:/tmp/demo)","target":{"kind":["lib"],"crate_types":["lib"],"name":"demo","src_path":"C:\\tmp\\demo\\src\\lib.rs","edition":"2021","doctest":true,"test":true,"doc":true},"message":{"message":"mismatched types","code":{"code":"E0308","explanation":null},"level":"error","spans":[{"file_name":"src/main.rs","byte_start":0,"byte_end":1,"line_start":2,"line_end":2,"column_start":5,"column_end":6,"is_primary":true,"text":[]}],"children":[{"message":"try using a conversion that does not fail","code":null,"level":"help","spans":[],"children":[],"rendered":null}],"rendered":"error[E0308]: mismatched types\n --> src/main.rs:2:5\n"}}"#;

    /// 仅 warning 级别、无 help 子诊断的 fixture。
    const RUSTC_JSON_WARNING_FIXTURE: &str = r#"{"reason":"compiler-message","package_id":"demo 0.1.0 (path+file:///C:/tmp/demo)","target":{"kind":["lib"],"name":"demo","src_path":"C:\\tmp\\demo\\src\\lib.rs"},"message":{"message":"unused variable: `x`","code":{"code":"unused_variables","explanation":null},"level":"warning","spans":[{"file_name":"src/lib.rs","byte_start":0,"byte_end":1,"line_start":1,"line_end":1,"column_start":9,"column_end":10,"is_primary":true,"text":[]}],"children":[],"rendered":"warning: unused variable: `x`\n --> src/lib.rs:1:9\n"}}"#;

    /// 输出 `fixture.json` 的最小命令（Windows `type` / Unix `cat`；测试专用）。
    ///
    /// 注意：不能带引号路径 —— Rust 在 Windows 上按 C 运行时规则转义参数，
    /// cmd 内置命令（type）不识别 `\"` 转义。测试通过 `cwd` 参数切换目录，
    /// 命令只使用裸文件名。
    fn cat_command() -> &'static str {
        if cfg!(windows) {
            "type fixture.json"
        } else {
            "cat fixture.json"
        }
    }

    #[test]
    fn test_verification_gate_new_none() {
        let gate = VerificationGate::new(None);
        assert!(gate.judge.is_none());
    }

    #[test]
    fn test_verification_result_to_json() {
        let result = VerificationResult {
            passed: true,
            exit_code: 0,
            reason: "All good".to_string(),
            suggestion: None,
            stdout: "Build successful".to_string(),
            stderr: String::new(),
            judge_method: "pattern".to_string(),
            structured_diagnostics: Vec::new(),
        };
        let json: serde_json::Value = result.into();
        assert_eq!(json["passed"], true);
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["reason"], "All good");
        assert!(json["suggestion"].is_null());
        assert_eq!(json["stdout"], "Build successful");
        assert_eq!(json["judge_method"], "pattern");
        assert!(json["structured_diagnostics"].is_array());
    }

    #[test]
    fn test_verification_result_to_json_with_suggestion() {
        let result = VerificationResult {
            passed: false,
            exit_code: 1,
            reason: "Found errors".to_string(),
            suggestion: Some("Fix the type error on line 42".to_string()),
            stdout: String::new(),
            stderr: "error: type mismatch".to_string(),
            judge_method: "llm".to_string(),
            structured_diagnostics: vec![StructuredDiagnostic {
                file: "src/main.rs".to_string(),
                line: 2,
                column: 5,
                level: "error".to_string(),
                code: Some("E0308".to_string()),
                message: "mismatched types".to_string(),
                suggestion: None,
            }],
        };
        let json: serde_json::Value = result.into();
        assert_eq!(json["passed"], false);
        assert_eq!(json["suggestion"], "Fix the type error on line 42");
        assert_eq!(json["judge_method"], "llm");
        assert_eq!(json["structured_diagnostics"][0]["code"], "E0308");
        assert_eq!(json["structured_diagnostics"][0]["file"], "src/main.rs");
    }

    #[tokio::test]
    async fn test_verify_build_without_judge() {
        let gate = VerificationGate::new(None);
        let result = gate.verify_build("echo Hello", None, Some(5)).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.passed);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("Hello"));
        assert_eq!(r.judge_method, "pattern");
        assert!(r.structured_diagnostics.is_empty());
    }

    // ── parse_json_diagnostics（纯函数） ─────────────────────────────────

    #[test]
    fn test_parse_json_diagnostics_extracts_error() {
        let diags = parse_json_diagnostics(RUSTC_JSON_FIXTURE);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.file, "src/main.rs");
        assert_eq!(d.line, 2);
        assert_eq!(d.column, 5);
        assert_eq!(d.level, "error");
        assert_eq!(d.code.as_deref(), Some("E0308"));
        assert_eq!(d.message, "mismatched types");
        assert_eq!(
            d.suggestion.as_deref(),
            Some("try using a conversion that does not fail")
        );
    }

    #[test]
    fn test_parse_json_diagnostics_skips_plain_text_lines() {
        let stdout = format!("Compiling demo v0.1.0\n{}\n   Finished", RUSTC_JSON_FIXTURE);
        let diags = parse_json_diagnostics(&stdout);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "src/main.rs");
    }

    #[test]
    fn test_parse_json_diagnostics_empty_for_plain_output() {
        let diags = parse_json_diagnostics("hello world\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn test_parse_json_diagnostics_warning_has_no_suggestion() {
        let diags = parse_json_diagnostics(RUSTC_JSON_WARNING_FIXTURE);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, "warning");
        assert!(diags[0].suggestion.is_none());
        assert_eq!(diags[0].code.as_deref(), Some("unused_variables"));
    }

    #[test]
    fn test_parse_json_diagnostics_caps_at_100() {
        let mut stdout = String::new();
        for i in 0..101 {
            stdout.push_str(&format!(
                r#"{{"reason":"compiler-message","package_id":"demo 0.1.0","target":{{"kind":["lib"],"name":"demo","src_path":"src/lib.rs"}},"message":{{"message":"error {i}","code":null,"level":"error","spans":[],"children":[],"rendered":null}}}}"#,
            ));
            stdout.push('\n');
        }
        let diags = parse_json_diagnostics(&stdout);
        assert_eq!(diags.len(), 100);
        assert!(diags.iter().all(|d| d.message.starts_with("error ")));
    }

    #[test]
    fn test_parse_json_diagnostics_truncates_long_message() {
        let long_msg = format!("error: {}", "x".repeat(3000));
        let line = format!(
            r#"{{"reason":"compiler-message","package_id":"demo 0.1.0","target":{{"kind":["lib"],"name":"demo","src_path":"src/lib.rs"}},"message":{{"message":"{long_msg}","code":null,"level":"error","spans":[],"children":[],"rendered":null}}}}"#,
        );
        let diags = parse_json_diagnostics(&line);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message.chars().count(), 2000);
    }

    // ── 门控决策路径 ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_verify_build_uses_json_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fixture.json"), RUSTC_JSON_FIXTURE).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let gate = VerificationGate::new(None);
        let result = gate
            .verify_build(cat_command(), Some(&cwd), Some(10))
            .await
            .unwrap();
        assert_eq!(result.judge_method, "json_diagnostics");
        // 含 error 级别诊断 → 即使退出码 0 也判失败
        assert!(!result.passed);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.structured_diagnostics.len(), 1);
        assert_eq!(
            result.structured_diagnostics[0].code.as_deref(),
            Some("E0308")
        );
    }

    #[tokio::test]
    async fn test_verify_build_json_warning_only_passes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fixture.json"), RUSTC_JSON_WARNING_FIXTURE).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let gate = VerificationGate::new(None);
        let result = gate
            .verify_build(cat_command(), Some(&cwd), Some(10))
            .await
            .unwrap();
        assert_eq!(result.judge_method, "json_diagnostics");
        assert!(result.passed);
        assert_eq!(result.structured_diagnostics[0].level, "warning");
    }

    #[tokio::test]
    async fn test_verify_build_fallback_pattern_on_nonzero_exit() {
        let gate = VerificationGate::new(None);
        let result = gate.verify_build("exit 1", None, Some(10)).await.unwrap();
        assert_eq!(result.judge_method, "pattern");
        assert!(!result.passed);
        assert_eq!(result.exit_code, 1);
        assert!(result.structured_diagnostics.is_empty());
    }

    #[tokio::test]
    async fn test_verify_build_llm_path_with_failing_mock() {
        use crate::model::MockChatService;
        use std::sync::Arc;

        let mut mock = MockChatService::new();
        mock.expect_chat_completion()
            .returning(|_| Err(TianyanError::Custom("mock 调用失败".to_string())));
        let judge = LlmJudge::new(Arc::new(mock), "test-model");
        let gate = VerificationGate::new(Some(judge));
        // 退出码非 0 且 stdout 非空 → 绕过 LLM 快速路径，实际调用 mock → 失败 → 回退退出码判断。
        let result = gate
            .verify_build("echo oops && exit 1", None, Some(10))
            .await
            .unwrap();
        assert_eq!(result.judge_method, "llm");
        assert!(!result.passed);
        assert!(result.structured_diagnostics.is_empty());
    }
}
