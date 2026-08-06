//! 代码搜索执行器（ripgrep 封装）。
//!
//! 输出契约（JSON，LLM 可见）：
//! ```json
//! { "pattern": "...", "path": "...", "results": [...], "count": N, "total": N,
//!   "truncated": bool, "exhausted": bool, "output_mode": "content" }
//! ```
//!
//! 守卫：单条记录文本 > 64KB 丢弃；单条记录 submatch 上限 100；单行显示截断
//! 2000 字符并追加 `…<truncated>`；`.git` 目录始终排除；总记录硬上限 5000。
//!
//! // allow: SIZE_OK — 任务约定单文件承载完整搜索实现（三种输出模式解析器 + 守卫
//! // + 分页流水线）；模块内聚单一职责，拆分仅增加间接层，故保持单文件。

use std::process::Stdio;

use serde_json::{json, Value};

use crate::common::error::TianyanError;

/// 单行最大显示字符数（超出截断并追加 [`TRUNCATED_MARKER`]）。
const MAX_LINE_CHARS: usize = 2000;
/// 单条记录（含上下文行）最大字节数：超出则丢弃整条记录。
const MAX_RECORD_BYTES: usize = 64 * 1024;
/// 单条记录 submatch 上限。
const MAX_SUBMATCHES: usize = 100;
/// 收集的记录总数硬上限（内存与输出体积保护）。
const MAX_TOTAL_MATCHES: usize = 5000;
/// 分页默认条数。
const DEFAULT_HEAD_LIMIT: usize = 200;
/// 截断标记。
const TRUNCATED_MARKER: &str = "…<truncated>";

/// 输出模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// 仅列出命中的文件名（默认）。
    FilesWithMatches,
    /// 匹配行 + 行号 + submatch 偏移。
    Content,
    /// 每个文件的匹配行数（`text` 字段为数字字符串）。
    Count,
}

impl OutputMode {
    /// 模式名（出现在输出 JSON 的 `output_mode` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            OutputMode::FilesWithMatches => "files_with_matches",
            OutputMode::Content => "content",
            OutputMode::Count => "count",
        }
    }

    /// 解析输出模式字符串；`None` 取默认 [`OutputMode::FilesWithMatches`]。
    fn parse(mode: Option<&str>) -> Result<Self, TianyanError> {
        match mode {
            None | Some("files_with_matches") => Ok(OutputMode::FilesWithMatches),
            Some("content") => Ok(OutputMode::Content),
            Some("count") => Ok(OutputMode::Count),
            Some(other) => Err(TianyanError::Custom(format!(
                "executor: 搜索失败：输出模式无效：{other}"
            ))),
        }
    }
}

/// 搜索选项（与工具参数一一对应；`None`/`false` 取默认行为）。
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// 搜索根目录（默认当前工作目录）。
    pub path: Option<String>,
    /// glob 过滤器（如 `*.rs`）。
    pub glob: Option<String>,
    /// 输出模式字符串（见 [`OutputMode::parse`]）。
    pub output_mode: Option<String>,
    /// 文件类型过滤器（rg `--type`，如 "rust"）。
    pub type_: Option<String>,
    /// 忽略大小写（`-i`）。
    pub ignore_case: bool,
    /// 是否显示行号（content 模式默认 true）。
    pub line_number: Option<bool>,
    /// 上下文行数（`-C`）。
    pub context: Option<usize>,
    /// 前文行数（`-B`）。
    pub before_context: Option<usize>,
    /// 后文行数（`-A`）。
    pub after_context: Option<usize>,
    /// 结果条数上限（默认 [`DEFAULT_HEAD_LIMIT`]）。
    pub head_limit: Option<usize>,
    /// 分页偏移（0 起始）。
    pub offset: usize,
    /// 多行匹配（`-U`）。
    pub multiline: bool,
}

