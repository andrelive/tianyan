//! LLM-as-Judge 语义验证模块。
//!
//! 对命令输出进行语义判断，而非仅依赖退出码和关键词匹配。
//! 用于 `verify_build` 工具的深度质量门控。

use crate::model::types::ChatCompletionRequest;
use crate::model::ChatService;
use std::sync::Arc;

/// 判断结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 通过 — 输出表明操作成功完成。
    Pass,
    /// 失败 — 输出表明操作失败。
    Fail,
    /// 需要修改 — 输出表明操作部分成功但有问题需要修复。
    NeedsChanges {
        /// LLM 给出的修复建议。
        suggestion: String,
    },
}

impl Verdict {
    /// 是否表示成功。
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }
}

/// LLM 判断结果。
#[derive(Debug, Clone)]
pub struct Judgment {
    /// 判断结论。
    pub verdict: Verdict,
    /// 判断理由。
    pub reason: String,
}

/// LLM-as-Judge — 使用 LLM 对命令输出进行语义质量评估。
///
/// 与简单的退出码检查和关键词匹配不同，LlmJudge 能够理解：
/// - 编译警告是否表明真正的回归
/// - 测试失败是否有关系（例如 flaky test）
/// - 输出是否符合预期的语义结构
#[derive(Clone)]
pub struct LlmJudge {
    model_service: Arc<dyn ChatService>,
    model: String,
}

impl LlmJudge {
    /// 创建一个新的 LLM 判断器。
    pub fn new(model_service: Arc<dyn ChatService>, model: impl Into<String>) -> Self {
        Self {
            model_service,
            model: model.into(),
        }
    }

    /// 对构建/验证命令输出进行语义判断。
    ///
    /// # 参数
    /// - `command` — 被执行的命令（如 `cargo check`）
    /// - `stdout` — 命令的标准输出
    /// - `stderr` — 命令的标准错误输出
    /// - `exit_code` — 命令的退出码
    pub async fn judge_build_result(
        &self,
        command: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
    ) -> Judgment {
        // Fast path: clean exit code 0 with empty stderr → Pass
        if exit_code == 0 && stderr.trim().is_empty() {
            return Judgment {
                verdict: Verdict::Pass,
                reason: "退出码 0，无错误输出".to_string(),
            };
        }

        // Fast path: non-zero exit code and stdout is empty → Fail
        if exit_code != 0 && stdout.trim().is_empty() && stderr.trim().is_empty() {
            return Judgment {
                verdict: Verdict::Fail,
                reason: format!("命令失败，退出码 {}，无输出", exit_code),
            };
        }

        // Truncate output to keep LLM call cheap
        let stdout_trunc = truncate_for_judge(stdout, 2000);
        let stderr_trunc = truncate_for_judge(stderr, 2000);

        let prompt = build_judge_prompt(command, &stdout_trunc, &stderr_trunc, exit_code);

        let request = ChatCompletionRequest::new(
            &self.model,
            vec![crate::common::types::Message::user(&prompt)],
        );

        match self.model_service.chat_completion(request).await {
            Ok(response) => {
                let choice = match response.choices.into_iter().next() {
                    Some(c) => c,
                    None => return fallback_judgment(exit_code),
                };
                parse_judgment(&choice.message.content, exit_code)
            }
            Err(e) => {
                tracing::warn!(error = %e, "LlmJudge LLM 调用失败，回退到退出码判断");
                fallback_judgment(exit_code)
            }
        }
    }
}

/// 构建 LLM 判断提示词。
fn build_judge_prompt(command: &str, stdout: &str, stderr: &str, exit_code: i32) -> String {
    format!(
        r#"你是一个构建输出分析器。请判断以下命令的输出是否表明成功。

命令: {}
退出码: {}

标准输出:
{}
标准错误:
{}

请用以下格式回复（只回复此格式，不要添加其他内容）:
VERDICT: PASS|FAIL|NEEDS_CHANGES
REASON: <一句话说明理由>
SUGGESTION: <如果 NEEDS_CHANGES，给出修复建议；否则留空>

判断指南:
- PASS: 命令成功完成，没有实质性错误。警告不算失败。
- FAIL: 命令明确失败，有编译错误或致命错误。
- NEEDS_CHANGES: 命令有部分错误，需要修改但不算完全失败（例如：1个测试失败，其余通过）。"#,
        command, exit_code, stdout, stderr,
    )
}

