//! 命令输出解析工具：从 build/lint 输出中提取编译错误、从 test 输出中统计/提取结果。

/// 从 build/lint 输出中提取编译错误。
///
/// 统一提取语义（合并 actions::extract_build_errors 与 verification::extract_build_errors 两套行为）：
/// - 大小写不敏感匹配 `error:` / `error[`（原 verification 版行为）
/// - 每行 trim 首尾空白（原 actions 版行为）
/// - 单行截断到 200 字符（原 verification 版行为，改为按 char 截断避免字节边界 panic）
/// - 最多保留 50 条，超出追加截断标记（原 actions 版行为）
pub(crate) fn extract_build_errors(stdout: &str, stderr: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut truncated = false;

    for line in stdout.lines().chain(stderr.lines()) {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if !(lower.contains("error:") || lower.contains("error[")) {
            continue;
        }
        if errors.len() >= 50 {
            truncated = true;
            break;
        }
        let clipped: String = trimmed.chars().take(200).collect();
        errors.push(clipped);
    }

    if truncated {
        errors.push("... (截断，过多错误输出)".to_string());
    }

    errors
}

/// 从 cargo test 输出中统计通过的测试数。
pub(crate) fn count_test_passed(stdout: &str) -> usize {
    for line in stdout.lines().rev() {
        if line.contains("test result:") {
            if let Some(passed_str) = line.split(';').next().and_then(|s| s.split("ok. ").nth(1)) {
                return passed_str
                    .split(' ')
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
            }
            if let Some(pos) = line.find(" passed") {
                let before = &line[..pos];
                if let Some(num) = before.rsplit(' ').next() {
                    return num.parse().unwrap_or(0);
                }
            }
        }
    }
    0
}

/// 从 cargo test 输出中提取失败测试的名称和错误。
pub(crate) fn extract_test_failures(stdout: &str, stderr: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let combined = format!("{}\n{}", stdout, stderr);
    let mut in_failure = false;

    for line in combined.lines() {
        if line.contains("FAILED") || line.starts_with("thread '") {
            in_failure = true;
            failures.push(line.trim().to_string());
        } else if line.contains("failures:") {
            in_failure = false;
        } else if in_failure && !line.trim().is_empty() {
            failures.push(line.trim().to_string());
        }
        if failures.len() > 50 {
            failures.push("... (截断，过多失败输出)".to_string());
            break;
        }
    }

    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_build_errors_from_stdout() {
        let stdout =
            "Compiling foo.rs v1.0.0\nerror[E0308]: mismatched types\n  --> src/main.rs:10:5\n";
        let stderr = "";
        let errors = extract_build_errors(stdout, stderr);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("error[E0308]: mismatched types"));
    }

    #[test]
    fn test_extract_build_errors_from_stderr() {
        let stdout = "";
        let stderr = "  error: expected `;`\n  --> src/lib.rs:42:10\n";
        let errors = extract_build_errors(stdout, stderr);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("error: expected `;`"));
    }

    #[test]
    fn test_extract_build_errors_clean_output() {
        let stdout = "Compiling foo.rs v1.0.0\n   Compilation successful\n";
        let stderr = "warning: unused variable `x`\n";
        let errors = extract_build_errors(stdout, stderr);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_extract_build_errors_case_insensitive() {
        let stdout = "ERROR: something went wrong\n";
        let stderr = "";
        let errors = extract_build_errors(stdout, stderr);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("ERROR: something went wrong"));
    }

    #[test]
    fn test_extract_build_errors_caps_at_50_with_marker() {
        let mut stdout = String::new();
        for i in 0..51 {
            stdout.push_str(&format!("error: fake error number {i}\n"));
        }
        let errors = extract_build_errors(&stdout, "");
        assert_eq!(errors.len(), 51);
        assert_eq!(&errors[50], "... (截断，过多错误输出)");
        assert!(errors[0].contains("fake error number 0"));
    }

    #[test]
    fn test_extract_build_errors_truncates_long_line() {
        let long = format!("error: {}", "x".repeat(300));
        let errors = extract_build_errors(&long, "");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("error:"));
        assert!(
            errors[0].len() <= 200,
            "行长度 {} 超过 200",
            errors[0].len()
        );
    }

    #[test]
    fn test_extract_build_errors_trims_leading_whitespace() {
        let stderr = "    error: expected `;`\n";
        let errors = extract_build_errors("", stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0], "error: expected `;`");
    }
}