/// 执行代码搜索。
///
/// 流程：构造 rg 命令 → 按退出码分流（0 匹配 / 1 无匹配 / 2 错误）→
/// 解析输出为记录（应用输出守卫）→ offset + head_limit 分页。
///
/// - `total` = 收集到的记录总数（含守卫丢弃前的上限 5000）。
/// - `truncated` = `offset + head_limit < total`（还有更多结果可翻页）。
/// - `exhausted` = `offset >= total`（已无更多结果）。
pub async fn execute_search_code(
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, TianyanError> {
    let mode = OutputMode::parse(options.output_mode.as_deref())?;
    let head_limit = options.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT).max(1);
    let offset = options.offset;
    let show_line_number = options.line_number.unwrap_or(mode == OutputMode::Content);

    let output = build_command(pattern, options, mode)
        .output()
        .await
        .map_err(|e| TianyanError::Custom(format!("executor: 搜索失败：执行 ripgrep 失败：{e}")))?;

    let records = match output.status.code() {
        Some(0) => match mode {
            OutputMode::FilesWithMatches => parse_files_with_matches(&output.stdout),
            OutputMode::Count => parse_counts(&output.stdout),
            OutputMode::Content => parse_content_records(&output.stdout, show_line_number),
        },
        // 退出码 1 = 无匹配（--files-with-matches/--count 亦同）。
        Some(1) => Vec::new(),
        Some(2) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if stderr.contains("regex parse error") {
                return Err(TianyanError::Custom(format!(
                    "executor: 搜索失败：正则无效：{stderr}"
                )));
            }
            return Err(TianyanError::Custom(format!(
                "executor: 搜索失败：ripgrep 执行失败：{stderr}"
            )));
        }
        _ => {
            return Err(TianyanError::Custom(
                "executor: 搜索失败：ripgrep 异常退出".to_string(),
            ));
        }
    };

    let total = records.len();
    let window_end = offset.saturating_add(head_limit).min(total);
    let results: Vec<Value> = if offset < total {
        records[offset..window_end].to_vec()
    } else {
        Vec::new()
    };

    let mut out = json!({
        "pattern": pattern,
        "results": results,
        "count": results.len(),
        "total": total,
        "truncated": offset.saturating_add(head_limit) < total,
        "exhausted": offset > 0 && offset >= total,
        "output_mode": mode.as_str(),
    });
    if let Some(dir) = &options.path {
        out["path"] = json!(dir);
    }
    Ok(out)
}

/// 构造 ripgrep 命令。
///
/// `--json` 仅用于 content 模式：`--files-with-matches` 与 `--count` 会覆盖
/// `--json`（实测输出纯文本），故这两者走各自纯文本解析。
fn build_command(
    pattern: &str,
    options: &SearchOptions,
    mode: OutputMode,
) -> tokio::process::Command {
    let mut cmd = rg_command();
    if let Some(dir) = &options.path {
        cmd.current_dir(dir);
    }
    if let Some(g) = &options.glob {
        cmd.arg("--glob").arg(g);
    }
    if let Some(t) = &options.type_ {
        cmd.arg("--type").arg(t);
    }
    if options.ignore_case {
        cmd.arg("-i");
    }
    if options.multiline {
        cmd.arg("-U");
    }
    match mode {
        OutputMode::FilesWithMatches => {
            // --null：NUL 分隔路径，避免换行/特殊字符歧义。
            cmd.arg("--null").arg("--files-with-matches");
        }
        OutputMode::Count => {
            cmd.arg("--count");
        }
        OutputMode::Content => {
            cmd.arg("--json").arg("--line-number");
            if let Some(c) = options.context {
                cmd.arg("-C").arg(c.to_string());
            }
            if let Some(b) = options.before_context {
                cmd.arg("-B").arg(b.to_string());
            }
            if let Some(a) = options.after_context {
                cmd.arg("-A").arg(a.to_string());
            }
        }
    }
    // `.git` 始终排除（rg 默认跳过隐藏目录，这里显式加固）。
    cmd.arg("--glob").arg("!.git");
    // `--` 分隔符：防止以 `-` 开头的模式被 rg 解析为选项。
    cmd.arg("--").arg(pattern);
    cmd
}

