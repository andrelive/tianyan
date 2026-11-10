//! 规则摘要回填：把历史「事件记录式」摘要改写成「场景 → 动作」形态（一次性运维脚本）。
//!
//! 背景（2026-11-10 只读验证 `D:\tianyan` 真实库）：活跃 65 条规则里 **54 条（83%）
//! 从未被检索命中**。其中 19 条是 `RuleRecorder` 早期的事件记录式摘要——
//! `[规则] apply_patch 定位冲突（累计 5 次，最近 2026-11-05T…）`。这种写法与用户提问的
//! 语义相似度天然低，过不了注入管线的相关性门槛（`MIN_SEMANTIC_SCORE = 0.35`），
//! 于是「规则是否降低失败率」根本无法验证。
//!
//! 写入侧已改为 [`rule_abstract`]（场景 → 动作）；本脚本把**存量**对齐到同一口径：
//!
//! 1. `[规则] tool class（累计 N 次）` → `tool class → 动作（累计 N 次）`
//!    （工具 / 类别 / 次数取自 `detail` 的结构化字段，不从自由文本猜）；
//! 2. `[演化] …` 前缀 → 去掉（该前缀不携带任何检索信号）；
//! 3. 其余摘要（演化语义规则，模型写的完整句子）**不动**——它们不是事件记录式写法。
//!
//! 改完摘要必须**重算 L0 向量**，否则检索仍按旧向量命中（`update_summary_vectors`）。
//! 幂等：已是「场景 → 动作」形态的条目直接跳过，可重复运行。
//!
//! 用法：
//! ```text
//! cargo run -p tianyan-tools-ops --bin rewrite_rule_abstracts -- --dry-run   # 只读预览
//! cargo run -p tianyan-tools-ops --bin rewrite_rule_abstracts                 # 执行
//! ```
//!
//! **必须先停止天演服务**：本脚本按运维工具惯例直接打开生产库（`Database::open` +
//! `init_schemas`）并写内容，与运行中的服务抢 SQLite 写锁会互相损坏。

use std::path::PathBuf;

use tianyan::common::error::Result;
use tianyan::common::types::{AgentPath, ContentLevel};
use tianyan::observability::rule_recorder::{rule_abstract, FailureClass};
use tianyan::vfs::{ContentStore, VfsCore, VfsSearch, VirtualFileSystem, VirtualFileSystemImpl};

/// 打印预览时每条旧摘要保留的可见字符数（够看清形态即可）。
const PREVIEW_CHARS: usize = 60;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("rewrite_rule_abstracts 失败：{e}");
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

    let stats = rewrite_all(&vfs, dry_run).await?;

    println!("\n=== 汇总 ===");
    println!("  扫描条目：{}", stats.scanned);
    println!("  改写：{}", stats.rewritten);
    println!("  已是新形态（跳过）：{}", stats.already_new);
    println!("  非事件记录式（演化语义规则，未动）：{}", stats.untouched);
    println!("  无法解析（跳过，不猜）：{}", stats.unparsable);
    if dry_run {
        println!("\n（dry-run：以上均为预览，未写入）");
    } else {
        println!("\n提示：摘要已更新的条目已重算 L0 向量；若某条向量更新失败，日志里会有 ⚠️。");
    }
    Ok(())
}

/// 回填统计。
#[derive(Default)]
struct Stats {
    scanned: usize,
    rewritten: usize,
    already_new: usize,
    untouched: usize,
    unparsable: usize,
}

/// 遍历 `agent/learned` 的活跃条目并回填摘要。
async fn rewrite_all(vfs: &VirtualFileSystemImpl, dry_run: bool) -> Result<Stats> {
    let learned = AgentPath::Learned.uri();
    let entries = vfs.list(&learned).await?;
    let mut stats = Stats::default();

    println!("=== 回填 agent/learned（活跃规则）===");
    for entry in entries {
        if entry.is_directory() {
            continue;
        }
        let uri = entry.uri().clone();
        stats.scanned += 1;

        let abstract_text = vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap_or_default();
        let trimmed = abstract_text.trim();
        if trimmed.is_empty() {
            stats.untouched += 1;
            continue;
        }

        // 只有事件记录式 / 带 [演化] 前缀的才需要读 detail（省一次 IO）
        let needs_detail = trimmed.starts_with("[规则]") && !trimmed.contains('→');
        let detail = if needs_detail {
            vfs.read_content(&uri, ContentLevel::Detail)
                .await
                .unwrap_or_default()
        } else {
            String::new()
        };

        match plan_rewrite(trimmed, &detail) {
            Kind::AlreadyNew => stats.already_new += 1,
            Kind::NotEventRecord => stats.untouched += 1,
            Kind::Unparsable => {
                stats.unparsable += 1;
                println!("  ⚠️ 无法解析（跳过，不猜）：{uri}");
            }
            Kind::Rewrite(new_abstract) => {
                let preview: String = trimmed.chars().take(PREVIEW_CHARS).collect();
                println!("  {uri}\n    旧: {preview}…\n    新: {new_abstract}");
                if !dry_run {
                    vfs.write_abstract(&uri, &new_abstract).await?;
                    // 摘要变了必须重算 L0 向量：否则检索仍按旧向量命中，改摘要等于白改。
                    // overview 沿用现有内容；取不到就用新摘要兜底（宁可 L1 向量短暂失真，
                    // 也不能让 L0 停留在旧摘要上——与 consolidate_rules 同口径）。
                    let overview = vfs
                        .read_content(&uri, ContentLevel::Overview)
                        .await
                        .unwrap_or_else(|_| new_abstract.clone());
                    if let Err(e) = vfs
                        .update_summary_vectors(&uri, &new_abstract, &overview)
                        .await
                    {
                        println!("    ⚠️ 向量更新失败（{e}），该条目 L0 向量仍是旧摘要");
                    }
                }
                stats.rewritten += 1;
            }
        }
    }

    Ok(stats)
}

