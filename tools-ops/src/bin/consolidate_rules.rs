//! 规则库存量整理：失败碎片归并 + 归档路径统一（一次性运维脚本）。
//!
//! 背景（2026-11 实测 `D:\tianyan` 真实库）：
//! - `agent/learned` 活跃 116 条里 **86 条**是 `RuleRecorder` 早期「失败一次写一条」
//!   留下的碎片（`rule-{日期}-{时间}`），按 `(工具, 失败类别)` 其实只有约 20 组——
//!   `write_file` 10 条里 9 条同为「拒绝访问 os error 5」；
//! - 归档分散在两处：标准位置 `agent/learned/archive/`（435 条）与历史遗留
//!   `agent/_archive/learned/`（149 条），另有 8 条裸时间戳归档 `agent/_archive/{ts}`。
//!
//! 本工具只做**形态整理**，不改语义：
//! 1. 按 `(工具, 失败类别)` 把碎片归并成一条（累计次数 + 最近失败记录 + 首次详情），
//!    其余移入 `learned/archive/`；组内只有 1 条时不处理（因此可安全重复运行）；
//! 2. 把 `_archive/learned/*` 与 `_archive/{纯时间戳}` 迁到标准位置 `learned/archive/`。
//!
//! **不重命名** `rule-evo-{ts}` 与 `evo-{ts}` 记忆——那要 LLM 按内容起语义名，
//! 是独立的一步（见后续计划）。
//!
//! 失败分类复用 `FailureClass::of(&TianyanError::Custom(错误行))`：谓词只看消息前缀，
//! 与生产写入路径是同一套契约，运维脚本不另写 `contains` 规则。
//!
//! 用法：
//! ```text
//! cargo run -p tianyan-tools-ops --bin consolidate_rules -- --dry-run   # 只读预览
//! cargo run -p tianyan-tools-ops --bin consolidate_rules                 # 执行
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::types::{AgentPath, ContentLevel, ContextNamespace, TianyanUri};
use tianyan::observability::rule_recorder::FailureClass;
use tianyan::vfs::{ContentStore, VfsCore, VfsSearch, VirtualFileSystem, VirtualFileSystemImpl};

/// 单条规则最多保留的「最近失败」记录行数（与 `RuleRecorder` 同口径）。
const MAX_RECENT: usize = 20;

