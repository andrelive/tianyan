//! 技能导入工具：把"标准化开发方法论"源目录导入 VFS 技能库。
//!
//! 背景（2026-09-14）：外部 agent 技能库（`~/.agents/skills`）
//! 中的标准化 skills 经评估后，将其方法论要点移植为本工具源目录
//! （`core/examples/import-skills/*.md`），一次性导入天演技能库
//! （`tianyan://skill/{id}`），供所有会话/子代理经 `call_skill` / `search_vfs`
//! 检索复用。
//!
//! 存储形态（与 `server::bootstrap_app_vfs` 的预置技能一致）：
//! - 目录式条目：`skill/{id}/content.md`（L2 详情）+ `skill/{id}/abstract.md`（L0 摘要）；
//! - **显式向量索引**：`vfs.write` 不触发索引（索引单点在 `update_summary_vectors`，
//!   否则 `search_vfs` 语义检索不到——预置 planning 技能即因此检不到）。
//!
//! 用法（**建议先退出天演应用**——避免与运行中实例的 LanceDB 写入冲突，
//! 与 memory_cleanup 同款前提）：
//! ```text
//! cargo run -p tianyan-core --example skill_import -- --dry-run          # 预览（不连数据目录）
//! cargo run -p tianyan-core --example skill_import                       # 导入（配置指向的数据目录）
//! cargo run -p tianyan-core --example skill_import -- --data-dir <path>  # 覆盖数据目录（彩排/隔离）
//! ```
//!
//! 幂等：已存在的技能原地更新（content/abstract/向量），可重复运行；
//! 单条失败不中止整批（失败项修复后重跑即可）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::db::Database;
use tianyan::model::ModelServices;
use tianyan::vfs::{
    ContentStore, LanceDbVectorStore, SqliteBackend, StorageBackend, VfsCore, VfsSearch,
    VirtualFileSystemBuilder, VirtualFileSystemImpl,
};

