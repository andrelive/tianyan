use serde_json::{json, Value};

use crate::common::binary::sniff_binary;
use crate::common::error::TianyanError;

pub use crate::executor::command::{
    execute_command_action, CommandEventSink, CommandManager, CommandNotifier, CommandTask,
    CommandTaskStatus, CommandWaker,
};
pub use crate::executor::security::SecurityPolicy;

use std::path::Path;

use crate::executor::truncate;

/// 执行文件读取操作（带行号锚点、offset/limit 窗口、截断标记、二进制嗅探与目录模式）。
///
/// 签名从 `(path)` 扩展为 `(path, offset, limit)`：`offset` 1 起始（`Some(0)` 视为 1），
/// `limit` 默认 2000（`Some(0)` 视为 1）。`executor/mod.rs` 的 `pub use` 按名称重导出，
/// 无需同步修改；全仓库无其它旧签名调用方（已 grep 验证），故不保留 1 参数兼容包装。
///
/// 返回结构化 JSON：`content`（`N#ID|content` 锚点行）、`truncated`、`total_lines`、
/// `showing`（实际窗口）、`message`（仅截断时）；二进制文件返回 `binary`/`size`/`preview`，
/// 目录返回 `directory`/`entries`。
pub async fn execute_read_file(
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, TianyanError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| not_found_error(path, e))?;
    if meta.is_dir() {
        // 目录模式：统一委托 fs::execute_list_dir（唯一目录列举实现）。
        let output = crate::executor::fs::execute_list_dir(Path::new(path), offset, limit).await?;
        return serde_json::to_value(output)
            .map_err(|e| TianyanError::Custom(format!("executor: 序列化失败：{e}")));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| not_found_error(path, e))?;
    if sniff_binary(&bytes) {
        return Ok(binary_result(path, &bytes));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    build_window(path, &text, offset, limit)
}

/// 将 io 错误映射为执行错误；文件不存在时附带父目录中相似文件名的建议（最多 3 个）。
fn not_found_error(path: &str, e: std::io::Error) -> TianyanError {
    if e.kind() == std::io::ErrorKind::NotFound {
        TianyanError::not_found(format!(
            "executor: 文件不存在：{path}{}",
            suggest_similar_paths(Path::new(path))
        ))
    } else {
        TianyanError::Custom(format!("executor: 文件操作失败：{e}"))
    }
}

/// 在父目录中查找文件名包含缺失文件 stem（小写）的条目作为建议，最多 3 个。
fn suggest_similar_paths(path: &Path) -> String {
    let stem_lower = match path.file_stem().and_then(|s| s.to_str()) {
        Some(s) if !s.is_empty() => s.to_lowercase(),
        _ => return String::new(),
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut similar: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(parent) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_lowercase().contains(&stem_lower) {
                similar.push(entry.path().to_string_lossy().into_owned());
                if similar.len() >= 3 {
                    break;
                }
            }
        }
    }
    if similar.is_empty() {
        String::new()
    } else {
        format!("，您是否想找：{}", similar.join("，"))
    }
}

/// 二进制文件结果：仅返回大小与前 200 字节的 lossy 预览。
fn binary_result(path: &str, bytes: &[u8]) -> Value {
    let preview_len = bytes.len().min(200);
    let preview = String::from_utf8_lossy(&bytes[..preview_len]).into_owned();
    json!({
        "binary": true,
        "path": path,
        "size": bytes.len(),
        "preview": preview,
    })
}

/// 文本行模式：应用 offset/limit 窗口，逐行生成 `N#ID|content` 锚点，追加截断提示行，
/// 并对超大窗口施加截断层（50KB / 2000 行）双上限。
fn build_window(
    path: &str,
    text: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, TianyanError> {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    // 空文件没有可读行：返回空内容，而非 "offset 1 超出文件总行数 0" 的误导性错误。
    if total_lines == 0 {
        return Ok(json!({
            "path": path,
            "content": "",
            "truncated": false,
            "total_lines": 0,
            "showing": { "offset": 1, "limit": limit.unwrap_or(2000) },
        }));
    }
    let start = offset.unwrap_or(1).max(1) - 1; // 1 起始；`offset: 0` 视为 1
    if start >= total_lines {
        return Err(TianyanError::Custom(format!(
            "executor: offset {} 超出文件总行数 {total_lines}",
            start + 1
        )));
    }
    let limit = limit.unwrap_or(2000).max(1); // `limit: 0` 视为 1
    let end = (start + limit).min(total_lines);
    let window_truncated = end < total_lines;
    // 内容匹配编辑（apply_edit 已改为 old_string 匹配）：read_file 输出纯内容，
    // 不再带行号/哈希前缀（省 token）；行范围提示仍保留供模型定位讨论。
    let window_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(end - start)
        .map(|(_, line)| *line)
        .collect();
    let message = window_truncated.then(|| {
        format!(
            "(Showing lines {}-{} of {}. Use offset={} to continue.)",
            start + 1,
            end,
            total_lines,
            end + 1
        )
    });
    // 超大窗口（大 limit）仍受统一截断层（50KB / 2000 行）约束。
    // 注意：先对窗口内容截断、后追加提示行 —— 若先拼上提示行，窗口恰好 2000 行时
    // 会被截断层计为 2001 行而把提示行吃掉，丢失精确的续读偏移。
    let trunc = truncate::truncate_head(&window_lines.join("\n"));
    let mut content = trunc.text;
    if let Some(m) = &message {
        content.push('\n');
        content.push_str(m);
    }
    let mut out = json!({
        "path": path,
        "content": content,
        "truncated": window_truncated || trunc.truncated,
        "total_lines": total_lines,
        "showing": { "offset": start + 1, "limit": limit },
    });
    if let Some(m) = message {
        out["message"] = json!(m);
    }
    Ok(out)
}

/// 执行文件写入操作。
pub async fn execute_write_file(path: &str, content: &str) -> Result<Value, TianyanError> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String("写入成功".to_string()))
}

/// 执行构建验证操作。
///
/// 统一委托 [`VerificationGate::verify_build`]（无 LLM 门控时 `judge=None`）：
/// 结构化诊断优先（json_diagnostics）→ pattern 回退，输出 JSON 形状与门控路径
/// 完全一致（passed / exit_code / reason / suggestion / structured_diagnostics /
/// stdout / stderr / judge_method）——判定规则不再有第二份实现。
pub async fn execute_verify_build(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, TianyanError> {
    let result = crate::executor::verification::VerificationGate::new(None)
        .verify_build(command, cwd, timeout_secs)
        .await?;
    Ok(Value::from(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_read_file() {
        let result = execute_read_file("Cargo.toml", None, None).await;
        assert!(result.is_ok());
        let value = result.unwrap();
        assert!(value["content"].is_string());
        assert!(value["total_lines"].as_u64().is_some());
        assert_eq!(value["truncated"].as_bool(), Some(false));
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