/// 一个待归并的碎片。
struct Member {
    name: String,
    uri: TianyanUri,
    detail: String,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("consolidate_rules 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let data_dir_override: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let config = tianyan_tools_ops::load_config(data_dir_override)?;
    tianyan_tools_ops::print_targets(&config);
    if dry_run {
        println!("\n>>> dry-run：只读预览，不写入任何内容\n");
    }
    let vfs = tianyan_tools_ops::open_vfs(&config).await?;

    let (groups, merged, archived) = merge_fragments(&vfs, dry_run).await?;
    let moved = unify_archive_paths(&vfs, dry_run).await?;

    println!("\n=== 汇总 ===");
    println!("  碎片分组：{groups} 组（其中 {merged} 组发生归并，归档 {archived} 条）");
    println!("  归档迁移：{moved} 条");
    if dry_run {
        println!("\n（dry-run：以上均为预览，未写入）");
    }
    Ok(())
}

/// 阶段 1：把 `rule-{日期}-{时间}` 碎片按 `(工具, 失败类别)` 归并。
async fn merge_fragments(
    vfs: &VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(usize, usize, usize)> {
    println!("=== 1. 失败碎片归并（rule-{{日期}}-{{时间}}）===");
    let learned = AgentPath::Learned.uri();
    let mut groups: BTreeMap<(String, String), (FailureClass, Vec<Member>)> = BTreeMap::new();
    let mut skipped = 0usize;

    for entry in vfs.list(&learned).await? {
        if entry.is_directory() {
            continue;
        }
        let name = entry.uri().path().last().cloned().unwrap_or_default();
        if !is_legacy_fragment(&name) {
            continue;
        }
        let detail = vfs
            .read_content(entry.uri(), ContentLevel::Detail)
            .await
            .unwrap_or_default();
        match classify(&detail) {
            Some((tool, class)) => {
                groups
                    .entry((tool, class.slug().to_string()))
                    .or_insert_with(|| (class, Vec::new()))
                    .1
                    .push(Member {
                        name,
                        uri: entry.uri().clone(),
                        detail,
                    });
            }
            None => {
                skipped += 1;
                println!("  跳过（detail 里既无「工具:」也无「错误:」行）：{name}");
            }
        }
    }

    let total: usize = groups.values().map(|(_, v)| v.len()).sum();
    let group_count = groups.len();
    println!("  碎片 {total} 条 → {group_count} 组（跳过 {skipped} 条）");

    let archive_dir = AgentPath::learned_archive();
    if !dry_run && !vfs.exists(&archive_dir).await? {
        vfs.create_directory(&archive_dir).await?;
    }

    let mut merged_groups = 0usize;
    let mut archived = 0usize;

    for ((tool, slug), (class, mut members)) in groups {
        if members.len() < 2 {
            continue;
        }
        // 名字里的时间戳字典序即时间序：最早的一条留作主条目（它承载「首次失败详情」）
        members.sort_by(|a, b| a.name.cmp(&b.name));
        merged_groups += 1;
        let occurrences = members.len();
        let keep = &members[0];
        println!(
            "  [{tool}/{slug}] {occurrences} 条 → 主条目 {}（{label}）",
            keep.name,
            label = class.label()
        );

        // 最近失败：新的在前，最多 MAX_RECENT 行
        let mut lines: Vec<String> = members
            .iter()
            .rev()
            .map(|m| record_line(&m.name, &m.detail))
            .collect();
        lines.truncate(MAX_RECENT);

        let now = chrono::Utc::now().to_rfc3339();
        let merged_detail = format!(
            "工具: {tool}\n主题: {tool} / {label}\n累计次数: {occurrences}\n最近发生: {now}\n\n\
             ## 最近失败\n{recent}\n\n\
             ## 首次失败详情\n{first}\n\n\
             ---\n## 规则元数据\n- 整理: consolidate_rules（2026-11 存量归并）\n- 最近合并: {now}\n",
            label = class.label(),
            recent = lines.join("\n"),
            first = keep.detail.trim_end(),
        );
        // 与生产写入侧同一口径（`rule_abstract`）：abstract 是「场景 → 动作」，
        // 计数在尾部——事件记录式写法检索命中率极低（2026-11-10 验证：83% 规则零命中）。
        let new_abstract =
            tianyan::observability::rule_recorder::rule_abstract(&tool, class, occurrences as u32);

        if dry_run {
            println!("    dry-run：将改写 {} 的 detail/abstract", keep.name);
            for m in &members[1..] {
                println!("    dry-run：将归档 {} → learned/archive/", m.name);
            }
            archived += members.len() - 1;
            continue;
        }

        vfs.write_content(&keep.uri, &merged_detail).await?;
        vfs.write_abstract(&keep.uri, &new_abstract).await?;
        // abstract 变了 → 重算向量（overview 沿用现有内容；取不到就用 abstract 兜底，
        // 宁可 L1 向量短暂失真也不能让主条目的 L0 向量停留在旧摘要上）
        let overview = vfs
            .read_content(&keep.uri, ContentLevel::Overview)
            .await
            .unwrap_or_else(|_| new_abstract.clone());
        if let Err(e) = vfs
            .update_summary_vectors(&keep.uri, &new_abstract, &overview)
            .await
        {
            println!("    ⚠️ 向量更新失败（{e}），该条目的 L0 向量仍是旧摘要");
        }

        // 其余条目归档（move_entry 会连向量点一起搬到新 URI）
        for m in &members[1..] {
            let dst = unique_destination(vfs, &archive_dir, &m.name).await?;
            vfs.move_entry(&m.uri, &dst).await?;
            archived += 1;
        }
    }

    Ok((group_count, merged_groups, archived))
}

/// 阶段 2：把历史归档路径统一到 `agent/learned/archive/`。
async fn unify_archive_paths(vfs: &VirtualFileSystemImpl, dry_run: bool) -> Result<usize> {
    println!("\n=== 2. 归档路径统一 → learned/archive/ ===");
    let target = AgentPath::learned_archive();
    let legacy_learned = TianyanUri::new(
        ContextNamespace::Agent,
        vec!["_archive".to_string(), "learned".to_string()],
    );
    let legacy_root = TianyanUri::new(ContextNamespace::Agent, vec!["_archive".to_string()]);

    let mut sources: Vec<TianyanUri> = Vec::new();
    // 2a. _archive/learned/* —— 历史规则归档位置
    match vfs.list(&legacy_learned).await {
        Ok(entries) => {
            for e in entries {
                if !e.is_directory() {
                    sources.push(e.uri().clone());
                }
            }
        }
        Err(e) => println!("  （{legacy_learned} 列举失败，跳过：{e}）"),
    }
    // 2b. _archive/{纯时间戳} —— 更早期的裸时间戳归档
    match vfs.list(&legacy_root).await {
        Ok(entries) => {
            for e in entries {
                let name = e.uri().path().last().cloned().unwrap_or_default();
                if !e.is_directory() && name.len() == 10 && name.bytes().all(|b| b.is_ascii_digit())
                {
                    sources.push(e.uri().clone());
                }
            }
        }
        Err(e) => println!("  （{legacy_root} 列举失败，跳过：{e}）"),
    }

    println!("  待迁移 {} 条", sources.len());
    if sources.is_empty() {
        return Ok(0);
    }

    if !dry_run && !vfs.exists(&target).await? {
        vfs.create_directory(&target).await?;
    }

    let mut moved = 0usize;
    for src in &sources {
        let name = src.path().last().cloned().unwrap_or_default();
        if dry_run {
            println!("    dry-run：{src} → {target}/{name}");
            moved += 1;
            continue;
        }
        let dst = unique_destination(vfs, &target, &name).await?;
        match vfs.move_entry(src, &dst).await {
            Ok(()) => moved += 1,
            Err(e) => println!("    ⚠️ 迁移失败 {src}：{e}"),
        }
    }
    Ok(moved)
}

/// 目标已存在时加 `-dupN` 后缀（`move_entry` 会覆盖同 URI，不能撞）。
async fn unique_destination(
    vfs: &VirtualFileSystemImpl,
    dir: &TianyanUri,
    name: &str,
) -> Result<TianyanUri> {
    let mut candidate = dir.append(name);
    if !vfs.exists(&candidate).await? {
        return Ok(candidate);
    }
    for n in 2..=99u32 {
        candidate = dir.append(&format!("{name}-dup{n}"));
        if !vfs.exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    Err(TianyanError::conflict(format!(
        "consolidate_rules: 目标目录已满（{name} 的 dup 后缀用尽）"
    )))
}

/// `rule-{YYYYMMDD}-{HHMMSS}` 形态（`RuleRecorder` 早期的碎片命名）。
fn is_legacy_fragment(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("rule-") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..].iter().all(u8::is_ascii_digit)
}

/// 从 detail 提取 `(工具, 失败类别)`。分类走生产同一套语义谓词。
fn classify(detail: &str) -> Option<(String, FailureClass)> {
    let tool = detail
        .lines()
        .find_map(|l| l.strip_prefix("工具: "))?
        .trim()
        .to_string();
    let err = detail
        .lines()
        .find_map(|l| l.strip_prefix("错误: "))
        .unwrap_or("")
        .trim();
    // 谓词只看消息前缀：把历史错误串包成 `Custom` 即可复用同一套分类契约
    let class = FailureClass::of(&TianyanError::Custom(err.to_string()));
    Some((tool, class))
}

/// 生成一条「最近失败」记录行（时间从文件名解析，参数与错误各截断）。
fn record_line(name: &str, detail: &str) -> String {
    let ts = name_to_rfc3339(name);
    let session = detail
        .lines()
        .find_map(|l| l.strip_prefix("来源会话: "))
        .unwrap_or("-")
        .trim();
    let param = detail
        .lines()
        .find_map(|l| l.strip_prefix("参数: "))
        .unwrap_or("")
        .trim();
    let err = detail
        .lines()
        .find_map(|l| l.strip_prefix("错误: "))
        .unwrap_or("")
        .trim();
    format!(
        "- {ts} [{session}] 参数={} 错误={}",
        tianyan::common::truncate::truncate_utf8_boundary(param, 240),
        tianyan::common::truncate::truncate_utf8_boundary(err, 360)
    )
}

/// `rule-20261105-020742` → `2026-11-05T02:07:42`（无法解析时原样返回）。
fn name_to_rfc3339(name: &str) -> String {
    let rest = name.strip_prefix("rule-").unwrap_or(name);
    if rest.len() == 15 && rest.is_char_boundary(8) && rest.as_bytes()[8] == b'-' {
        format!(
            "{}-{}-{}T{}:{}:{}",
            &rest[0..4],
            &rest[4..6],
            &rest[6..8],
            &rest[9..11],
            &rest[11..13],
            &rest[13..15]
        )
    } else {
        name.to_string()
    }
}
