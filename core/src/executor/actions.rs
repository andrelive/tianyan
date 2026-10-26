use serde_json::{json, Value};

use crate::common::binary::{sniff_binary, sniff_binary_format};
use crate::common::error::TianyanError;

pub use crate::executor::command::{
    execute_command_action, execute_command_action_cancellable, kill_all_running_children,
    CommandEventSink, CommandManager, CommandNotifier, CommandTask, CommandTaskStatus,
    CommandWaker,
};
pub use crate::executor::security::SecurityPolicy;

use std::path::Path;

use crate::executor::truncate;

/// 执行文件读取操作（offset/limit 窗口、截断标记、二进制嗅探与目录模式）。
///
/// 签名从 `(path)` 扩展为 `(path, offset, limit)`：`offset` 1 起始（`Some(0)` 视为 1），
/// `limit` 默认 2000（`Some(0)` 视为 1）。`executor/mod.rs` 的 `pub use` 按名称重导出，
/// 无需同步修改；全仓库无其它旧签名调用方（已 grep 验证），故不保留 1 参数兼容包装。
///
/// 返回结构化 JSON：`content`（纯文本）、`truncated`、`total_lines`、`total_bytes`、
/// `showing`（实际窗口）、`message`（仅截断时）；二进制文件返回
/// `binary`/`size`/`format`/`hint`/`preview`（无 `content`），目录返回 `directory`/`entries`。
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

/// 二进制文件结果：大小 + 格式标识 + 可执行 hint + 前 200 字节 lossy 预览。
///
/// **字段契约**：`binary` / `path` / `size` / `preview` 的名称与语义保持历史不变
/// （`gui-vite` 的 `FileViewer.tsx` 按它们渲染「二进制文件」提示与预览），`format`
/// / `hint` 只是**新增**字段。刻意不返回 `content`：命中二进制时没有文本内容可给，
/// 给一段 lossy 文本只会诱导模型从乱码里"读出"内容。
fn binary_result(path: &str, bytes: &[u8]) -> Value {
    let preview_len = bytes.len().min(200);
    let preview = String::from_utf8_lossy(&bytes[..preview_len]).into_owned();
    let format = sniff_binary_format(bytes);
    json!({
        "binary": true,
        "path": path,
        "size": bytes.len(),
        "format": format,
        "preview": preview,
        "hint": binary_hint(format),
    })
}