/// 单个技能源（frontmatter 元数据 + 正文）。
struct SkillSource {
    id: String,
    abstract_text: String,
    content: String,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("skill_import 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), tianyan::TianyanError> {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let verify_only = args.iter().any(|a| a == "--verify");
    let data_dir_override: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("import-skills");
    let sources = load_sources(&src_dir)?;
    println!("源目录：{}（{} 个技能）", src_dir.display(), sources.len());
    for s in &sources {
        println!("  - {}", s.id);
    }

    if dry_run {
        println!("\ndry-run（只读预览）：不连接数据目录、不写入。");
        return Ok(());
    }

    // ── 装配 VFS（与 server initialize_vfs_for_app / memory_cleanup 同构）──
    let mut config = TianyanConfig::load()?;
    if let Some(d) = &data_dir_override {
        config.storage.data_dir = d.clone();
        config.storage.sqlite_path = None;
    }
    let db_path = config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"));
    println!("\n数据目录：{}", config.storage.data_dir.display());
    println!("数据库：{}", db_path.display());

    let database = Database::open(db_path)?;
    database.init_schemas().await?;
    let storage: Arc<dyn StorageBackend> = Arc::new(SqliteBackend::new(database));
    let vector_storage = Arc::new(LanceDbVectorStore::new(&config.storage).await?);
    let model_services = ModelServices::from_config(&config.models).await?;
    let emb_model = config
        .models
        .resolve(ModelCapability::TextEmbedding)
        .map(|r| r.model)
        .unwrap_or_else(|| "text-embedding-3-small".to_string());
    let vfs = VirtualFileSystemBuilder::new()
        .with_config(config.storage.clone())
        .with_storage(storage)
        .with_vector_storage(vector_storage)
        .with_embedding_provider(model_services.embedding, emb_model)
        .build()?;
    vfs.initialize().await?;

    // ── --verify：技能检索自检（语义检索应命中导入的技能）───────────
    if verify_only {
        println!("\n--verify：技能检索自检");
        let probes = [
            ("排查 bug 的反馈回路纪律", "diagnosing-bugs"),
            (
                "多 agent 并行 全局 git 操作禁止",
                "parallel-agent-orchestration",
            ),
            ("代码审查 双轴 气味基线", "code-review"),
        ];
        let mut hit = 0usize;
        for (query, expect) in probes {
            match vfs.search(query, 5, Some(ContextNamespace::Skill)).await {
                Ok(hits) => {
                    let ok = hits.iter().any(|h| h.uri.to_string().contains(expect));
                    if ok {
                        hit += 1;
                    }
                    println!(
                        "  「{query}」→ {}（期望命中 {expect}）",
                        if ok { "命中" } else { "未命中" }
                    );
                    for h in hits.iter().take(5) {
                        println!("      {} (score {:.3})", h.uri, h.score);
                    }
                }
                Err(e) => println!("  「{query}」检索失败：{e}"),
            }
        }
        println!("  自检结果：{hit}/{} 命中", probes.len());
        return Ok(());
    }

    // ── 逐技能导入（幂等：已存在原地更新）──────────────────────────────
    println!();
    let mut ok = 0usize;
    let mut failed = 0usize;
    for s in &sources {
        match import_one(&vfs, s).await {
            Ok(updated) => {
                ok += 1;
                println!(
                    "[{}] {}（content {} 字符 + abstract + 向量）",
                    if updated { "更新" } else { "新增" },
                    s.id,
                    s.content.chars().count()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("[失败] {}：{e}（修复后重跑即可，幂等）", s.id);
            }
        }
    }
    println!("\n完成：{ok} 成功 / {failed} 失败（共 {}）", sources.len());
    if failed > 0 {
        return Err(tianyan::TianyanError::Custom(format!(
            "skill_import: {failed} 个技能导入失败"
        )));
    }
    Ok(())
}

/// 导入单个技能：目录式条目 + 两级内容 + 向量索引 + 读回自检。
async fn import_one(
    vfs: &VirtualFileSystemImpl,
    s: &SkillSource,
) -> Result<bool, tianyan::TianyanError> {
    let uri = TianyanUri::new(ContextNamespace::Skill, vec![s.id.clone()]);
    let existed = vfs.exists(&uri).await?;
    if !existed {
        vfs.create_directory(&uri).await?;
    }
    vfs.write(&uri, ContentLevel::Detail, &s.content).await?;
    vfs.write(&uri, ContentLevel::Abstract, &s.abstract_text)
        .await?;
    // 显式索引（write 不触发；缺失则 search_vfs 语义检索不到）
    vfs.update_summary_vectors(&uri, &s.abstract_text, &s.content)
        .await?;
    // 读回自检（内容精确往返）
    let back = vfs.read(&uri, ContentLevel::Detail).await?;
    if back.trim() != s.content.trim() {
        return Err(tianyan::TianyanError::Custom(format!(
            "skill_import: 读回校验失败：{}（写入 {} 字符，读回 {} 字符）",
            s.id,
            s.content.trim().chars().count(),
            back.trim().chars().count()
        )));
    }
    Ok(existed)
}

/// 加载源目录下全部 `*.md`（按文件名排序，稳定顺序）。
fn load_sources(dir: &Path) -> Result<Vec<SkillSource>, tianyan::TianyanError> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| {
            tianyan::TianyanError::Custom(format!(
                "skill_import: 源目录不可读：{}（{}）",
                dir.display(),
                e
            ))
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(tianyan::TianyanError::Custom(format!(
            "skill_import: 源目录为空：{}",
            dir.display()
        )));
    }
    files
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| {
                tianyan::TianyanError::Custom(format!(
                    "skill_import: 读取失败：{}（{}）",
                    p.display(),
                    e
                ))
            })?;
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            parse_source(&text, name)
        })
        .collect()
}

/// 解析源文件：frontmatter（`id` / `abstract` 两字段）+ 正文。
fn parse_source(text: &str, file: &str) -> Result<SkillSource, tianyan::TianyanError> {
    let mut lines = text.lines();
    let err =
        |msg: &str| tianyan::TianyanError::Custom(format!("skill_import: {file} 解析失败：{msg}"));
    if lines.next().map(str::trim) != Some("---") {
        return Err(err("缺少 frontmatter 起始 `---`"));
    }
    let mut id: Option<String> = None;
    let mut abstract_text: Option<String> = None;
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in lines {
        if in_body {
            body_lines.push(line);
            continue;
        }
        if line.trim() == "---" {
            in_body = true;
            continue;
        }
        if let Some(v) = line.strip_prefix("id:") {
            id = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("abstract:") {
            abstract_text = Some(v.trim().to_string());
        }
    }
    let id = id
        .filter(|v| !v.is_empty())
        .ok_or_else(|| err("frontmatter 缺 `id`"))?;
    let abstract_text = abstract_text
        .filter(|v| !v.is_empty())
        .ok_or_else(|| err("frontmatter 缺 `abstract`"))?;
    let content = body_lines.join("\n").trim().to_string();
    if content.is_empty() {
        return Err(err("正文为空"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(err("id 仅允许小写字母/数字/连字符"));
    }
    Ok(SkillSource {
        id,
        abstract_text,
        content,
    })
}
