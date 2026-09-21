//! 内嵌代码搜索引擎（纯 Rust，替代外部 ripgrep 二进制）。
//!
//! 与 rg 的语义对齐点：
//! - 文件遍历用 ignore::WalkBuilder（rg 同作者同款：.gitignore、隐藏文件、
//!   符号链接策略一致），.git 目录始终排除；
//! - 匹配用 regex::bytes::Regex，支持 (?s) 多行模式与 submatch 偏移；
//! - 输出契约与 executor::search 完全一致（三种模式 + 守卫 + 分页），
//!   LLM 侧无感知。
//!
//! 优势：无外部二进制依赖（Windows/Linux 开箱即用），错误消息可本地化
//! （正则无效等直接给出中文原因，不再透传 rg 的英文 stderr）。

use std::path::Path;

use regex::bytes::RegexBuilder;
use serde_json::{json, Value};

use crate::common::error::TianyanError;

use crate::executor::truncate::truncate_line;

use super::search::{OutputMode, SearchOptions};

/// 单条记录（含上下文行）最大字节数：超出则丢弃整条记录。
const MAX_RECORD_BYTES: usize = 64 * 1024;
/// 单条记录 submatch 上限。
const MAX_SUBMATCHES: usize = 100;
/// 收集的记录总数硬上限（内存与输出体积保护）。
const MAX_TOTAL_MATCHES: usize = 5000;

/// 执行内嵌代码搜索，返回与 search.rs 相同的输出 JSON。
pub fn execute_embedded_search(
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, TianyanError> {
    let mode = options.output_mode.unwrap_or_default();
    let head_limit = options
        .head_limit
        .unwrap_or(super::search::DEFAULT_HEAD_LIMIT)
        .max(1);
    let offset = options.offset;

    // 编译正则：语法错误 → 本地化错误（rg 的 regex parse error 语义）
    let mut builder = RegexBuilder::new(pattern);
    builder
        .case_insensitive(options.ignore_case)
        .multi_line(true) // 匹配行边界；跨行用 (?s)
        .dot_matches_new_line(options.multiline);
    let re = builder
        .build()
        .map_err(|e| TianyanError::Custom(format!("executor: 搜索失败：正则无效：{e}")))?;

    let root = options
        .path
        .as_deref()
        .map(Path::new)
        .unwrap_or_else(|| Path::new("."));

    // 文件遍历：rg 同款规则（.gitignore/隐藏文件；显式排除 .git）
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            entry.file_name().to_str() != Some(".git")
                && entry.file_name().to_str() != Some(".tianyan")
        });
    if let Some(g) = &options.glob {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        if ob.add(g).is_ok() {
            if let Ok(overrides) = ob.build() {
                walk.overrides(overrides);
            }
        }
    }
    if let Some(t) = &options.type_ {
        if let Some(exts) = type_extensions(t) {
            // 类型过滤 = 白名单扩展名：用 OverrideBuilder 的否定 glob
            // （!*.txt 等）排除非目标类型文件（rg --type 语义）。
            let mut ob = ignore::overrides::OverrideBuilder::new(root);
            for ext in &exts {
                let _ = ob.add(&format!("*.{ext}"));
            }
            if let Ok(overrides) = ob.build() {
                walk.overrides(overrides);
            }
        }
    }

    let mut records: Vec<Value> = Vec::new();

    for entry in walk.build() {
        if records.len() >= MAX_TOTAL_MATCHES {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let content = match std::fs::read(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // 多行模式：以整个文件为单个匹配单元
        if options.multiline {
            scan_multiline(&re, entry.path(), &content, mode, &mut records);
            continue;
        }

        let lines = split_lines(&content);
        let mut matches_in_file: Vec<(usize, &[u8])> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                matches_in_file.push((i + 1, line));
            }
        }
        if matches_in_file.is_empty() {
            continue;
        }

        match mode {
            OutputMode::FilesWithMatches => {
                records.push(json!({
                    "path": entry.path().to_string_lossy(),
                    "line_number": 1,
                    "text": "",
                    "matches": [],
                }));
            }
            OutputMode::Count => {
                records.push(json!({
                    "path": entry.path().to_string_lossy(),
                    "line_number": 1,
                    "text": matches_in_file.len().to_string(),
                    "matches": [],
                }));
            }
            OutputMode::Content => {
                scan_content(
                    &re,
                    entry.path(),
                    &lines,
                    &matches_in_file,
                    options,
                    &mut records,
                );
            }
        }
    }

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

/// 按字节切分行（去掉行尾 \r\n 的 \r 保留 \n 语义）。
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            let mut end = i;
            if end > start && content[end - 1] == b'\r' {
                end -= 1;
            }
            lines.push(&content[start..end]);
            start = i + 1;
        }
    }
    if start < content.len() {
        let mut end = content.len();
        if end > start && content[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(&content[start..end]);
    }
    lines
}

