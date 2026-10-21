//! 记忆畸形路径修复（一次性运维工具，2026-10-17 数据审计配套）。
//!
//! ## 背景：畸形条目 = 双写碎片 + 遍历不可达的「孤儿」
//!
//! 演化综述的 LLM 曾把「路径式引用」当作 id 回传（如 `memory/facts/evo-x`、
//! `tianyan://user/preferences/evo-x`），写入侧当时未做归一化，产生 24 条
//! 畸形条目（`memory/facts/memory/facts/evo-x` 等）。两层危害：
//!
//! 1. **双写碎片**：更新静默退化为「另写一份畸形副本」，原条目永不更新；
//! 2. **遍历不可达（孤儿）**：畸形条目的中间目录从未创建（如
//!    `memory/facts/memory/` 不存在），而 `list_directory` 的语义是「只返回
//!    一级子」——遍历链断裂，**任何基于遍历的通道（记忆面板 / 演化
//!    `find_entry` / 本工具遍历扫描）都到不了它们**；只有按 uri 精确查询
//!    （`read_entry`/`delete_entry` 的 SQL `WHERE uri = ?1`）可达。这意味着
//!    连「删除通道」都无法把它们清掉（`find_entry` 找不到 → 静默跳过）。
//!
//! 写入侧已以 `normalize_entry_id` 收口（`evolution_task`）；本工具修复存量。
//!
//! ## 流程（双源扫描）
//!
//! - **源 A（遍历）**：逐级遍历 memory/user/agent/skill 收集**可达**畸形条目
//!   （防未来新增；当前应为 0 条）；
//! - **源 B（清单）**：`ORPHAN_URIS` 硬编码的 24 条孤儿（精确可达性审计产物，
//!   见常量注释）——按 uri 精确寻址逐一修复；
//! - 计划展示后，`--apply`：先备份全部畸形条目完整内容到本地 JSON（审计），
//!   再执行（目标缺则迁移含三层内容与向量点、目标在则保留较新内容），随后
//!   删除畸形条目。单条失败不阻断其余条目。
//!
//! 用法（在仓库根，建议服务停止后执行——本工具与运行中的服务共用数据目录、
//! 且直接写 LanceDB）：
//! ```text
//! cargo run -p tianyan-storage-vector --example memory_path_repair # 检查
//! cargo run -p tianyan-storage-vector --example memory_path_repair -- --apply # 执行
//! ```
//!
//! 一次性工具：修复完成后可删除本文件（不参与发布链）。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tianyan::common::types::{ContentLevel, ContextNamespace, MemoryCategory, TianyanUri};
use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::db::Database;
use tianyan::model::ModelServices;
use tianyan::vfs::{StorageBackend, VfsCore, VirtualFileSystem, VirtualFileSystemBuilder};
use tianyan_storage_sqlite::SqliteBackend;
use tianyan_storage_vector::LanceDbVectorStore;

/// 已知孤儿条目清单（2026-10-17 精确可达性审计产物）。
///
/// 审计方法：全库 855 条按 `list_directory` 语义做 BFS（一级子 = 前缀匹配且
/// rest 不含 '/'），可达 788 叶子 + 43 目录，**不可达 24 条**（全部为文件）。
/// VFS 遍历到不了它们（中间目录缺失），按 uri 精确查询可达——故本清单为
/// 审计产物、随工具硬编码（一次性工具的既有先例：0.3.x `memory_cleanup.rs`
/// 的合并计划亦为硬编码）。
const ORPHAN_URIS: &[&str] = &[
    "tianyan://agent/patterns/agent/patterns/evo-1789193189",
    "tianyan://memory/cases/successful_tasks/memory/cases/successful_tasks/snn-coding-rules-negatives",
    "tianyan://memory/events/decisions/memory/events/decisions/evo-1791794544",
    "tianyan://memory/facts/memory/facts/1786860719",
    "tianyan://memory/facts/memory/facts/architecture-constraints",
    "tianyan://memory/facts/memory/facts/digital-brain-stage2-milestone",
    "tianyan://memory/facts/memory/facts/env-powershell-5.1-verified",
    "tianyan://memory/facts/memory/facts/event-push-architecture",
    "tianyan://memory/facts/memory/facts/evo-1789280062",
    "tianyan://memory/facts/memory/facts/evo-1789627026",
    "tianyan://memory/facts/memory/facts/evo-1789713649",
    "tianyan://memory/facts/memory/facts/evo-1790560000",
    "tianyan://memory/facts/memory/facts/evo-1791519112",
    "tianyan://memory/facts/memory/facts/evo-1791605596",
    "tianyan://memory/facts/memory/facts/evo-1791881035",
    "tianyan://memory/facts/memory/facts/instruction-following",
    "tianyan://memory/facts/memory/facts/project-environment",
    "tianyan://memory/facts/tianyan://memory/facts/evo-1789627026",
    "tianyan://user/entities/tianyan://user/entities/project-digital-brain",
    "tianyan://user/entities/user/entities/project-digital-brain",
    "tianyan://user/preferences/memory/events/decisions/evo-1790729837",
    "tianyan://user/preferences/user/preferences/communication",
    "tianyan://user/preferences/user/preferences/evo-1790426792",
    "tianyan://user/preferences/user/preferences/evo-1791259570",
];

