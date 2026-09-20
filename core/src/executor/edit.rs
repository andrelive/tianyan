//! 内容匹配编辑（apply_edit 核心逻辑）。
//!
//! 采用 old_string / new_string 内容匹配（对齐 DSH / Claude Code / opencode edit）：
//! 在文件内容中查找 old_string（须唯一，除非 replace_all），替换为 new_string。
//! 内容匹配天然免疫行号漂移，无需 read_file 的行号 / 哈希前缀。
//! 批量编辑（1..=20 条）先全部定位校验，再自底向上应用（原子性）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use similar::TextDiff;

use crate::common::error::{Result, TianyanError};

/// 单次编辑允许的最大条数（1..=20）。
pub const MAX_EDITS: usize = 20;

/// 单条内容编辑规格。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContentEdit {
    /// 要查找并替换的原文（须在文件中唯一；多处出现需 replace_all 或提供更多上下文）。
    pub old_string: String,
    /// 替换后的新内容。
    pub new_string: String,
    /// 替换所有出现（默认 false：仅允许唯一匹配）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_all: Option<bool>,
}

/// 检测内容的行尾风格（含 CRLF 则整体按 CRLF 处理，否则 LF）。
pub fn detect_eol(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 按行拆分内容（剥离行尾 CR；末尾 LF 产生的空串弹出，写回时恢复）。
///
/// 与 apply_patch 共用同一 EOL 语义（I7）。
pub(crate) fn split_lines(content: &str) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let had_trailing_newline = content.ends_with('\n');
    if had_trailing_newline {
        lines.pop();
    }
    (lines, had_trailing_newline)
}

/// 以检测到的行尾风格 join 写回，保留原尾随换行。
pub(crate) fn join_lines(lines: &[String], eol: &str, had_trailing_newline: bool) -> String {
    let mut out = lines.join(eol);
    if had_trailing_newline {
        out.push_str(eol);
    }
    out
}

/// 区间重叠检测：按起始位置自底向上排序后，相邻区间相交即重叠。
///
/// 输入为半开区间 (start, end)；返回重叠区间的 1 起始索引对（错误消息用）。
pub(crate) fn find_overlap(ranges: &[(usize, usize)]) -> Option<(usize, usize)> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by(|&a, &b| ranges[b].0.cmp(&ranges[a].0));
    for pair in order.windows(2) {
        let upper = ranges[pair[0]];
        let lower = ranges[pair[1]];
        if lower.0 < upper.1 && upper.0 < lower.1 {
            return Some((pair[1] + 1, pair[0] + 1));
        }
    }
    None
}

/// 查找 needle 在 haystack 中的非重叠出现位置（字节偏移）。
fn find_all_non_overlapping(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    if needle.is_empty() {
        return positions;
    }
    let needle_len = needle.len();
    let mut from = 0;
    while from <= haystack.len() {
        match haystack[from..].find(needle) {
            Some(rel) => {
                let pos = from + rel;
                positions.push(pos);
                from = pos + needle_len;
            }
            None => break,
        }
    }
    positions
}

/// 找 content 中与 needle（空白不敏感）最相似的行，返回 (行号, 行内容)。
/// 用于 old_string 未找到时给模型就近提示，减少整文件重读。
fn closest_line(content: &str, needle: &str) -> Option<(usize, String)> {
    let needle_norm: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
    if needle_norm.is_empty() {
        return None;
    }
    let mut best: Option<(usize, f32, String)> = None;
    for (i, line) in content.lines().enumerate() {
        let line_norm: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        let score = TextDiff::from_chars(&line_norm, &needle_norm).ratio();
        if best.as_ref().is_none_or(|(_, bs, _)| score > *bs) {
            best = Some((i + 1, score, line.to_string()));
        }
    }
    best.map(|(n, _, l)| (n, l))
}