/// 按识别出的格式生成**可执行的下一步**（不是一句通用客套话）。
///
/// 每条 hint 都包含同一条纪律：「不要依据 preview 猜测文件内容」——`preview` 是
/// lossy 的 200 字节碎片，从里面"读出"文档内容是模型最典型的臆想来源。
fn binary_hint(format: &str) -> String {
    let head = match format {
        "zip" => "ZIP 容器。docx/xlsx/pptx 都是 ZIP：用 execute_command 解压后读 \
                  word/document.xml（docx）、xl/worksheets/*.xml（xlsx）、\
                  ppt/slides/*.xml（pptx）可提取文本；纯 zip 则列出条目即可。"
            .to_string(),
        "pdf" => "PDF 二进制，read_file 读不了；可试 execute_command 跑 pdftotext（若环境有），\
                  否则告知用户该格式无法直接读取。"
            .to_string(),
        "image" => {
            "这是图片，改用 view_image 工具（模型可直接看到像素），不要用 read_file。".to_string()
        }
        "ole2" => "旧版 Office 复合文档（doc/xls/ppt），无法直接提取文本；\
                   建议告知用户另存为 docx/xlsx 后再读。"
            .to_string(),
        "sqlite" => {
            "SQLite 数据库文件，不是文本；需要数据请用 execute_command 调用 sqlite3（若环境有），\
                     否则告知用户。"
                .to_string()
        }
        "pe" => "Windows 可执行文件（exe/dll），不是文本，读不出任何有意义内容。".to_string(),
        "elf" => "ELF 可执行文件 / 共享库，不是文本，读不出任何有意义内容。".to_string(),
        "gzip" => "gzip 压缩流；可用 execute_command 以 gzip/tar 解压后再读解压产物。".to_string(),
        "rar" => "RAR 压缩包；解压需 unrar（若环境无该命令则告知用户无法读取）。".to_string(),
        "7z" => "7z 压缩包；解压需 7z（若环境无该命令则告知用户无法读取）。".to_string(),
        _ => "无法识别格式的二进制文件，本工具只能读文本；请改用 execute_command 探查，\
              或告知用户。"
            .to_string(),
    };
    format!("{head}**不要依据 preview 猜测文件内容**；应向用户说明该格式无法直接读取。")
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
    let total_bytes = text.len();
    // 空文件没有可读行：返回空内容，而非 "offset 1 超出文件总行数 0" 的误导性错误。
    if total_lines == 0 {
        return Ok(json!({
            "path": path,
            "content": "",
            "truncated": false,
            "total_lines": 0,
            "total_bytes": total_bytes,
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
    // `limit: 0` 视为 1；上限钳制到 2000 行（对齐截断层单窗口行数上限，
    // 防"一次读全"的误用——超大请求被钳制后按窗口截断并提示 offset 续读）。
    let limit = limit.unwrap_or(2000).clamp(1, 2000);
    let end = (start + limit).min(total_lines);
    // 内容匹配编辑（apply_edit 已改为 old_string 匹配）：read_file 输出纯内容，
    // 不再带行号/哈希前缀（省 token）；行范围提示仍保留供模型定位讨论。
    let window_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(end - start)
        .map(|(_, line)| *line)
        .collect();
    // 超大窗口（大 limit）仍受统一截断层（50KB / 2000 行）约束。
    // 注意：先对窗口内容截断、后追加提示行 —— 若先拼上提示行，窗口恰好 2000 行时
    // 会被截断层计为 2001 行而把提示行吃掉，丢失精确的续读偏移。
    let trunc = truncate::truncate_head(&window_lines.join("\n"));
    // 实际返回的完整行数（字节截断时可能少于标称窗口）——续读偏移与
    // showing.limit 都按它计算，避免跳过被截掉的行（前端"加载更多"同一契约：
    // showing.limit = 该窗口实际返回的真实行数，不含截断标记）。
    let returned_lines = trunc.kept_lines;
    let shown_end = start + returned_lines;
    let has_more = shown_end < total_lines;
    let message = has_more.then(|| {
        format!(
            "(Showing lines {}-{} of {}. Use offset={} to continue.)",
            start + 1,
            shown_end,
            total_lines,
            shown_end + 1
        )
    });
    let mut content = trunc.text;
    if let Some(m) = &message {
        content.push('\n');
        content.push_str(m);
    }
    let mut out = json!({
        "path": path,
        "content": content,
        "truncated": has_more,
        "total_lines": total_lines,
        "total_bytes": total_bytes,
        "showing": { "offset": start + 1, "limit": returned_lines },
    });
    if let Some(m) = message {
        out["message"] = json!(m);
    }
    Ok(out)
}

/// 执行文件写入操作。
pub async fn execute_write_file(
    path: &str,
    content: &str,
    create_dirs: bool,
) -> Result<Value, TianyanError> {
    // T1-15：原子写（同目录临时文件 + rename）——避免“写一半”损坏用户源码
    crate::executor::write_file_atomic(path, content, create_dirs)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 文件操作失败：{}", e)))?;
    Ok(Value::String("写入成功".to_string()))
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
        let result = execute_write_file(&temp_path, "测试内容", false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(temp_path);
    }

    /// 写一个临时二进制文件并跑 execute_read_file（返回结果 + 清理）。
    async fn read_scratch_binary(tag: &str, bytes: &[u8]) -> Value {
        let path = format!(
            "{}/test_executor_binary_{}_{}.bin",
            std::env::temp_dir().display(),
            std::process::id(),
            tag
        );
        std::fs::write(&path, bytes).expect("写临时文件");
        let result = execute_read_file(&path, None, None).await;
        let _ = std::fs::remove_file(&path);
        result.expect("读取二进制文件不应报错")
    }

    #[tokio::test]
    async fn test_read_file_binary_returns_format_and_hint() {
        // DOCX 就是 ZIP：format 认 zip，hint 指向解压读 word/document.xml
        let value = read_scratch_binary("docx", b"PK\x03\x04\x14\x00\x06\x00\x00\x00").await;
        assert_eq!(value["binary"], json!(true));
        assert_eq!(value["format"], json!("zip"));
        assert_eq!(value["size"], json!(10));
        // 现有字段契约不变：preview 依然存在，且没有 content
        assert!(value["preview"].is_string());
        assert!(value.get("content").is_none(), "二进制不应返回 content");
        let hint = value["hint"].as_str().unwrap();
        assert!(
            hint.contains("word/document.xml"),
            "zip hint 应给出 docx 解法"
        );
        assert!(hint.contains("不要依据 preview 猜测文件内容"));
    }

    #[tokio::test]
    async fn test_read_file_image_points_to_view_image() {
        let value = read_scratch_binary("png", b"\x89PNG\r\n\x1A\n\x00\x00\x00\rIHDR").await;
        assert_eq!(value["format"], json!("image"));
        assert!(value["hint"].as_str().unwrap().contains("view_image"));
    }

    #[tokio::test]
    async fn test_read_file_unknown_binary_still_warns() {
        // 无 magic 但含 NUL → binary:true + format unknown，hint 仍禁止猜测
        let value = read_scratch_binary("raw", b"\x00\x01\x02\x03random").await;
        assert_eq!(value["format"], json!("unknown"));
        assert!(value["hint"].as_str().unwrap().contains("preview"));
    }
}
