//! 统一截断层：为 read/grep 模式与命令输出模式提供 UTF-8 安全的截断。
//!
//! 行数（[`MAX_LINES`]）与字节数（[`MAX_BYTES`]）双上限；
//! 只通过 `lines()` / `chars()` 遍历，绝不按字节切片，永不切分字符。

use std::path::Path;

use crate::common::error::TianyanError;

/// 最大保留行数。
pub const MAX_LINES: usize = 2000;
/// 最大保留字节数（50 KiB）。
pub const MAX_BYTES: usize = 50 * 1024;

/// 截断结果。
pub struct Truncated {
    /// 截断后的文本（未截断时为原文）。
    pub text: String,
    /// 是否发生截断。
    pub truncated: bool,
    /// 原文总行数。
    pub total_lines: usize,
    /// 原文总字节数。
    pub total_bytes: usize,
    /// 实际保留行数（截断后仍完整保留的行；未截断 = total_lines）。
    pub kept_lines: usize,
    /// 全文溢出文件路径（仅 [`truncate_spill`] 设置）。
    pub spill_path: Option<String>,
}

/// 保留开头（read/grep 模式）：从头部逐行累积，行数或字节数任一超限即停止；
/// 截断时追加 `... (输出已截断，共 N 行 M 字节，使用 offset 继续)` 标记行。
pub fn truncate_head(text: &str) -> Truncated {
    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let marker =
        format!("\n... (输出已截断，共 {total_lines} 行 {total_bytes} 字节，使用 offset 继续)");
    truncate_head_with_marker(text, &marker)
}

/// 头部截断 + 自定义提示后缀（提示与总行数/字节数一起进入统一截断标记）。
/// 用于**没有 offset 续读参数**的场景——如 vfs_read 会话导出（提示改用检索工具）。
pub fn truncate_head_noted(text: &str, note: &str) -> Truncated {
    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let marker = format!("\n... (输出已截断，共 {total_lines} 行 {total_bytes} 字节{note})");
    truncate_head_with_marker(text, &marker)
}

/// 保留末尾（命令输出模式，错误/摘要通常在尾部）：
/// 从最后一行向前累积，行数或字节数超限即停止；
/// 截断时在开头前置 `... (输出已截断，共 N 行 M 字节)` 标记行。
pub fn truncate_tail(text: &str) -> Truncated {
    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let marker = format!("... (输出已截断，共 {total_lines} 行 {total_bytes} 字节)\n");
    truncate_tail_with_marker(text, &marker)
}

/// 尾部截断 + 自定义提示后缀（提示与总行数/字节数一起进入统一截断标记）。
/// 用于命令输出已落盘场景——截断后指引完整输出的读取路径（read_file 分页）。
pub fn truncate_tail_noted(text: &str, note: &str) -> Truncated {
    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let marker = format!("... (输出已截断，共 {total_lines} 行 {total_bytes} 字节{note})\n");
    truncate_tail_with_marker(text, &marker)
}

/// [`truncate_tail`] 与 [`truncate_tail_noted`] 的公共实现：以给定标记行前置截断。
fn truncate_tail_with_marker(text: &str, marker: &str) -> Truncated {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    for line in lines.iter().rev() {
        let line_bytes = line.len() + 1; // 近似计入 '\n'
        if kept.len() >= MAX_LINES || bytes + line_bytes > MAX_BYTES {
            truncated = true;
            break;
        }
        bytes += line_bytes;
        kept.push(line);
    }
    if !truncated {
        return Truncated {
            text: text.to_string(),
            truncated: false,
            total_lines: lines.len(),
            total_bytes: text.len(),
            kept_lines: lines.len(),
            spill_path: None,
        };
    }
    kept.reverse();
    let mut out = marker.to_string();
    out.push_str(&kept.join("\n"));
    Truncated {
        text: out,
        truncated: true,
        total_lines: lines.len(),
        total_bytes: text.len(),
        kept_lines: kept.len(),
        spill_path: None,
    }
}

/// 同 [`truncate_head`]，但将完整原文写入 `{spill_dir}/{tag}-{unix_ms}.txt`，
/// 设置 `spill_path`，标记消息追加 `，全文已保存至 {path}`。
pub async fn truncate_spill(
    text: &str,
    spill_dir: &Path,
    tag: &str,
) -> Result<Truncated, TianyanError> {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = spill_dir.join(format!("{tag}-{unix_ms}.txt"));
    tokio::fs::write(&path, text).await.map_err(|e| {
        TianyanError::Custom(format!(
            "executor: truncate_spill 写入溢出文件失败 {}: {e}",
            path.display()
        ))
    })?;

    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let marker = format!(
        "\n... (输出已截断，共 {total_lines} 行 {total_bytes} 字节，使用 offset 继续，全文已保存至 {})",
        path.display()
    );
    let mut result = truncate_head_with_marker(text, &marker);
    result.spill_path = Some(path.to_string_lossy().into_owned());
    Ok(result)
}

/// [`truncate_head`] 与 [`truncate_spill`] 的公共实现：以给定标记行截断。
fn truncate_head_with_marker(text: &str, marker: &str) -> Truncated {
    let total_lines = text.lines().count();
    let total_bytes = text.len();
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    for line in text.lines() {
        let line_bytes = line.len() + 1; // 近似计入 '\n'
        if kept.len() >= MAX_LINES || bytes + line_bytes > MAX_BYTES {
            truncated = true;
            break;
        }
        bytes += line_bytes;
        kept.push(line);
    }
    if !truncated {
        return Truncated {
            text: text.to_string(),
            truncated: false,
            total_lines,
            total_bytes,
            kept_lines: total_lines,
            spill_path: None,
        };
    }
    let mut out = kept.join("\n");
    out.push_str(marker);
    Truncated {
        text: out,
        truncated: true,
        total_lines,
        total_bytes,
        kept_lines: kept.len(),
        spill_path: None,
    }
}

/// 单行显示最大字符数（超过截断并追加 TRUNCATED_MARKER）。
pub const MAX_LINE_CHARS: usize = 2000;
/// 单行截断标记。
pub const TRUNCATED_MARKER: &str = "…<truncated>";

/// 单行显示截断：超过 MAX_LINE_CHARS 个字符的行截断并追加标记。
///
/// 返回 (显示文本, 保留字节数)——保留字节数供偏移计算（search_engine 用）。
pub fn truncate_line(line: &str) -> (String, usize) {
    if line.chars().count() > MAX_LINE_CHARS {
        let shown: String = line.chars().take(MAX_LINE_CHARS).collect();
        let kept_bytes = shown.len();
        (shown + TRUNCATED_MARKER, kept_bytes)
    } else {
        (line.to_string(), line.len())
    }
}

#[cfg(test)]
#[path = "truncate_tests.rs"]
mod tests;