/// 解析 rg 可执行文件：`TIANYAN_RG` 环境变量优先（rg 安装在非 PATH 位置时可用），
/// 未设置时使用 PATH 中的 `rg`。
///
/// stdin 一律置 [`Stdio::null`]：tokio 默认给子进程创建 stdin 管道，Windows 上
/// 其它进程并发运行时 rg 的 stdin 启发式会误判（"heuristic chose to search
/// stdin"），静默改为搜索空 stdin 而非目标目录，返回 count=0 且无任何错误。
fn rg_command() -> tokio::process::Command {
    let mut cmd = if let Ok(bin) = std::env::var("TIANYAN_RG") {
        if !bin.is_empty() {
            tokio::process::Command::new(bin)
        } else {
            tokio::process::Command::new("rg")
        }
    } else {
        tokio::process::Command::new("rg")
    };
    cmd.stdin(Stdio::null());
    cmd
}

/// 解析 `--files-with-matches` 输出（NUL 分隔路径），每个文件一条记录。
fn parse_files_with_matches(stdout: &[u8]) -> Vec<Value> {
    let mut records = Vec::new();
    for path in String::from_utf8_lossy(stdout).split('\0') {
        if path.is_empty() || records.len() >= MAX_TOTAL_MATCHES {
            continue;
        }
        records.push(json!({
            "path": path,
            "line_number": 1,
            "text": "",
            "matches": [],
        }));
    }
    records
}

/// 解析 `--count` 输出（每行 `path:count`；路径可含 `:`，按最后一个冒号切分），
/// `text` 字段为计数字符串（与 content 模式的文本语义区分，字段保持一致）。
fn parse_counts(stdout: &[u8]) -> Vec<Value> {
    let mut records = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        if records.len() >= MAX_TOTAL_MATCHES {
            break;
        }
        let Some((path, count)) = line.rsplit_once(':') else {
            continue;
        };
        records.push(json!({
            "path": path,
            "line_number": 1,
            "text": count,
            "matches": [],
        }));
    }
    records
}

/// 组装中的 content 记录（匹配行 + 前后上下文行 + submatch）。
struct InProgress {
    path: String,
    line_number: Option<u64>,
    text: String,
    submatches: Vec<(usize, usize)>,
    before: Vec<String>,
    after: Vec<String>,
}

