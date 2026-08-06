//! 验证门控模块。
//!
//! 将命令执行与 LLM-as-Judge 语义验证结合起来，
//! 为 `verify_build` 工具提供深度质量门控。

use crate::common::error::TianyanError;
use crate::executor::actions::execute_command_action;
use crate::executor::judge::LlmJudge;
use crate::executor::output_parse::extract_build_errors;
use serde_json::json;

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
    /// 判断方式：exit_code / pattern_match / llm_judge / fallback。
    pub judge_method: String,
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
    /// 1. 检查退出码是否为 0
    /// 2. 使用 LLM 对输出进行语义判断（如果 LlmJudge 可用）
    /// 3. 回退到基于模式的错误检测
    pub async fn verify_build(
        &self,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<VerificationResult, TianyanError> {
        // Execute the command
        let output = execute_command_action(command, cwd, timeout_secs).await?;
        let stdout = output["stdout"].as_str().unwrap_or("");
        let stderr = output["stderr"].as_str().unwrap_or("");
        let exit_code = output["exit_code"].as_i64().unwrap_or(-1) as i32;

        // Step 1: Pattern-based error extraction (always run, fast)
        let pattern_errors = extract_build_errors(stdout, stderr);

        // Step 2: LLM judgment (if available)
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
                judge_method: "llm_judge".to_string(),
            });
        }

        // Step 3: Fallback — exit code + pattern matching
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
            judge_method: "pattern_match".to_string(),
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
            "stdout": r.stdout,
            "stderr": r.stderr,
            "judge_method": r.judge_method,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            judge_method: "pattern_match".to_string(),
        };
        let json: serde_json::Value = result.into();
        assert_eq!(json["passed"], true);
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["reason"], "All good");
        assert!(json["suggestion"].is_null());
        assert_eq!(json["stdout"], "Build successful");
        assert_eq!(json["judge_method"], "pattern_match");
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
            judge_method: "llm_judge".to_string(),
        };
        let json: serde_json::Value = result.into();
        assert_eq!(json["passed"], false);
        assert_eq!(json["suggestion"], "Fix the type error on line 42");
        assert_eq!(json["judge_method"], "llm_judge");
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
        assert_eq!(r.judge_method, "pattern_match");
    }
}