/// content 模式：逐匹配行生成记录（含上下文与 submatch 偏移）。
fn scan_content(
    re: &regex::bytes::Regex,
    path: &Path,
    lines: &[&[u8]],
    matches_in_file: &[(usize, &[u8])],
    options: &SearchOptions,
    records: &mut Vec<Value>,
) {
    let ctx = options.context.unwrap_or(0);

    for &(line_no, line) in matches_in_file {
        if records.len() >= MAX_TOTAL_MATCHES {
            break;
        }
        // 上下文行（before：匹配行之前；after：匹配行之后）
        let mut before: Vec<String> = Vec::new();
        let b_start = line_no.saturating_sub(ctx + 1);
        for i in b_start..line_no.saturating_sub(1) {
            if let Some(l) = lines.get(i) {
                before.push(String::from_utf8_lossy(l).into_owned());
            }
        }
        let mut after: Vec<String> = Vec::new();
        for i in line_no..(line_no + ctx) {
            if let Some(l) = lines.get(i) {
                after.push(String::from_utf8_lossy(l).into_owned());
            }
        }
        // submatch 偏移（相对行首字节）
        let submatches: Vec<(usize, usize)> = re
            .find_iter(line)
            .take(MAX_SUBMATCHES)
            .map(|m| (m.start(), m.end()))
            .collect();

        // 组装（与 search.rs finalize_record 同语义）
        let (match_line, kept_bytes) = truncate_line(&String::from_utf8_lossy(line));
        let mut text = String::new();
        let mut prefix_bytes = 0usize;
        for l in &before {
            let (shown, _) = truncate_line(l);
            prefix_bytes += shown.len() + 1;
            text.push_str(&shown);
            text.push('\n');
        }
        text.push_str(&match_line);
        for l in &after {
            text.push('\n');
            text.push_str(&truncate_line(l).0);
        }
        if text.len() > MAX_RECORD_BYTES {
            continue;
        }
        let matches: Vec<Value> = submatches
            .iter()
            .filter(|&&(_, end)| end <= kept_bytes)
            .map(|&(s, e)| json!({ "start": s + prefix_bytes, "end": e + prefix_bytes }))
            .collect();
        let record = json!({
            "path": path.to_string_lossy(),
            "line_number": line_no,
            "text": text,
            "matches": matches,
        });
        records.push(record);
    }
}

/// 多行模式：整个文件为一个匹配单元，命中即产生一条记录。
fn scan_multiline(
    re: &regex::bytes::Regex,
    path: &Path,
    content: &[u8],
    mode: OutputMode,
    records: &mut Vec<Value>,
) {
    if !re.is_match(content) {
        return;
    }
    if records.len() >= MAX_TOTAL_MATCHES {
        return;
    }
    match mode {
        OutputMode::FilesWithMatches | OutputMode::Count => {
            records.push(json!({
                "path": path.to_string_lossy(),
                "line_number": 1,
                "text": "",
                "matches": [],
            }));
        }
        OutputMode::Content => {
            let text = String::from_utf8_lossy(content).into_owned();
            let (shown, _) = truncate_line(&text);
            let matches: Vec<Value> = re
                .find_iter(content)
                .take(MAX_SUBMATCHES)
                .map(|m| json!({ "start": m.start(), "end": m.end() }))
                .collect();
            if shown.len() > MAX_RECORD_BYTES {
                return;
            }
            let mut record = json!({
                "path": path.to_string_lossy(),
                "text": shown,
                "matches": matches,
            });
            record["line_number"] = json!(1);
            records.push(record);
        }
    }
}

/// rg --type 名称 → 扩展名列表（常用语言子集；未知类型返回 None 不过滤）。
fn type_extensions(type_name: &str) -> Option<Vec<&'static str>> {
    let map: &[(&str, &[&str])] = &[
        ("rust", &["rs"]),
        ("py", &["py", "pyi"]),
        ("js", &["js", "jsx", "mjs", "cjs"]),
        ("ts", &["ts", "tsx", "mts", "cts"]),
        ("go", &["go"]),
        ("java", &["java"]),
        ("c", &["c", "h"]),
        ("cpp", &["cpp", "cc", "cxx", "hpp", "hh", "hxx"]),
        ("csharp", &["cs"]),
        ("css", &["css"]),
        ("html", &["html", "htm"]),
        ("json", &["json"]),
        ("toml", &["toml"]),
        ("yaml", &["yaml", "yml"]),
        ("markdown", &["md", "markdown"]),
        ("sh", &["sh", "bash", "zsh"]),
        ("ps1", &["ps1", "psm1", "psd1"]),
        ("sql", &["sql"]),
        ("swift", &["swift"]),
        ("kotlin", &["kt", "kts"]),
        ("dart", &["dart"]),
        ("rb", &["rb"]),
        ("php", &["php"]),
        ("lua", &["lua"]),
        ("vue", &["vue"]),
        ("svelte", &["svelte"]),
        ("proto", &["proto"]),
        ("dockerfile", &["dockerfile", "Dockerfile"]),
        ("make", &["makefile", "Makefile", "mk"]),
        ("gitignore", &["gitignore"]),
    ];
    map.iter()
        .find(|(name, _)| *name == type_name)
        .map(|(_, exts)| exts.to_vec())
}