#[tokio::main]
async fn main() {
    let apply = std::env::args().any(|a| a == "--apply");
    if let Err(e) = run(apply).await {
        eprintln!("memory_path_repair 失败：{e}");
        std::process::exit(1);
    }
}

/// 畸形模式识别（三种；返回人类可读标签）。对跨空间拼接等未覆盖形态返回
/// None（由调用方以「孤儿条目」兜底标注）。
fn malformed_reason(uri: &TianyanUri) -> Option<&'static str> {
    let ns_name = match uri.namespace() {
        ContextNamespace::Memory => "memory",
        ContextNamespace::User => "user",
        ContextNamespace::Agent => "agent",
        ContextNamespace::Skill => "skill",
        _ => return None,
    };
    let path = uri.path();
    let joined = path.join("/");
    if joined.contains("tianyan:") {
        return Some("嵌入完整 URI");
    }
    if path.first().map(String::as_str) == Some(ns_name) {
        return Some("重复前缀");
    }
    if joined.contains(&format!("/{ns_name}/")) {
        return Some("中段重复");
    }
    None
}

/// 类别 token → MemoryCategory。
fn category_from_token(token: &str) -> Option<MemoryCategory> {
    match token.trim().to_ascii_lowercase().as_str() {
        "preference" => Some(MemoryCategory::Preference),
        "decision" => Some(MemoryCategory::Decision),
        "fact" => Some(MemoryCategory::Fact),
        "entity" => Some(MemoryCategory::Entity),
        "pattern" => Some(MemoryCategory::Pattern),
        "successful_case" | "success" => Some(MemoryCategory::SuccessfulCase),
        "failed_case" | "failure" => Some(MemoryCategory::FailedCase),
        _ => None,
    }
}

/// 从条目文本解析类别：优先 detail 的 `- **类别**: xxx`，回退 abstract 的
/// `类别: xxx`（rfind 从后往前，避开正文同形字样）。
fn parse_category(abstract_text: &str, detail_text: &str) -> Option<MemoryCategory> {
    let extract = |text: &str, marker: &str| -> Option<MemoryCategory> {
        let pos = text.rfind(marker)?;
        let rest = &text[pos + marker.len()..];
        let token: String = rest
            .trim_start_matches(&[' ', '*', ':'][..])
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '|' && *c != '\n')
            .collect();
        category_from_token(&token)
    };
    extract(detail_text, "**类别**").or_else(|| extract(abstract_text, "类别"))
}

struct Item {
    source: TianyanUri,
    reason: &'static str,
    id: String,
    target: Option<TianyanUri>,
    updated: DateTime<Utc>,
    abstract_text: Option<String>,
    detail_text: Option<String>,
}

