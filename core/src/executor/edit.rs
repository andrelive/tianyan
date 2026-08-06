//! 行锚点语义编辑（apply_edit 核心逻辑）。
//!
//! 哈希锚定：每条编辑以 `start_line`（1 起始）+ 可选 `anchor`（行内容的
//! 空白不敏感 2 位十六进制哈希）定位目标行。所有编辑先对照**原始内容**
//! 全量校验（任一失败即整批中止、不落盘，保证原子性），再按 `start_line`
//! **自底向上**应用，保证较早行号不受后续替换影响。
//!
//! 行尾风格：写回时保持原文件的 EOL（检测 `\r\n` / `\n`）；内部按行处理时
//! 剥离行尾 `\r`，因此 CRLF 文件经 apply_edit 后仍为 CRLF，不会被整体改写。
//! 空 `new_lines` 表示删除目标行。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::common::error::{Result, TianyanError};
use crate::executor::hashline;

/// 单次编辑允许的最大条数（1..=20）。
pub const MAX_EDITS: usize = 20;

/// 单条编辑规格。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditSpec {
    /// 目标起始行号（1 起始）。
    pub start_line: usize,
    /// 期望的行内容哈希（2 位十六进制，空白不敏感）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// 期望的旧内容（逐行哈希比对；缺省时视为单行替换）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_lines: Option<Vec<String>>,
    /// 替换行（空数组 = 删除目标行）。
    pub new_lines: Vec<String>,
}

/// 检测内容的行尾风格（含 `\r\n` 则整体按 CRLF 处理，否则 LF）。
pub fn detect_eol(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 纯函数：对内容应用编辑（校验 + 自底向上应用），不涉及文件系统。
///
/// 流程：
/// 1. 校验编辑条数（1..=20）与每条编辑的边界（`start_line` 1 起始、不越界）。
/// 2. 逐条对照**原始内容**校验锚点/旧内容哈希（任一失败即整批中止）。
/// 3. 检测重叠范围（按起始行自底向上排序后相邻区间相交即重叠）。
/// 4. 按 `start_line` 自底向上替换（较早行号不受后续替换影响）。
/// 5. 以检测到的行尾风格（CRLF/LF）join 写回，保留原尾随换行。
///
/// 行尾归一化：行内容统一剥离行尾 `\r` 后参与哈希/比较/输出，
/// 换行符由 `detect_eol` 检测并统一重新加入，因此 CRLF 文件保持 CRLF。
pub fn apply_edits_to_content(content: &str, edits: &[EditSpec]) -> Result<String> {
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

    let eol = detect_eol(content);
    // 剥离行尾 `\r` 并拆分为行；末尾 `\n` 产生的空串弹出，写回时恢复。
    let mut lines: Vec<String> = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let had_trailing_newline = content.ends_with('\n');
    if had_trailing_newline {
        lines.pop();
    }
    let total = lines.len();

    // ── 校验阶段（全部对照原始内容，任一失败即中止，不落盘）────────────
    for (idx, edit) in edits.iter().enumerate() {
        let i = idx + 1; // 1 起始编辑序号
        if edit.start_line == 0 {
            return Err(TianyanError::Custom(format!(
                "executor: apply_edit: 第{i}处编辑：起始行 0 无效（1 起始）"
            )));
        }
        if edit.start_line > total {
            return Err(TianyanError::Custom(format!(
                "executor: apply_edit: 第{i}处编辑：起始行 {} 超出文件总行数 {total}",
                edit.start_line
            )));
        }
        let old_len = edit.old_lines.as_ref().map_or(1, |v| v.len());
        let range_end = edit.start_line + old_len - 1;
        if range_end > total {
            return Err(TianyanError::Custom(format!(
                "executor: apply_edit: 第{i}处编辑：起始行 {} 起 {old_len} 行超出文件总行数 {total}",
                edit.start_line
            )));
        }

        let actual_line = &lines[edit.start_line - 1];
        let actual_hash = hashline::line_hash(actual_line);
        if let Some(expected) = &edit.anchor {
            if *expected != actual_hash {
                return Err(TianyanError::Custom(format!(
                    "executor: apply_edit: 第{i}处编辑：锚点不匹配 第{}行 期望 {expected} 实际 {actual_hash}，最新锚点为 {}",
                    edit.start_line,
                    hashline::format_line(edit.start_line, &actual_hash, actual_line)
                )));
            }
        }
        if let Some(old_lines) = &edit.old_lines {
            for (offset, expected_line) in old_lines.iter().enumerate() {
                let line_no = edit.start_line + offset;
                let actual = &lines[line_no - 1];
                if hashline::line_hash(actual) != hashline::line_hash(expected_line) {
                    return Err(TianyanError::Custom(format!(
                        "executor: apply_edit: 第{i}处编辑：旧内容不匹配 第{line_no}行 期望 {} 实际 {actual}",
                        expected_line.trim_end_matches('\r')
                    )));
                }
            }
        }
    }

    // ── 重叠检测：自底向上排序后，相邻区间起点落在前一区间内即重叠 ────
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by(|&a, &b| edits[b].start_line.cmp(&edits[a].start_line));
    for pair in order.windows(2) {
        let upper = &edits[pair[0]]; // 起始行较大者
        let lower = &edits[pair[1]];
        let upper_len = upper.old_lines.as_ref().map_or(1, |v| v.len());
        let lower_len = lower.old_lines.as_ref().map_or(1, |v| v.len());
        // 区间相交判定：[lower.start, lower.end) ∩ [upper.start, upper.end) ≠ ∅
        let overlaps = lower.start_line < upper.start_line + upper_len
            && upper.start_line < lower.start_line + lower_len;
        if overlaps {
            return Err(TianyanError::Custom(format!(
                "executor: apply_edit: 编辑重叠：第{}处编辑（起始行 {}）与第{}处编辑（起始行 {}）重叠",
                pair[1] + 1,
                lower.start_line,
                pair[0] + 1,
                upper.start_line
            )));
        }
    }

    // ── 应用阶段：自底向上（起始行降序）替换 ────────────────────────────
    for &i in &order {
        let edit = &edits[i];
        let old_len = edit.old_lines.as_ref().map_or(1, |v| v.len());
        let start_idx = edit.start_line - 1;
        lines.drain(start_idx..start_idx + old_len);
        for (offset, new_line) in edit.new_lines.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    let mut out = lines.join(eol);
    if had_trailing_newline {
        out.push_str(eol);
    }
    Ok(out)
}

/// 异步动作：读取文件 → 纯函数应用 → 写回。
///
/// 返回结构化 JSON：`path` 与 `edits_applied`。
pub async fn apply_edit_action(path: &str, edits: Vec<EditSpec>) -> Result<Value> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: apply_edit: 读取失败：{e}")))?;
    let new_content = apply_edits_to_content(&content, &edits)?;
    tokio::fs::write(path, new_content)
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: apply_edit: 写入失败：{e}")))?;
    Ok(json!({
        "path": path,
        "edits_applied": edits.len(),
    }))
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
