use serde_json::{json, Value};

use crate::common::error::TianyanError;
use crate::executor::output_parse::{
    count_test_passed, extract_build_errors, extract_test_failures,
};

pub use crate::executor::command::execute_command_action;
pub use crate::executor::security::SecurityPolicy;

/// 执行文件读取操作。
pub async fn execute_read_file(path: &str) -> Result<Value, TianyanError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String(content))
}

/// 执行文件写入操作。
pub async fn execute_write_file(path: &str, content: &str) -> Result<Value, TianyanError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String("写入成功".to_string()))
}

/// 执行代码搜索操作。
pub async fn execute_search_code(query: &str, scope: Option<&str>) -> Result<Value, TianyanError> {
    let mut cmd = tokio::process::Command::new("rg");
    cmd.arg("--json")
        .arg("--line-number")
        .arg("--max-count")
        .arg("20")
        .arg(query);

    if let Some(dir) = scope {
        cmd.current_dir(dir);
    }

    let output = cmd.output().await.map_err(|e| {
        TianyanError::Custom(format!("executor: 搜索失败：执行 ripgrep 失败：{}", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no matches found") || stderr.is_empty() {
            return Ok(json!({
                "query": query,
                "scope": scope,
                "results": [],
                "count": 0
            }));
        }
        return Err(TianyanError::Custom(format!(
            "executor: 搜索失败：ripgrep 执行失败：{}",
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        if let Ok(json_value) = serde_json::from_str::<Value>(line) {
            if let Some(data) = json_value.get("data") {
                results.push(json!({
                    "path": data.get("path").map(|p| p.as_str().unwrap_or("")),
                    "line_number": data.get("line_number").map(|n| n.as_u64().unwrap_or(0)),
                    "text": data.get("text").map(|t| t.as_str().unwrap_or("")),
                }));
            }
        }
    }

    Ok(json!({
        "query": query,
        "scope": scope,
        "results": results,
        "count": results.len()
    }))
}

/// 执行测试运行操作。
pub async fn execute_run_tests(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
    let output = execute_command_action(command, cwd, timeout_secs).await?;
    let stdout = output["stdout"].as_str().unwrap_or("");
    let stderr = output["stderr"].as_str().unwrap_or("");
    let exit_code = output["exit_code"].as_i64().unwrap_or(-1);

    let passed = count_test_passed(stdout);
    let failures: Vec<String> = extract_test_failures(stdout, stderr);

    Ok(json!({
        "success": exit_code == 0,
        "passed": passed,
        "failed": failures.len(),
        "failures": failures,
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }))
}

/// 执行构建验证操作。
pub async fn execute_verify_build(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
    let output = execute_command_action(command, cwd, timeout_secs).await?;
    let stdout = output["stdout"].as_str().unwrap_or("");
    let stderr = output["stderr"].as_str().unwrap_or("");
    let exit_code = output["exit_code"].as_i64().unwrap_or(-1);

    let errors: Vec<String> = extract_build_errors(stdout, stderr);

    Ok(json!({
        "success": exit_code == 0,
        "error_count": errors.len(),
        "errors": errors,
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": exit_code,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_read_file() {
        let result = execute_read_file("Cargo.toml").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_write_file() {
        let temp_path = format!(
            "{}/test_executor_{}.txt",
            std::env::temp_dir().display(),
            std::process::id()
        );
        let result = execute_write_file(&temp_path, "测试内容").await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(temp_path);
    }
}