/// 解析 LLM 回复中的判断。
fn parse_judgment(content: &str, exit_code: i32) -> Judgment {
    let verdict_line = content
        .lines()
        .find(|l| l.starts_with("VERDICT:"))
        .unwrap_or("");

    let reason_line = content
        .lines()
        .find(|l| l.starts_with("REASON:"))
        .unwrap_or("");

    let suggestion_line = content
        .lines()
        .find(|l| l.starts_with("SUGGESTION:"))
        .unwrap_or("");

    let verdict = if verdict_line.contains("PASS") {
        Verdict::Pass
    } else if verdict_line.contains("NEEDS_CHANGES") {
        let suggestion = suggestion_line
            .strip_prefix("SUGGESTION:")
            .unwrap_or("")
            .trim()
            .to_string();
        Verdict::NeedsChanges { suggestion }
    } else {
        // Default: FAIL — and fall back to exit code if ambiguous
        if exit_code == 0 {
            Verdict::Pass // LLM couldn't find failure, exit code says OK
        } else {
            Verdict::Fail
        }
    };

    let reason = reason_line
        .strip_prefix("REASON:")
        .unwrap_or("无法解析 LLM 判断结果")
        .trim()
        .to_string();

    Judgment { verdict, reason }
}

/// 回退判断 — 仅基于退出码。
fn fallback_judgment(exit_code: i32) -> Judgment {
    if exit_code == 0 {
        Judgment {
            verdict: Verdict::Pass,
            reason: "退出码 0（LLM 判断不可用，回退到退出码检查）".to_string(),
        }
    } else {
        Judgment {
            verdict: Verdict::Fail,
            reason: format!("退出码 {}（LLM 判断不可用，回退到退出码检查）", exit_code),
        }
    }
}

/// 截断输出以控制 LLM 调用成本。
fn truncate_for_judge(output: &str, max_chars: usize) -> String {
    if output.len() <= max_chars {
        output.to_string()
    } else {
        // Keep the beginning (first error is usually most important)
        // and the end (summary/error count often at the end)
        let head = &output[..max_chars * 2 / 3];
        let tail_start = output.len().saturating_sub(max_chars / 3);
        let tail = &output[tail_start..];
        format!(
            "{}...\n[输出被截断，省略 {} 字符]...\n{}",
            head,
            output.len() - head.len() - tail.len(),
            tail
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_output_is_unchanged() {
        let short = "hello world";
        assert_eq!(truncate_for_judge(short, 2000), short);
    }

    #[test]
    fn test_truncate_long_output() {
        let long = "a".repeat(3000);
        let truncated = truncate_for_judge(&long, 2000);
        assert!(truncated.len() <= 2500); // Allow some overhead for truncation message
        assert!(truncated.contains("[输出被截断"));
    }

    #[test]
    fn test_fallback_judgment_pass() {
        let j = fallback_judgment(0);
        assert_eq!(j.verdict, Verdict::Pass);
    }

    #[test]
    fn test_fallback_judgment_fail() {
        let j = fallback_judgment(1);
        assert_eq!(j.verdict, Verdict::Fail);
    }

    #[test]
    fn test_parse_judgment_pass() {
        let j = parse_judgment("VERDICT: PASS\nREASON: All good", 0);
        assert_eq!(j.verdict, Verdict::Pass);
    }

    #[test]
    fn test_parse_judgment_fail() {
        let j = parse_judgment("VERDICT: FAIL\nREASON: Compilation error", 1);
        assert_eq!(j.verdict, Verdict::Fail);
    }

    #[test]
    fn test_parse_judgment_needs_changes() {
        let j = parse_judgment(
            "VERDICT: NEEDS_CHANGES\nREASON: One test fails\nSUGGESTION: Fix the assertion on line 42",
            1,
        );
        assert!(matches!(j.verdict, Verdict::NeedsChanges { .. }));
    }

    #[test]
    fn test_verdict_is_pass() {
        assert!(Verdict::Pass.is_pass());
        assert!(!Verdict::Fail.is_pass());
        assert!(!Verdict::NeedsChanges {
            suggestion: String::new()
        }
        .is_pass());
    }
}
