//! 文件系统浏览工具核心逻辑：`glob` 查找文件与 `list_dir` 列出目录条目。
//!
//! `glob` 优先使用 `rg --files`（原生支持 .gitignore），rg 不可用时回退为
//! 手工递归遍历（不支持 .gitignore —— 文档化降级行为，跳过隐藏条目与
//! [`EXCLUDED_DIRS`]）。结果统一按修改时间倒序排列，硬上限 [`MAX_GLOB_RESULTS`]。
//!
//! `list_dir` 仅列单个目录层级：目录在前、组内字节序排序（locale 无关），
//! 支持 offset/limit 分页（排序后应用）。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::common::error::TianyanError;

/// glob 结果硬上限（设计文档 4.3：最多返回 200 条，超出置 truncated）。
pub const MAX_GLOB_RESULTS: usize = 200;

/// 回退遍历时跳过的目录（rg 由 .gitignore/隐藏规则处理，回退路径需手工排除）。
pub const EXCLUDED_DIRS: &[&str] = &[".git", "node_modules", "target"];

/// glob 工具输出。
#[derive(Debug, Clone, Serialize)]
pub struct GlobOutput {
    /// 原始 glob 模式。
    pub pattern: String,
    /// 搜索根目录（绝对路径）。
    pub path: String,
    /// 命中的文件绝对路径（按修改时间倒序，最多 [`MAX_GLOB_RESULTS`] 条）。
    pub results: Vec<String>,
    /// 实际返回条数。
    pub count: usize,
    /// 是否因上限被截断。
    pub truncated: bool,
}

/// list_dir 单条目输出。
#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    /// "dir" 或 "file"。
    #[serde(rename = "type")]
    pub entry_type: String,
    /// 条目名（目录带尾部 `/`）。
    pub name: String,
    /// 完整绝对路径。
    pub path: String,
}

/// list_dir 工具输出。
#[derive(Debug, Clone, Serialize)]
pub struct ListDirOutput {
    /// 列出的目录路径。
    pub path: String,
    /// 分页后的条目。
    pub entries: Vec<DirEntry>,
    /// 本页条目数。
    pub count: usize,
    /// 是否因分页被截断（offset+limit < total）。
    pub truncated: bool,
    /// 完整条目总数（分页前）。
    pub total: usize,
}

/// 执行 glob 查找：rg 优先，回退手工遍历；结果按修改时间倒序，上限
/// [`MAX_GLOB_RESULTS`]。`path` 为 None 时默认当前工作目录。
pub async fn execute_glob(pattern: &str, path: Option<&Path>) -> Result<GlobOutput, TianyanError> {
    let base = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| TianyanError::Custom(format!("executor: glob 获取当前目录失败：{e}")))?,
    };
    if !base.is_dir() {
        return Err(TianyanError::not_found(format!(
            "executor: 目录不存在：{}",
            base.display()
        )));
    }

    let rel_paths = match collect_rg_files(&base, pattern).await {
        Ok(Some(paths)) => paths,
        Ok(None) => collect_walk_files(&base, pattern).await?,
        Err(e) => return Err(e),
    };

    // 按修改时间倒序（新的在前），硬上限截断
    let mut with_mtime: Vec<(PathBuf, SystemTime)> = Vec::with_capacity(rel_paths.len());
    for rel in rel_paths {
        let abs = base.join(&rel);
        let mtime = tokio::fs::metadata(&abs)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        with_mtime.push((abs, mtime));
    }
    with_mtime.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));
    let truncated = with_mtime.len() > MAX_GLOB_RESULTS;
    let results: Vec<String> = with_mtime
        .iter()
        .take(MAX_GLOB_RESULTS)
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();
    Ok(GlobOutput {
        pattern: pattern.to_string(),
        path: base.to_string_lossy().into_owned(),
        count: results.len(),
        results,
        truncated,
    })
}