/// 解析 `--json` 输出为 content 记录（应用输出守卫：64KB 记录丢弃、submatch
/// 上限 100、单行截断、总记录上限 5000）。
///
/// rg 的 JSON 流按文件分组（begin → match*/context* → end）；上下文记录归属：
/// 路径相同的挂到当前记录的 `after`，否则进入 `before` 缓冲等待下一条匹配。
fn parse_content_records(stdout: &[u8], show_line_number: bool) -> Vec<Value> {
    let mut records: Vec<Value> = Vec::new();
    let mut before_buf: Vec<String> = Vec::new();
    let mut current: Option<InProgress> = None;

    for line in String::from_utf8_lossy(stdout).lines() {
        if records.len() >= MAX_TOTAL_MATCHES {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(typ) = value["type"].as_str() else {
            continue;
        };
        let data = &value["data"];
        match typ {
            "match" => {
                if let Some(cur) = current.take() {
                    finalize_record(&mut records, cur, show_line_number);
                }
                let path = data["path"]["text"].as_str().unwrap_or("").to_string();
                let text = data["lines"]["text"].as_str().unwrap_or("").to_string();
                let submatches = data["submatches"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .take(MAX_SUBMATCHES)
                            .filter_map(|m| {
                                Some((m["start"].as_u64()? as usize, m["end"].as_u64()? as usize))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                current = Some(InProgress {
                    path,
                    line_number: data["line_number"].as_u64(),
                    text,
                    submatches,
                    before: std::mem::take(&mut before_buf),
                    after: Vec::new(),
                });
            }
            "context" => {
                let ctx = data["lines"]["text"].as_str().unwrap_or("").to_string();
                let path = data["path"]["text"].as_str().unwrap_or("").to_string();
                match &mut current {
                    Some(cur) if cur.path == path => cur.after.push(ctx),
                    _ => before_buf.push(ctx),
                }
            }
            // 每个文件分组以 end 收尾：此时该文件的 after 上下文已全部到达。
            "end" => {
                if let Some(cur) = current.take() {
                    finalize_record(&mut records, cur, show_line_number);
                }
            }
            _ => {}
        }
    }
    if let Some(cur) = current.take() {
        finalize_record(&mut records, cur, show_line_number);
    }
    records
}

/// 组装单条 content 记录：行截断（2000 字符 + 标记）、submatch 偏移调整（相对
/// 含上下文行的文本）、记录体积守卫（64KB 丢弃）、越界 submatch 丢弃。
fn finalize_record(records: &mut Vec<Value>, rec: InProgress, show_line_number: bool) {
    if records.len() >= MAX_TOTAL_MATCHES {
        return;
    }
    let (match_line, kept_bytes) = truncate_line(&rec.text);
    let mut text = String::new();
    let mut prefix_bytes = 0usize;
    for line in &rec.before {
        let (shown, _) = truncate_line(line);
        prefix_bytes += shown.len() + 1;
        text.push_str(&shown);
        text.push('\n');
    }
    text.push_str(&match_line);
    for line in &rec.after {
        text.push('\n');
        text.push_str(&truncate_line(line).0);
    }
    if text.len() > MAX_RECORD_BYTES {
        return; // 超限记录直接丢弃
    }
    let matches: Vec<Value> = rec
        .submatches
        .iter()
        .filter(|(_, end)| *end <= kept_bytes)
        .map(|(start, end)| json!({ "start": start + prefix_bytes, "end": end + prefix_bytes }))
        .collect();
    let mut record = json!({
        "path": rec.path,
        "text": text,
        "matches": matches,
    });
    if show_line_number {
        record["line_number"] = json!(rec.line_number.unwrap_or(0));
    }
    records.push(record);
}

/// 单行显示：超过 [`MAX_LINE_CHARS`] 个字符的行截断并追加 [`TRUNCATED_MARKER`]。
///
/// 返回（显示文本, 保留区的字节长度）——保留区字节长度用于过滤越界 submatch
/// （submatch 偏移是相对原始行的字节偏移，截断后落在保留区之外的直接丢弃）。
fn truncate_line(line: &str) -> (String, usize) {
    if line.chars().count() > MAX_LINE_CHARS {
        let mut shown: String = line.chars().take(MAX_LINE_CHARS).collect();
        let kept_bytes = shown.len();
        shown.push_str(TRUNCATED_MARKER);
        (shown, kept_bytes)
    } else {
        (line.to_string(), line.len())
    }
}

/// 测试环境：确保 `rg` 可调用（本机 rg 不在进程 PATH 中，位于
/// `~/.cache/opencode/bin`）。
///
/// 通过 `TIANYAN_RG` 注入二进制路径而非修改全局 PATH——避免影响同一测试进程内
/// 其它并发测试（如 executor::fs 的 rg 探测）。测试线程并发执行，全部写入相同值，
/// 覆盖写竞态是良性的。
#[cfg(test)]
pub(crate) fn ensure_rg_available() {
    use std::path::PathBuf;

    if std::process::Command::new("rg")
        .arg("--version")
        .output()
        .is_ok()
    {
        return;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        candidates.push(
            PathBuf::from(home)
                .join(".cache")
                .join("opencode")
                .join("bin"),
        );
    }
    candidates.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
    candidates.push(PathBuf::from(r"~\scoop\shims"));
    for dir in candidates {
        let bin = dir.join("rg.exe");
        if bin.exists() {
            std::env::set_var("TIANYAN_RG", bin);
            return;
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
