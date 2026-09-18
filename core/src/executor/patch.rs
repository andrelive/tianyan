//! 统一 diff 补丁解析与应用（apply_patch 核心逻辑）。
//!
//! 解析 codex 风格 `*** Update File:` 信封补丁（对齐 omo）：每个文件一个头部
//! + 若干 `-`/`+`/上下文行。`@@` 块头**可选**（模型可省略，降低出错面）；
//! 块定位采用"提示位置精确匹配 → 全文件精确匹配 → 模糊匹配（similar 行级
//! ratio ≥ 阈值）"三级策略；全部块定位成功后才落盘（批量原子性）。写回时
//! 保持目标文件原有 CRLF/LF 行尾。
//!
//! 围栏容错：Claude Code 风格补丁带 `*** Begin Patch` / `*** End Patch` 围栏
//! 头尾（LLM 输出惯性夹带），本工具无围栏（直接 `*** Update File:` 起止）——
//! 已知围栏行一律跳过（与空行同策略），未知 `***` 头仍报错。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::common::error::{Result, TianyanError};
use crate::executor::edit;

/// 模糊匹配的最低相似度阈值（`similar::TextDiff::ratio`，0..=1）。
///
/// 平衡 codex 的宽松（0.65）与严格（0.8）：太低会误改无关位置，
/// 太高会让轻微漂移的上下文匹配失败。
pub const FUZZY_RATIO_THRESHOLD: f32 = 0.75;

/// 补丁中的单个文件条目。
#[derive(Debug, Clone)]
pub struct PatchFile {
    /// 补丁头部声明的文件路径（相对或绝对）。
    pub path: String,
    /// 该文件的补丁块列表。
    pub hunks: Vec<PatchHunk>,
}

/// 单个 `@@` 补丁块。
#[derive(Debug, Clone)]
pub struct PatchHunk {
    /// 旧起始行号（内部 0 起始；文本中为 1 起始）。
    pub old_start: usize,
    /// 旧文件行数（文本中逗号后数值；0 表示纯插入/创建）。
    pub old_count: usize,
    /// 新文件行数。
    pub new_count: usize,
    /// 块体行。
    pub lines: Vec<PatchLine>,
}

/// 块内单行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchLine {
    /// 上下文行（空格前缀）。
    Context(String),
    /// 删除行（`-` 前缀）。
    Remove(String),
    /// 新增行（`+` 前缀）。
    Add(String),
}

/// 单个文件的待写入结果（定位/计算阶段产物，落盘前统一持有）。
struct PlannedWrite {
    full: PathBuf,
    orig_path: String,
    content: String,
    hunks_applied: usize,
    lines_changed: usize,
}

// ── 解析 ─────────────────────────────────────────────────────────────────