/// 列出单个目录层级：目录在前、组内字节序排序，offset/limit 排序后应用。
pub async fn execute_list_dir(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ListDirOutput, TianyanError> {
    if !path.is_dir() {
        return Err(TianyanError::not_found(format!(
            "executor: 目录不存在：{}",
            path.display()
        )));
    }
    let mut rd = tokio::fs::read_dir(path).await.map_err(|e| {
        TianyanError::Custom(format!("executor: 读取目录失败 {}：{e}", path.display()))
    })?;
    let mut entries: Vec<DirEntry> = Vec::new();
    while let Some(entry) = rd.next_entry().await.map_err(|e| {
        TianyanError::Custom(format!("executor: 读取目录失败 {}：{e}", path.display()))
    })? {
        let file_type = match entry.file_type().await {
            Ok(t) => t,
            Err(_) => continue, // 单个条目读取失败跳过，不中断整体列举
        };
        let is_dir = file_type.is_dir();
        if !is_dir && !file_type.is_file() {
            continue; // 符号链接等特殊条目跳过
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push(DirEntry {
            entry_type: if is_dir { "dir" } else { "file" }.to_string(),
            name: if is_dir { format!("{name}/") } else { name },
            path: entry.path().to_string_lossy().into_owned(),
        });
    }
    // 目录在前（"dir" < "file" 字节序），组内按字节序（locale 无关）
    entries.sort_by(|a, b| {
        a.entry_type
            .cmp(&b.entry_type)
            .then_with(|| a.name.cmp(&b.name))
    });

    let total = entries.len();
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(usize::MAX);
    let truncated = offset.saturating_add(limit) < total;
    let page: Vec<DirEntry> = entries.into_iter().skip(offset).take(limit).collect();
    Ok(ListDirOutput {
        path: path.to_string_lossy().into_owned(),
        count: page.len(),
        truncated,
        total,
        entries: page,
    })
}

/// 尝试用 `rg --files --glob <pattern> --glob '!.git' <base>` 收集相对路径列表。
///
/// - `Ok(Some(paths))`：rg 执行成功（.gitignore 原生生效）
/// - `Ok(None)`：rg 不可用（spawn NotFound），调用方应回退手工遍历
/// - `Err`：rg 启动/执行出错（真实故障，不静默回退）
async fn collect_rg_files(
    base: &Path,
    pattern: &str,
) -> Result<Option<Vec<PathBuf>>, TianyanError> {
    let output = match crate::executor::command::hide_console_window(
        tokio::process::Command::new("rg"),
    )
    .arg("--files")
        .arg("--glob")
        .arg(pattern)
        .arg("--glob")
        .arg("!.git")
        .arg(base)
        // stdin 置 null：tokio 默认创建 stdin 管道，Windows 并发场景下 rg 的
        // stdin 启发式会误判为"搜索 stdin"（同 search.rs rg_command 的缺陷），
        // 静默返回空结果而非目标目录的 --files 输出。
        .stdin(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(TianyanError::Custom(format!("executor: rg 启动失败：{e}"))),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            return Err(TianyanError::Custom(format!(
                "executor: rg 执行失败：{stderr}"
            )));
        }
        return Ok(Some(Vec::new())); // 无匹配（rg 退出码 1）
    }
    let paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    Ok(Some(paths))
}

/// 展开 glob 模式中的 `{a,b,c}` 花括号组（如 `*.{rs,ts}` → `*.rs`、`*.ts`）。
///
/// rg 原生支持 brace，但手工遍历回退（rg 缺失）的 [`glob_match`] 只做字面
/// 匹配——不展开则 `**/*.{rs,ts}` 这类模型常用模式永远零命中。递归处理多组。
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(start) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(rel_end) = pattern[start..].find('}') else {
        return vec![pattern.to_string()];
    };
    let end = start + rel_end;
    let pre = &pattern[..start];
    let rest = &pattern[end + 1..];
    let mut out = Vec::new();
    for opt in pattern[start + 1..end].split(',') {
        let combined = format!("{pre}{opt}{rest}");
        out.extend(expand_braces(&combined));
    }
    out
}