/// 单条摘要的处置判定。
enum Kind {
    /// 已是「场景 → 动作」形态，跳过（幂等）。
    AlreadyNew,
    /// 不是事件记录式写法（演化语义规则），不动。
    NotEventRecord,
    /// 是事件记录式写法，但缺结构化字段 → 跳过并打印（不猜）。
    Unparsable,
    /// 需要改写成这条。
    Rewrite(String),
}

/// 判定并（必要时）生成新摘要。`detail` 只在事件记录式分支被用到。
fn plan_rewrite(abstract_text: &str, detail: &str) -> Kind {
    // `[演化] {x} (来源: evolution)`：只去前缀，内容保留（不携带检索信号的只有前缀）
    if let Some(rest) = abstract_text.strip_prefix("[演化]") {
        let cleaned = rest
            .trim()
            .replace(" (来源: evolution)", "（来源: evolution）");
        if cleaned.is_empty() {
            return Kind::Unparsable;
        }
        return Kind::Rewrite(cleaned);
    }

    if !abstract_text.starts_with("[规则]") {
        return if abstract_text.contains('→') {
            Kind::AlreadyNew
        } else {
            Kind::NotEventRecord
        };
    }

    // 事件记录式：工具 / 类别 / 次数从 detail 的结构化字段取
    let Some((tool, label, occurrences)) = parse_detail(detail) else {
        return Kind::Unparsable;
    };
    let Some(class) = FailureClass::from_label(&label) else {
        return Kind::Unparsable;
    };
    Kind::Rewrite(rule_abstract(&tool, class, occurrences))
}

/// 从 `detail` 里取 `(工具, 类别 label, 累计次数)`。
///
/// 解析的是 `RuleRecorder` 写下的**结构化事实**（`主题: tool / label`、`累计次数: N`），
/// 不从自由文本猜：解析失败宁可跳过，也不生成一条错的摘要。
fn parse_detail(detail: &str) -> Option<(String, String, u32)> {
    let mut tool = None;
    let mut label = None;
    let mut occurrences = 1u32;
    for line in detail.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("主题: ") {
            if let Some((t, c)) = rest.split_once(" / ") {
                tool = Some(t.trim().to_string());
                label = Some(c.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("累计次数: ") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                occurrences = n;
            }
        }
    }
    Some((tool?, label?, occurrences))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const DETAIL: &str = "工具: apply_patch\n主题: apply_patch / 定位冲突\n累计次数: 5\n\
                          最近发生: 2026-11-05T13:05:01+00:00\n\n## 最近失败\n- …\n";

    fn rewritten(kind: Kind) -> String {
        match kind {
            Kind::Rewrite(text) => text,
            Kind::AlreadyNew => panic!("应改写，实际判为 already_new"),
            Kind::NotEventRecord => panic!("应改写，实际判为 not_event_record"),
            Kind::Unparsable => panic!("应改写，实际判为 unparsable"),
        }
    }

    /// 事件记录式摘要 → 「场景 → 动作」，工具/类别/次数取自 detail。
    ///
    /// 判别力：把 `plan_rewrite` 的事件记录分支改回「原样返回」→ 本用例红。
    #[test]
    fn test_event_record_is_rewritten_to_scenario_form() {
        let text = rewritten(plan_rewrite(
            "[规则] apply_patch 定位冲突（累计 5 次，最近 2026-11-05T13:05:01+00:00）",
            DETAIL,
        ));
        assert!(
            text.starts_with("apply_patch 定位冲突 → "),
            "形态不对：{text}"
        );
        assert!(text.contains("累计 5 次"), "次数应取自 detail：{text}");
        assert!(
            !text.starts_with("[规则]"),
            "不得保留事件记录式前缀：{text}"
        );
    }

    /// detail 缺结构化字段 / 未知类别 → 跳过（不猜）。
    #[test]
    fn test_missing_detail_is_unparsable() {
        let kind = plan_rewrite("[规则] x 执行失败（累计 1 次）", "只有自由文本，没有主题行");
        assert!(matches!(kind, Kind::Unparsable));
        let kind = plan_rewrite("[规则] x 执行失败（累计 1 次）", "主题: x / 不存在的类别\n");
        assert!(matches!(kind, Kind::Unparsable), "未知类别也应跳过");
    }

    /// 已是新形态 / 演化语义规则：都不动（幂等）。
    #[test]
    fn test_idempotent_and_semantic_rules_untouched() {
        assert!(matches!(
            plan_rewrite("apply_patch 定位冲突 → 连续两次即停（累计 5 次）", ""),
            Kind::AlreadyNew
        ));
        assert!(matches!(
            plan_rewrite("多步改进时先给推荐项与取舍，等拍板再执行。", ""),
            Kind::NotEventRecord
        ));
    }

    /// `[演化]` 前缀只去前缀，内容保留。
    #[test]
    fn test_evolution_prefix_is_stripped() {
        let text = rewritten(plan_rewrite("[演化] 先复现再改代码 (来源: evolution)", ""));
        assert_eq!(text, "先复现再改代码（来源: evolution）");
    }

    /// `parse_detail` 取结构化字段（不从自由文本猜）。
    #[test]
    fn test_parse_detail_reads_structured_fields() {
        let (tool, label, occ) = parse_detail(DETAIL).expect("应能解析");
        assert_eq!(tool, "apply_patch");
        assert_eq!(label, "定位冲突");
        assert_eq!(occ, 5);
        assert!(parse_detail("无主题行").is_none());
    }
}