/// 解析补丁文本为文件条目列表。
///
/// 信封格式（codex/opencode 风格）：
/// ```text
/// *** Update File: src/main.rs
/// @@ -12,3 +12,4 @@
///  context line
/// -old line
/// +new line
/// ```
/// 解析错误统一为 `executor: apply_patch: 解析失败: {line}: {detail}`。
pub fn parse_patch(text: &str) -> Result<Vec<PatchFile>> {
    let mut files: Vec<PatchFile> = Vec::new();
    let mut cur_file: Option<PatchFile> = None;
    let mut cur_hunk: Option<PatchHunk> = None;

    for (idx, raw) in text.split('\n').enumerate() {
        let line_num = idx + 1;
        let line = raw.trim_end_matches('\r');

        if let Some(rest) = line.strip_prefix("*** Update File: ") {
            // 闭合当前块与当前文件
            close_hunk(&mut cur_hunk, &mut cur_file, line_num)?;
            let path = rest.trim().to_string();
            if path.is_empty() {
                return Err(parse_err(line_num, "文件路径为空"));
            }
            if let Some(pf) = cur_file.take() {
                if pf.hunks.is_empty() {
                    // 空文件段一律报错（静默丢弃 = 静默失败，模型会以为改过但实际没改）：
                    // 同路径 = 重复头；异路径 = 该文件缺少补丁内容
                    if pf.path == path {
                        return Err(parse_err(
                            line_num,
                            format!("重复的 *** Update File 头: {path}"),
                        ));
                    }
                    return Err(parse_err(
                        line_num,
                        format!("文件缺少补丁内容: {}", pf.path),
                    ));
                }
                files.push(pf);
            }
            cur_file = Some(PatchFile {
                path,
                hunks: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("*** ") {
            // 未知信封头：rename 等不支持的补丁操作单独点名
            if line.to_lowercase().contains("rename") {
                return Err(TianyanError::Custom(
                    "executor: apply_patch: 不支持的补丁操作：rename".to_string(),
                ));
            }
            // 已知围栏行（Claude Code 风格 Begin/End 信封）：模型输出惯性
            // 夹带，但围栏不携带语义——跳过（与空行忽略同策略；出现在文件
            // 头前、文件间、文件尾均安全）。未知 `***` 头仍报错。
            let rest_lower = rest.trim().to_lowercase();
            if matches!(
                rest_lower.as_str(),
                "begin" | "end" | "begin patch" | "end patch"
            ) {
                continue;
            }
            return Err(parse_err(line_num, format!("未知的信封头部: {rest}")));
        } else if line.starts_with("@@") {
            let (old_start, old_count, new_count) = parse_hunk_header(line, line_num)?;
            if cur_file.is_none() {
                return Err(parse_err(line_num, "@@ 块出现在 *** Update File 头部之前"));
            }
            close_hunk(&mut cur_hunk, &mut cur_file, line_num)?;
            cur_hunk = Some(PatchHunk {
                old_start,
                old_count,
                new_count,
                lines: Vec::new(),
            });
        } else if line.is_empty() {
            // **完全空行**：格式噪声（补丁文本尾换行 / LLM 夹带的分隔行）→ 忽略。
            //
            // ⚠️ 不得用 `line.trim().is_empty()`：「一个空格」是**显式空上下文
            // 行**（patch 格式约定：上下文行以空格前缀书写，内容可为空）。把
            // 它当空行丢掉，会让期望匹配窗口少一行、与文件不再精确匹配，继而
            // 被模糊匹配"整体平移一行"命中——即静默改错位置（历史 bug）。
        } else if line.starts_with('\\') {
            // `\ No newline at end of file` 标记：忽略（保持简单）。
        } else {
            // 对齐 omo：@@ 可选——文件头后可直接跟 -/+ /上下文行，自动开块
            //（位置未知，靠内容定位；模型无需写 @@ 行号，降低出错面）
            if cur_hunk.is_none() {
                if cur_file.is_none() {
                    return Err(parse_err(line_num, "补丁行出现在 *** Update File 头部之前"));
                }
                cur_hunk = Some(PatchHunk {
                    old_start: usize::MAX,
                    old_count: usize::MAX,
                    new_count: usize::MAX,
                    lines: Vec::new(),
                });
            }
            if let Some(hunk) = cur_hunk.as_mut() {
                hunk.lines.push(parse_body_line(line, line_num)?);
            }
        }
    }

    close_hunk(&mut cur_hunk, &mut cur_file, text.split('\n').count())?;
    if let Some(pf) = cur_file.take() {
        if pf.hunks.is_empty() {
            // 结尾空文件段同样报错（模型写了头但没写内容）
            return Err(parse_err(
                text.split('\n').count(),
                format!("文件缺少补丁内容: {}", pf.path),
            ));
        }
        files.push(pf);
    }
    if files.is_empty() {
        return Err(TianyanError::Custom(
            "executor: apply_patch: 解析失败: 补丁为空或缺少补丁内容".to_string(),
        ));
    }
    Ok(files)
}

/// 将已完成的块挂入当前文件，并做一致性校验（0 范围与删除/新增行冲突）。
fn close_hunk(
    cur_hunk: &mut Option<PatchHunk>,
    cur_file: &mut Option<PatchFile>,
    line_num: usize,
) -> Result<()> {
    let Some(hunk) = cur_hunk.take() else {
        return Ok(());
    };
    if hunk.old_count == 0 && hunk.lines.iter().any(|l| matches!(l, PatchLine::Remove(_))) {
        return Err(parse_err(line_num, "删除行与旧文件范围 0 冲突"));
    }
    if hunk.new_count == 0 && hunk.lines.iter().any(|l| matches!(l, PatchLine::Add(_))) {
        return Err(parse_err(line_num, "新增行与新文件范围 0 冲突"));
    }
    if let Some(pf) = cur_file.as_mut() {
        pf.hunks.push(hunk);
    }
    Ok(())
}

/// 解析 @@ -old,count +new,count @@ 块头，返回 (old_start 0 起始, old_count, new_count)。
///
/// 宽容解析：模型常漏写结束 @@ 或行号范围。缺范围时返回哨兵
/// usize::MAX 表示"位置未知"，由内容匹配（locate_hunk）定位，而非报错。
fn parse_hunk_header(line: &str, _line_num: usize) -> Result<(usize, usize, usize)> {
    let Some(inner) = line
        .strip_prefix("@@")
        .and_then(|s| s.find("@@").map(|p| &s[..p]))
    else {
        // 裸 @@（无结束 @@）：位置未知
        return Ok((usize::MAX, usize::MAX, usize::MAX));
    };
    let mut old: Option<(usize, usize)> = None;
    let mut new: Option<(usize, usize)> = None;
    for token in inner.split_whitespace() {
        let mut chars = token.chars();
        let Some(sign) = chars.next() else {
            continue;
        };
        let rest = chars.as_str();
        if rest.is_empty() {
            continue;
        }
        let (start_s, count_s) = match rest.split_once(',') {
            Some((s, c)) => (s, c),
            None => (rest, "1"),
        };
        let Ok(start) = start_s.parse::<usize>() else {
            continue;
        };
        let Ok(count) = count_s.parse::<usize>() else {
            continue;
        };
        match sign {
            '-' => old = Some((start, count)),
            '+' => new = Some((start, count)),
            _ => {}
        }
    }
    let (Some((old_start, old_count)), Some((_new_start, new_count))) = (old, new) else {
        // 缺 -/+ 范围：位置未知，靠内容定位
        return Ok((usize::MAX, usize::MAX, usize::MAX));
    };
    // 文本 1 起始 → 内部 0 起始（old_start=0 表示创建场景，无需减 1）
    let old_start0 = if old_start == 0 { 0 } else { old_start - 1 };
    Ok((old_start0, old_count, new_count))
}

/// 解析块体单行（空格/-/+ 前缀）。
fn parse_body_line(line: &str, line_num: usize) -> Result<PatchLine> {
    if let Some(rest) = line.strip_prefix(' ') {
        Ok(PatchLine::Context(rest.to_string()))
    } else if let Some(rest) = line.strip_prefix('-') {
        Ok(PatchLine::Remove(rest.to_string()))
    } else if let Some(rest) = line.strip_prefix('+') {
        Ok(PatchLine::Add(rest.to_string()))
    } else {
        Err(parse_err(line_num, format!("非补丁行: {line}")))
    }
}

fn parse_err(line_num: usize, detail: impl std::fmt::Display) -> TianyanError {
    TianyanError::Custom(format!(
        "executor: apply_patch: 解析失败: {line_num}: {detail}"
    ))
}

// ── 定位（精确 → 模糊） ───────────────────────────────────────────────────

/// 行归一化：剥离行尾 `\r` / 空格 / 制表符（匹配与哈希容错）。
fn norm_line(s: &str) -> &str {
    s.trim_end_matches(['\r', ' ', '\t'])
}

/// 构造块的期望匹配窗口：全部 Context + Remove 行（归一化），
/// 忽略首尾为空白行的 Context（Remove 必须参与匹配，不得裁剪）。
///
/// 返回 `(窗口行, 前导裁剪的空 Context 数)`——apply 阶段必须跳过同样数量
/// 的前导空 Context（不消费文件行），否则与文件行的消费计数错位。
fn match_window(hunk: &PatchHunk) -> (Vec<String>, usize) {
    let mut entries: Vec<(bool, String)> = Vec::new();
    for l in &hunk.lines {
        match l {
            PatchLine::Context(s) | PatchLine::Remove(s) => {
                let is_remove = matches!(l, PatchLine::Remove(_));
                entries.push((is_remove, norm_line(s).to_string()));
            }
            PatchLine::Add(_) => {}
        }
    }
    let mut start = 0;
    let mut end = entries.len();
    while start < end && !entries[start].0 && entries[start].1.is_empty() {
        start += 1;
    }
    while end > start && !entries[end - 1].0 && entries[end - 1].1.is_empty() {
        end -= 1;
    }
    let leading_trimmed = start;
    (
        entries[start..end].iter().map(|(_, s)| s.clone()).collect(),
        leading_trimmed,
    )
}

/// 定位块：返回 `(替换范围 [start, end)（0 起始，end 排他）, 前导裁剪数)`。
///
/// 策略：纯插入按 old_start 定位；否则提示位置（±1）精确匹配 →
/// 全文件精确匹配 → 全文件模糊匹配（similar 行级 ratio）。
fn locate_hunk(lines: &[String], hunk: &PatchHunk) -> Option<(usize, usize, usize)> {
    // 裸 @@（无行号范围）→ 位置未知，跳过行号定位，直接按内容匹配
    let no_hint = hunk.old_start == usize::MAX;
    if !no_hint && hunk.old_count == 0 {
        // 纯插入（创建/追加）：位置钳制到文件末尾
        let pos = hunk.old_start.min(lines.len());
        return Some((pos, pos, 0));
    }
    let (expected, leading_trimmed) = match_window(hunk);
    let win_len = expected.len();
    if win_len == 0 {
        // 全空白 Context 块：无操作；无提示时追加到末尾
        let pos = if no_hint {
            lines.len()
        } else {
            hunk.old_start.min(lines.len())
        };
        return Some((pos, pos, leading_trimmed));
    }

    // 1. 提示位置精确匹配（±1 容差，消歧重复内容）——仅当有位置提示
    if !no_hint {
        for cand in [
            hunk.old_start,
            hunk.old_start.saturating_sub(1),
            hunk.old_start.saturating_add(1),
        ] {
            if exact_match(lines, cand, &expected) {
                return Some((cand, cand + win_len, leading_trimmed));
            }
        }
    }
    // 2. 全文件精确匹配
    let last_start = lines.len().saturating_sub(win_len);
    for start in 0..=last_start {
        if exact_match(lines, start, &expected) {
            return Some((start, start + win_len, leading_trimmed));
        }
    }
    // 3. 模糊匹配：similar 行级 ratio，取最佳达标窗口；
    //    同等 ratio 时优先提示位置附近的窗口（消歧重复/相似区域）。
    //    另加**位置敏感守卫**（`positional_match_ratio`）：非空行须逐位置
    //    对应且匹配率达标——"整体平移一行"（期望首行落到窗口第 2 位）几乎
    //    全位置不匹配而被拒（防静默错位），"个别行抄写误差"只损失少量位置
    //    仍可达标（保留模糊匹配的正当容错）。
    let mut best: Option<(f32, usize, usize)> = None;
    for start in 0..=last_start {
        let ratio = fuzzy_ratio(lines, start, win_len, &expected);
        if ratio < FUZZY_RATIO_THRESHOLD {
            continue;
        }
        if positional_match_ratio(lines, start, win_len, &expected) < FUZZY_RATIO_THRESHOLD {
            continue;
        }
        let dist = if no_hint {
            0
        } else {
            start.abs_diff(hunk.old_start)
        };
        let better = match best {
            Some((b_ratio, b_dist, _)) => ratio > b_ratio || (ratio == b_ratio && dist < b_dist),
            None => true,
        };
        if better {
            best = Some((ratio, dist, start));
        }
    }
    best.map(|(_, _, start)| (start, start + win_len, leading_trimmed))
}

/// 窗口精确匹配（归一化后逐行相等）。
fn exact_match(lines: &[String], start: usize, expected: &[String]) -> bool {
    if expected.len() > lines.len().saturating_sub(start) {
        return false;
    }
    lines[start..start + expected.len()]
        .iter()
        .zip(expected)
        .all(|(a, b)| norm_line(a) == norm_line(b))
}

/// 窗口模糊相似度：`similar::TextDiff::from_slices` 的行级 ratio（0..=1）。
fn fuzzy_ratio(lines: &[String], start: usize, win_len: usize, expected: &[String]) -> f32 {
    let window: Vec<&str> = lines[start..start + win_len]
        .iter()
        .map(|s| norm_line(s))
        .collect();
    let expected_s: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
    similar::TextDiff::from_slices(&expected_s, &window).ratio()
}
/// 逐位置**非空行**匹配率（0..=1）：窗口与期望的非空行按顺序一一比较。
///
/// 位置敏感是刻意的：
/// - **整体平移一行**（期望首行落到窗口第 2 位）→ 几乎所有位置都不匹配 →
///   被拒（该形态的 `fuzzy_ratio` 仍可能达标，故不能只靠 ratio）；
/// - **个别行抄写误差**（LLM 上下文有 1 行不同）→ 只损失少量位置 → 仍可
///   达标（模糊匹配的正当用途，保留）。
///
/// 非空行数不一致（结构不同）直接判 0；期望无非空行（纯空上下文）判 1。
fn positional_match_ratio(
    lines: &[String],
    start: usize,
    win_len: usize,
    expected: &[String],
) -> f32 {
    let exp: Vec<&str> = expected
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    if exp.is_empty() {
        return 1.0;
    }
    let win: Vec<&str> = lines[start..start + win_len]
        .iter()
        .map(|s| norm_line(s))
        .filter(|s| !s.is_empty())
        .collect();
    if win.len() != exp.len() {
        return 0.0;
    }
    let hit = exp.iter().zip(win.iter()).filter(|(e, w)| e == w).count();
    hit as f32 / exp.len() as f32
}

// ── 应用 ─────────────────────────────────────────────────────────────────

/// 纯函数：对内容应用补丁块（定位 + 自底向上），不涉及文件系统。
///
/// 行尾风格保持：目标内容含 `\r\n` 则整体按 CRLF 写回，否则 LF。
pub fn apply_patch_to_content(content: &str, hunks: &[PatchHunk]) -> Result<String> {
    let (out, _, _) = apply_hunks(content, hunks, None)?;
    Ok(out)
}

/// 定位 + 应用（`label` 携带文件名用于定位错误；`None` 为纯函数调用）。
///
/// 1. 全部块对照**原始**内容定位（任一失败 → 整体中止，不落盘）。
/// 2. 重叠检测（自底向上排序后相邻范围相交即重叠）。
/// 3. 自底向上应用（起始位置降序），较早行号不受后续替换影响。
fn apply_hunks(
    content: &str,
    hunks: &[PatchHunk],
    label: Option<&str>,
) -> Result<(String, usize, usize)> {
    let eol = edit::detect_eol(content);
    // 空内容特例：split_lines 对空串返回空行列表（无尾随换行语义）
    let (mut lines, had_trailing_newline) = if content.is_empty() {
        (Vec::new(), false)
    } else {
        edit::split_lines(content)
    };

    // 定位阶段：全部对照原始内容
    let mut located: Vec<(usize, usize, usize)> = Vec::with_capacity(hunks.len());
    for (idx, hunk) in hunks.iter().enumerate() {
        let Some((s, e, leading_trimmed)) = locate_hunk(&lines, hunk) else {
            let detail = match label {
                Some(name) => format!("无法定位补丁块（文件 {name}，块 {}）", idx + 1),
                None => format!("无法定位补丁块（块 {}）", idx + 1),
            };
            return Err(TianyanError::conflict(format!(
                "executor: apply_patch: {detail}"
            )));
        };
        located.push((s, e, leading_trimmed));
    }

    // 重叠检测 + 自底向上应用顺序（区间判定与 apply_edit 同一实现）
    let mut order: Vec<usize> = (0..hunks.len()).collect();
    order.sort_by(|&a, &b| located[b].0.cmp(&located[a].0));
    let ranges: Vec<(usize, usize)> = located.iter().map(|&(s, e, _)| (s, e)).collect();
    if let Some((a, b)) = edit::find_overlap(&ranges) {
        return Err(TianyanError::conflict(format!(
            "executor: apply_patch: 补丁块重叠：块 {a} 与块 {b}"
        )));
    }

    for &i in &order {
        let (s, e, leading_trimmed) = located[i];
        // 替换段：Context 取匹配窗口内的**文件原行**（模糊漂移时不得把补丁
        // 中的陈旧上下文写回文件），Add 取补丁行，Remove 仅消费窗口行。
        // 前导裁剪的空 Context（match_window 已从窗口剔除）不消费文件行，
        // 保持与窗口的文件行消费计数对齐——否则 Remove 消费错位（删除被吞）。
        let mut replacement: Vec<String> = Vec::new();
        let mut file_idx = s;
        let mut skip_blank = leading_trimmed;
        for l in &hunks[i].lines {
            match l {
                PatchLine::Context(c) => {
                    if skip_blank > 0 && norm_line(c).is_empty() {
                        skip_blank -= 1;
                    } else if file_idx < e {
                        replacement.push(lines[file_idx].clone());
                        file_idx += 1;
                    }
                }
                PatchLine::Remove(_) => {
                    file_idx += 1;
                }
                PatchLine::Add(s) => replacement.push(s.clone()),
            }
        }
        lines.splice(s..e, replacement);
    }

    let lines_changed: usize = hunks
        .iter()
        .map(|h| {
            h.lines
                .iter()
                .filter(|l| matches!(l, PatchLine::Remove(_) | PatchLine::Add(_)))
                .count()
        })
        .sum();

    let out = edit::join_lines(&lines, eol, had_trailing_newline);
    Ok((out, hunks.len(), lines_changed))
}

// ── 异步动作（原子多文件） ───────────────────────────────────────────────

/// 解析并校验补丁内单文件路径。
///
/// - 拒绝 `..` 组件（正反斜杠统一检测），防目录逃逸。
/// - 拒绝绝对路径：补丁路径必须相对 `base_dir` 解析——多文件补丁的后续文件
///   若允许绝对路径，可绕过 registry 层基于"当前目录"的逐文件路径安全检查
///   （check_path 对相对路径按 CWD 解析，对绝对路径按字面使用，二者基准不同）。
fn resolve_patch_path(patch_path: &str, base_dir: &Path) -> Result<PathBuf> {
    let normalized = patch_path.replace('\\', "/");
    let p = Path::new(&normalized);
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(TianyanError::invalid_input(format!(
            "executor: apply_patch: 非法路径: {patch_path}"
        )));
    }
    if p.is_absolute() {
        return Err(TianyanError::invalid_input(format!(
            "executor: apply_patch: 不支持绝对路径：{patch_path}"
        )));
    }
    Ok(base_dir.join(p))
}

/// 提取补丁中全部 `*** Update File:` 文件路径（按出现顺序，去空白）。
///
/// 与 [`parse_patch`] 的头部识别规则一致（轻量行扫描，不做完整解析）；
/// 供 registry 层在审批门控与落盘之前对每个目标文件逐一执行路径安全检查。
pub(crate) fn collect_patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|l| {
            l.trim_end_matches('\r')
                .strip_prefix("*** Update File: ")
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// 提取补丁的首个 `*** Update File:` 路径（供审批展示；无则空串）。
pub fn first_patch_path(patch_text: &str) -> String {
    patch_text
        .lines()
        .find_map(|l| {
            l.trim_end_matches('\r')
                .strip_prefix("*** Update File: ")
                .map(str::trim)
        })
        .map(str::to_string)
        .unwrap_or_default()
}

/// 异步动作：解析 → 全量定位校验 → 原子落盘。
///
/// 所有文件的块全部定位成功后才写入任一文件；任一失败则零写入。
/// 返回 `{ "files": [{ path, hunks_applied, lines_changed }], "total_files" }`。
pub async fn apply_patch_action(patch_text: &str, base_dir: &Path) -> Result<Value> {
    let files = parse_patch(patch_text)?;

    let mut planned: Vec<PlannedWrite> = Vec::with_capacity(files.len());
    for pf in &files {
        let full = resolve_patch_path(&pf.path, base_dir)?;
        // 存在旧行（old_count > 0）的块要求文件已存在；
        // 纯创建（全部 old_count == 0）允许缺失，按空内容处理。
        let needs_existing = pf.hunks.iter().any(|h| h.old_count > 0);
        let content = match tokio::fs::read_to_string(&full).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if needs_existing {
                    return Err(TianyanError::not_found(format!(
                        "executor: apply_patch: 文件不存在: {}",
                        pf.path
                    )));
                }
                String::new()
            }
            Err(e) => {
                return Err(TianyanError::Custom(format!(
                    "executor: apply_patch: 读取失败: {e}"
                )));
            }
        };
        let (new_content, hunks_applied, lines_changed) =
            apply_hunks(&content, &pf.hunks, Some(&pf.path))?;
        planned.push(PlannedWrite {
            full,
            orig_path: pf.path.clone(),
            content: new_content,
            hunks_applied,
            lines_changed,
        });
    }

    // 全部定位成功 → 统一落盘
    let mut files_json: Vec<Value> = Vec::with_capacity(planned.len());
    for write in planned {
        // T1-15：原子写（同目录临时文件 + rename）——避免半截文件
        crate::executor::write_file_atomic(&write.full, &write.content)
            .await
            .map_err(|e| TianyanError::Custom(format!("executor: apply_patch: 写入失败: {e}")))?;
        files_json.push(json!({
            "path": write.orig_path,
            "hunks_applied": write.hunks_applied,
            "lines_changed": write.lines_changed,
        }));
    }
    Ok(json!({ "files": files_json, "total_files": files_json.len() }))
}

/// 测试模块（拆分至独立文件，保持主文件聚焦生产逻辑）。
#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