/// 手工递归遍历回退：跳过隐藏条目与 [`EXCLUDED_DIRS`]，不支持 .gitignore
/// （文档化降级行为，与 rg 语义的差异点）。brace 模式先展开（见
/// [`expand_braces`]），任一展开模式命中即计入。
async fn collect_walk_files(base: &Path, pattern: &str) -> Result<Vec<PathBuf>, TianyanError> {
    let mut out = Vec::new();
    let patterns = expand_braces(pattern);
    // 迭代式 DFS（显式栈，避免 async 递归）
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = tokio::fs::read_dir(&dir).await.map_err(|e| {
            TianyanError::Custom(format!("executor: 读取目录失败 {}：{e}", dir.display()))
        })?;
        while let Some(entry) = rd.next_entry().await.map_err(|e| {
            TianyanError::Custom(format!("executor: 读取目录失败 {}：{e}", dir.display()))
        })? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue; // 隐藏条目（与 rg --files 默认行为一致）
            }
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(t) => t,
                Err(_) => continue, // 单个条目读取失败跳过，不中断整体遍历
            };
            if file_type.is_dir() {
                if EXCLUDED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                if let Ok(rel) = path.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    if patterns.iter().any(|p| glob_match(p, &rel_str)) {
                        out.push(rel.to_path_buf());
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 递归 glob 匹配（回退遍历使用，模式与路径段均以 `/` 分隔）：
///
/// - `*` 匹配段内任意字符序列（不跨 `/`），`**` 匹配零个或多个目录段
/// - `?` 匹配单个非 `/` 字符
/// - `[...]` 字符类（支持 `!`/`^` 取反与 `a-z` 范围）
/// - `\` 转义下一个字符
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&[u8]> = pattern.split('/').map(str::as_bytes).collect();
    let txt: Vec<&[u8]> = path.split('/').map(str::as_bytes).collect();
    match_segments(&pat, &txt)
}

fn match_segments(pat: &[&[u8]], txt: &[&[u8]]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    if pat[0] == b"**" {
        // `**`：匹配零个（直接跳过该段）或多个（消费一个文本段后继续）目录段
        return match_segments(&pat[1..], txt)
            || (!txt.is_empty() && match_segments(pat, &txt[1..]));
    }
    !txt.is_empty() && match_segment(pat[0], txt[0]) && match_segments(&pat[1..], &txt[1..])
}

fn match_segment(pat: &[u8], txt: &[u8]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    match pat[0] {
        b'*' => (0..=txt.len()).any(|i| match_segment(&pat[1..], &txt[i..])),
        b'?' => !txt.is_empty() && match_segment(&pat[1..], &txt[1..]),
        b'[' => {
            if !txt.is_empty() {
                if let Some(consumed) = match_char_class(pat, txt[0]) {
                    return match_segment(&pat[consumed..], &txt[1..]);
                }
            }
            // 无闭合 `]`：按字面 `[` 处理
            !txt.is_empty() && txt[0] == b'[' && match_segment(&pat[1..], &txt[1..])
        }
        c => !txt.is_empty() && txt[0] == c && match_segment(&pat[1..], &txt[1..]),
    }
}

/// 匹配 `[...]` 字符类；成功返回消耗的模式长度（含 `]`），否则 None。
fn match_char_class(pat: &[u8], c: u8) -> Option<usize> {
    debug_assert_eq!(pat[0], b'[');
    let mut i = 1;
    let negate = matches!(pat.get(i), Some(b'!') | Some(b'^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let b = *pat.get(i)?; // 无闭合 `]` → None（按字面处理）
        if b == b']' && !first {
            break;
        }
        first = false;
        if b == b'\\' {
            if *pat.get(i + 1)? == c {
                matched = true;
            }
            i += 2;
            continue;
        }
        // `a-z` 范围（`-` 后紧跟 `]` 时按字面 `-` 处理）
        if let (Some(&b'-'), Some(&hi)) = (pat.get(i + 1), pat.get(i + 2)) {
            if hi != b']' {
                if (b..=hi).contains(&c) {
                    matched = true;
                }
                i += 3;
                continue;
            }
        }
        if b == c {
            matched = true;
        }
        i += 1;
    }
    let consumed = i + 1; // 含 `]`
    if negate {
        matched = !matched;
    }
    matched.then_some(consumed)
}

#[cfg(test)]
#[path = "fs_tests.rs"]
mod tests;
