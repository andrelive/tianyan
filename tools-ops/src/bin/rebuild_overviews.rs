//! 存量 L1 一次性重建：旧概览 → 章节目录（ADR-001 修订 2026-09-22 配套迁移）。
//!
//! 背景：VFS 三层语义重定义为"简介 · 目录 · 正文"后，存量条目的 L1 仍是旧语义
//! （同质概览）。本工具把**超容量内容**的 L1 重建为章节目录（经 `SummaryEngine`
//! 单点生成，不另起生成路径）；短内容"直用"契约不受影响（L1 = 全文，无需重建）。
//!
//! 特性：
//! - **幂等**：L1 已是目录（`parse_doc_index` 成功）的条目跳过（不重复调 LLM）；
//! - 只处理超容量内容（`estimate_tokens > OVERVIEW_TOKEN_LIMIT`）；
//! - 跳过系统/运维路径（归档/评审/运维子域——`system_paths` 判定单点，ADR-034）。
//! - 边界：LLM 判定"无法分节"而**降级为概览**（纯文本）的条目无法用内容特征
//!   识别，下次运行会重试（首次实跑未触发——16/16 全部目录化）。
//!
//! 实跑记录（2026-09-22，本机数据目录）：169 个叶子条目 → 超容量 **16 条全部
//! 目录化**（含 43KB 规则、角色会话、5 个技能、知识文档；定位行号全部命中），
//! 153 条短内容"直用"跳过；复跑幂等（重建 0 / 已是目录 16）。
//!
//! 用法（**建议先退出天演应用**，避免与运行中实例争用 LanceDB 写入）：
//! ```text
//! cargo run -p tianyan-tools-ops --bin rebuild_overviews -- --dry-run   # 预览
//! cargo run -p tianyan-tools-ops --bin rebuild_overviews                # 执行
//! ```

use tianyan::common::token_estimator::estimate_tokens;
use tianyan::common::types::{system_paths, ContentLevel, ContextNamespace, TianyanUri};
// ADR-049 期 2 序 6：ModelServices 容器已搬到 server 装配层；运维脚本直接用 provider
// 插件的能力集合（tianyan-provider-embed / tianyan-provider-llm）——它们只依赖 core。
// 最小装配（配置 → SQLite 后端 → LanceDB → 嵌入 provider → VFS）已抽到本包 `lib.rs`；
// 只有本脚本需要 LLM 摘要，故 `llm_services` + `SummaryEngine` 留在这里。
use tianyan::vfs::{
    parse_doc_index, ContentStore, SummaryEngine, SummaryService, VfsCore, VfsSearch,
    VirtualFileSystem, OVERVIEW_TOKEN_LIMIT,
};
use tianyan_provider_llm::services as llm_services;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("rebuild_overviews 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), tianyan::TianyanError> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");

    // ── 装配（与 server initialize_vfs_for_app 同构；实现见本包 `lib.rs`）────
    let config = tianyan_tools_ops::load_config(None)?;
    tianyan_tools_ops::print_targets(&config);
    let vfs = tianyan_tools_ops::open_vfs(&config).await?;

    let llm = llm_services(&config.models)?;
    let engine = SummaryEngine::new(llm.chat.clone(), llm.chat_model.clone());

    println!(
        "模式：{}",
        if dry_run {
            "dry-run（只读预览）"
        } else {
            "执行"
        }
    );

    // ── 收集全部叶子条目（跳过系统/运维路径）───────────────────────────────
    let mut leaves = Vec::new();
    for &ns in ContextNamespace::ALL {
        if ns == ContextNamespace::Session {
            continue; // 会话内容不入 VFS（ADR-018）
        }
        collect_leaves(&vfs, &TianyanUri::new(ns, vec![]), &mut leaves, 0).await;
    }

    // ── 逐个重建：仅超容量内容；已是目录的跳过（幂等）──────────────────────
    let mut scanned = 0usize;
    let mut rebuilt = 0usize;
    let mut skipped_short = 0usize;
    let mut skipped_done = 0usize;
    let mut failed = 0usize;

    for uri in &leaves {
        let detail = match vfs.read_content(uri, ContentLevel::Detail).await {
            Ok(c) if !c.trim().is_empty() => c,
            _ => continue,
        };
        scanned += 1;
        if estimate_tokens(&detail) <= OVERVIEW_TOKEN_LIMIT {
            skipped_short += 1; // 短内容：L1 = 全文（直用），无需目录
            continue;
        }
        if let Ok(current) = vfs.read_content(uri, ContentLevel::Overview).await {
            if parse_doc_index(&current).is_some() {
                skipped_done += 1; // 已是目录（幂等）
                continue;
            }
        }
        if dry_run {
            println!("    [将重建] {uri}");
            rebuilt += 1;
            continue;
        }
        match engine.generate_overview(&detail).await {
            Ok(overview) => {
                let abstract_text = vfs.read_abstract(uri).await.unwrap_or_default();
                vfs.write_overview(uri, &overview).await?;
                vfs.update_summary_vectors(uri, &abstract_text, &overview)
                    .await?;
                rebuilt += 1;
                if rebuilt.is_multiple_of(10) {
                    println!("    …已重建 {rebuilt}");
                }
            }
            Err(e) => {
                failed += 1;
                eprintln!("    [跳过] {uri} 生成失败：{e}");
            }
        }
    }

    println!(
        "\n扫描叶子条目 {scanned}：重建 {rebuilt} / 短内容跳过 {skipped_short} / 已是目录 {skipped_done} / 失败 {failed}"
    );
    Ok(())
}

/// 递归收集叶子（跳过系统/运维路径与归档；深度上限防失控）。
async fn collect_leaves(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dir: &TianyanUri,
    out: &mut Vec<TianyanUri>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = vfs.list(dir).await else {
        return;
    };
    for entry in entries {
        if system_paths::is_system_path(entry.uri()) {
            continue;
        }
        if entry.is_directory() {
            Box::pin(collect_leaves(vfs, entry.uri(), out, depth + 1)).await;
        } else {
            out.push(entry.uri().clone());
        }
    }
}
