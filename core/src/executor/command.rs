use std::time::Duration;

use serde_json::{json, Value};

use crate::common::error::TianyanError;

pub(super) const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 30;

async fn read_reader_to_vec<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if let Err(e) = reader.read_to_end(&mut buf).await {
        tracing::warn!(error = %e, "读取命令输出失败");
    }
    buf
}

pub(super) fn extract_command_base(cmd: &str) -> String {
    let name = cmd.split_whitespace().next().unwrap_or(cmd);
    let file_name = name.rsplit(&['\\', '/'][..]).next().unwrap_or(name);
    let pure = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".bat"))
        .or_else(|| file_name.strip_suffix(".cmd"))
        .unwrap_or(file_name);
    pure.to_lowercase()
}

/// 执行 ExecuteCommand 动作（无安全策略依赖，纯函数）。
///
/// 跨平台：Unix (Linux/macOS) 用 `sh -c`，Windows 用 `cmd /C`。
pub async fn execute_command_action(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
    let timeout = timeout_secs.unwrap_or(DEFAULT_COMMAND_TIMEOUT_SECS);

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C");
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c");
        c
    };
    cmd.arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| TianyanError::Custom(format!("executor: 命令执行失败：{}", e)))?;

    let stdout_handle = child
        .stdout
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|reader| tokio::spawn(read_reader_to_vec(reader)));

    let result = tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await;

    match result {
        Ok(Ok(status)) => {
            let stdout_bytes = if let Some(handle) = stdout_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stdout 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let stderr_bytes = if let Some(handle) = stderr_handle {
                match handle.await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("读取 stderr 任务失败: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            Ok(json!({
                "stdout": String::from_utf8_lossy(&stdout_bytes).to_string(),
                "stderr": String::from_utf8_lossy(&stderr_bytes).to_string(),
                "exit_code": status.code().unwrap_or(-1),
            }))
        }
        Ok(Err(e)) => Err(TianyanError::Custom(format!(
            "executor: 命令执行失败：{}",
            e
        ))),
        Err(_elapsed) => {
            if let Err(e) = child.kill().await {
                tracing::warn!(error = %e, "终止超时子进程失败");
            }
            if let Err(e) = child.wait().await {
                tracing::warn!(error = %e, "等待超时子进程退出失败");
            }
            Err(TianyanError::timeout("executor: 执行超时"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_command() {
        let result = execute_command_action("echo Hello", None, Some(5)).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output["stdout"].as_str().unwrap_or("").contains("Hello"));
    }

    #[tokio::test]
    async fn test_command_timeout_is_timeout_error() {
        // 子进程运行时长超过超时阈值：Windows 用 ping 计数（约 4s），Unix 用 sleep
        let cmd = if cfg!(target_os = "windows") {
            "ping -n 5 127.0.0.1"
        } else {
            "sleep 5"
        };
        let err = execute_command_action(cmd, None, Some(1))
            .await
            .unwrap_err();
        assert!(
            err.is_timeout(),
            "执行超时应分类为 timeout（ADR-014）：{err}"
        );
    }
}