/// 纯函数：对内容应用编辑（内容匹配定位 + 原子批量应用），不涉及文件系统。
///
/// 行尾归一化（对齐 DSH editText）：匹配前把 CRLF 归成 LF（模型用 \n 写 old_string，
/// CRLF 文件是 \r\n，不归一化则永远匹配不上）；写回时按原文件主导行尾恢复。
/// 内容匹配是精确子串匹配（仅容忍行尾差异）。
///
/// 流程：
/// 1. 校验编辑条数（1..=20）与 old_string 非空。
/// 2. 逐条在 LF 归一化内容中查找 old_string（同样归一化）：未找到 → 409；多处且非 replace_all → 报错。
/// 3. 检测替换区间重叠。
/// 4. 自底向上（起始位置降序）替换，保证较早位置不受后续替换影响。
pub fn apply_edits_to_content(content: &str, edits: &[ContentEdit]) -> Result<String> {
    let crlf = content.contains("\r\n");
    let normalized = content.replace("\r\n", "\n");
    let result = apply_normalized(&normalized, edits)?;
    if crlf {
        Ok(result.replace('\n', "\r\n"))
    } else {
        Ok(result)
    }
}

/// 在 LF 归一化内容上应用编辑（匹配与替换），不做行尾处理。
fn apply_normalized(content: &str, edits: &[ContentEdit]) -> Result<String> {
    if edits.is_empty() {
        return Err(TianyanError::Custom(
            "executor: apply_edit: 编辑列表为空，至少需要 1 处编辑".to_string(),
        ));
    }
    if edits.len() > MAX_EDITS {
        return Err(TianyanError::Custom(format!(
            "executor: apply_edit: 编辑数量 {} 超过上限 {MAX_EDITS}",
            edits.len()
        )));
    }

    // 收集所有替换区间（start, end, new_string）。
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(TianyanError::Custom(format!(
                "executor: apply_edit: 第{}处编辑：old_string 不能为空",
                i + 1
            )));
        }
        // old_string 同样做行尾归一化（模型用 \n 写，CRLF 文件需先归成 \n 再匹配）
        let needle = edit.old_string.replace("\r\n", "\n");
        let positions = find_all_non_overlapping(content, &needle);
        if positions.is_empty() {
            // 文件当前内容与模型预期不符 → 409 Conflict（同旧锚点不匹配语义）
            let hint = closest_line(content, &needle)
                .map(|(n, l)| format!("；最接近的是第 {n} 行：{l}"))
                .unwrap_or_default();
            return Err(TianyanError::conflict(format!(
                "executor: apply_edit: 第{}处编辑：未找到要替换的原文（old_string 不在当前文件中）{hint}，请据此修正或重新 read_file 确认当前内容",
                i + 1
            )));
        }
        let replace_all = edit.replace_all.unwrap_or(false);
        if positions.len() > 1 && !replace_all {
            return Err(TianyanError::conflict(format!(
                "executor: apply_edit: 第{}处编辑：原文出现 {} 次（不唯一），请提供更多上下文或设 replace_all",
                i + 1,
                positions.len()
            )));
        }
        let old_len = needle.len();
        for pos in positions {
            spans.push((pos, pos + old_len, edit.new_string.as_str()));
        }
    }

    // 重叠检测：按起始位置升序，相邻区间起点落在前一区间内即重叠。
    let mut sorted = spans.clone();
    sorted.sort_by_key(|s| s.0);
    for w in sorted.windows(2) {
        if w[1].0 < w[0].1 {
            return Err(TianyanError::Custom(
                "executor: apply_edit: 编辑重叠：两条编辑的替换区间重叠".to_string(),
            ));
        }
    }

    // 自底向上应用（起始位置降序），保证较早位置不受影响。
    let mut by_start = spans;
    by_start.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut result = content.to_string();
    for (start, end, new_string) in by_start {
        result = format!("{}{}{}", &result[..start], new_string, &result[end..]);
    }
    Ok(result)
}

/// 异步动作：读取文件 → 纯函数应用 → 写回。
///
/// 返回结构化 JSON：path 与 edits_applied。
pub async fn apply_edit_action(path: &str, edits: Vec<ContentEdit>) -> Result<Value> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: apply_edit: 读取失败：{e}")))?;
    let new_content = apply_edits_to_content(&content, &edits)?;
    // T1-15：原子写（同目录临时文件 + rename）——避免半截文件
    crate::executor::write_file_atomic(path, &new_content, false)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: apply_edit: 写入失败：{e}")))?;
    Ok(json!({
        "path": path,
        "edits_applied": edits.len(),
    }))
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