/// 构建单条修复计划（读内容 + 解析类别 + 计算目标路径）。
async fn build_item(
    vfs: &dyn VirtualFileSystem,
    uri: TianyanUri,
    reason: &'static str,
    updated: DateTime<Utc>,
) -> Item {
    let id = uri.path().last().cloned().unwrap_or_default();
    let abstract_text = vfs.read(&uri, ContentLevel::Abstract).await.ok();
    let detail_text = vfs.read(&uri, ContentLevel::Detail).await.ok();
    let category = parse_category(
        abstract_text.as_deref().unwrap_or(""),
        detail_text.as_deref().unwrap_or(""),
    );
    Item {
        source: uri,
        reason,
        id: id.clone(),
        target: category.map(|c| c.default_uri(&id)),
        updated,
        abstract_text,
        detail_text,
    }
}

/// 递归收集可达叶子条目（uri + updated_at）。
async fn collect_leaves(
    vfs: &dyn VirtualFileSystem,
    dir: &TianyanUri,
    out: &mut Vec<(TianyanUri, DateTime<Utc>)>,
) {
    let Ok(entries) = vfs.list(dir).await else {
        return;
    };
    for entry in entries {
        if entry.is_directory() {
            Box::pin(collect_leaves(vfs, &entry.metadata.uri, out)).await;
        } else {
            out.push((entry.metadata.uri.clone(), entry.metadata.updated_at));
        }
    }
}

/// 执行单条修复；返回动作描述。失败不阻断其余条目（由调用方捕获）。
async fn apply_item(
    vfs: &dyn VirtualFileSystem,
    it: &Item,
) -> Result<String, Box<dyn std::error::Error>> {
    let Some(target) = &it.target else {
        return Ok("跳过（类别无法解析）".to_string());
    };
    if target == &it.source {
        return Ok("跳过（目标即自身）".to_string());
    }
    let action = if vfs.exists(target).await? {
        let target_updated = vfs.get_entry(target).await?.metadata.updated_at;
        if it.updated > target_updated {
            if let Some(d) = &it.detail_text {
                vfs.write_content(target, d).await?;
            }
            if let Some(a) = &it.abstract_text {
                vfs.write_abstract(target, a).await?;
            }
            if let Ok(ov) = vfs.read(&it.source, ContentLevel::Overview).await {
                let _ = vfs.write_overview(target, &ov).await;
            }
            format!(
                "覆盖（源较新 {} > {}）",
                it.updated.format("%m-%d %H:%M"),
                target_updated.format("%m-%d %H:%M")
            )
        } else {
            format!(
                "去重（目标已较新 {} <= {}）",
                it.updated.format("%m-%d %H:%M"),
                target_updated.format("%m-%d %H:%M")
            )
        }
    } else {
        // 注意：move_entry 内部已删除源条目（实现内的 `delete_entry(source)`）——
        // 不得再执行外层 delete，否则对已删条目误报「条目未找到」
        // （2026-10-17 首次执行实证）。
        vfs.move_entry(&it.source, target).await?;
        return Ok("迁移".to_string());
    };
    vfs.delete(&it.source).await?;
    Ok(action)
}

async fn run(apply: bool) -> Result<(), Box<dyn std::error::Error>> {
    let config = TianyanConfig::load()?;
    let db_path = config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"));
    println!("数据目录：{}", config.storage.data_dir.display());
    println!("数据库：  {}", db_path.display());
    println!(
        "模式：    {}",
        if apply {
            "执行（--apply）"
        } else {
            "检查（只读）"
        }
    );

    // 装配 VFS（与 server initialize_vfs_for_app 同构；嵌入服务可用则装配，
    // 使「覆盖」写入随附重嵌入；不可用则跳过——向量留待后续重建工具处理）
    let database = Database::open(db_path)?;
    let storage: Arc<dyn StorageBackend> = Arc::new(SqliteBackend::new(database));
    let vector_storage = Arc::new(LanceDbVectorStore::new(&config.storage).await?);
    let mut builder = VirtualFileSystemBuilder::new()
        .with_config(config.storage.clone())
        .with_storage(storage)
        .with_vector_storage(vector_storage);
    match ModelServices::from_config(&config.models).await {
        Ok(ms) => {
            let emb_model = config
                .models
                .resolve(ModelCapability::TextEmbedding)
                .map(|r| r.model)
                .unwrap_or_else(|| "text-embedding-3-small".to_string());
            builder = builder.with_embedding_provider(ms.embedding, emb_model);
            println!("嵌入服务：已装配（覆盖写入将自动重嵌入）");
        }
        Err(e) => {
            println!("嵌入服务：不可用（{e}）——跳过重嵌入，仅做路径修复");
        }
    }
    let vfs = builder.build()?;
    vfs.initialize().await?;
    let vfs: Arc<dyn VirtualFileSystem> = Arc::new(vfs);

    // ── 源 A：遍历扫描（可达部分；防未来新增，当前应为 0 条）──────────
    let roots = [
        TianyanUri::new(ContextNamespace::Memory, vec![]),
        TianyanUri::new(ContextNamespace::User, vec![]),
        TianyanUri::new(ContextNamespace::Agent, vec![]),
        TianyanUri::new(ContextNamespace::Skill, vec![]),
    ];
    let mut leaves: Vec<(TianyanUri, DateTime<Utc>)> = Vec::new();
    for root in &roots {
        collect_leaves(&*vfs, root, &mut leaves).await;
    }
    println!("遍历扫描：{} 条叶子（可达）", leaves.len());

    let mut items: Vec<Item> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (uri, updated) in &leaves {
        let Some(reason) = malformed_reason(uri) else {
            continue;
        };
        seen.insert(uri.to_string());
        items.push(build_item(&*vfs, uri.clone(), reason, *updated).await);
    }

    // ── 源 B：清单孤儿（遍历不可达；按 uri 精确寻址）────────────────────
    println!("清单孤儿：{} 条（审计产物）", ORPHAN_URIS.len());
    for raw in ORPHAN_URIS {
        let uri = match TianyanUri::parse(raw) {
            Ok(u) => u,
            Err(e) => {
                println!("  [解析失败] {raw}（{e}）");
                continue;
            }
        };
        if !seen.insert(uri.to_string()) {
            continue;
        }
        match vfs.get_entry(&uri).await {
            Ok(entry) => {
                if entry.is_directory() {
                    println!("  [跳过] {raw}（目录条目，非预期形态）");
                    continue;
                }
                let reason = malformed_reason(&uri).unwrap_or("孤儿条目");
                items.push(build_item(&*vfs, uri, reason, entry.metadata.updated_at).await);
            }
            Err(e) => println!("  [读取失败] {raw}（{e}）"),
        }
    }

    items.sort_by_key(|a| a.source.to_string());
    println!("\n=== 待修复：{} 条 ===", items.len());
    for it in &items {
        match &it.target {
            Some(t) => println!(
                "[{}] {}\n    → {}（id={}；更新 {}）",
                it.reason,
                it.source,
                t,
                it.id,
                it.updated.format("%m-%d %H:%M")
            ),
            None => println!(
                "[{}] {}\n    → 类别无法解析（跳过，待人工）",
                it.reason, it.source
            ),
        }
    }

    if !apply {
        println!("\n检查模式：未做任何写入。加 --apply 执行。");
        return Ok(());
    }

    // ── 备份（审计留痕）────────────────────────────────────────────────
    let backup_dir = std::env::temp_dir().join("tianyan-scratch");
    std::fs::create_dir_all(&backup_dir)?;
    let backup_path = backup_dir.join(format!(
        "memory-path-repair-backup-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let backup: Vec<serde_json::Value> = items
        .iter()
        .map(|it| {
            serde_json::json!({
                "source": it.source.to_string(),
                "reason": it.reason,
                "target": it.target.as_ref().map(|t| t.to_string()),
                "updated_at": it.updated.to_rfc3339(),
                "abstract": it.abstract_text,
                "detail": it.detail_text,
            })
        })
        .collect();
    std::fs::write(&backup_path, serde_json::to_string_pretty(&backup)?)?;
    println!("\n备份已写入：{}", backup_path.display());

    // ── 执行（单条失败不阻断）──────────────────────────────────────────
    println!("\n=== 执行 ===");
    let (mut ok, mut failed) = (0, 0);
    for it in &items {
        match apply_item(&*vfs, it).await {
            Ok(action) => {
                ok += 1;
                println!("[{action}] {}", it.source);
            }
            Err(e) => {
                failed += 1;
                println!("[失败] {}（{e}）", it.source);
            }
        }
    }
    println!("\n=== 完成：成功 {ok} / 失败 {failed} ===");
    Ok(())
}
