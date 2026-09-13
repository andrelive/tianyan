//! 记忆/规则存量清洗工具（一次性；记忆治理波次配套）。
//!
//! 背景（2026-09-13 侦查结论）：
//! - 演化删除通道因 `find_entry` 仅匹配目录的缺陷自上线起从未生效 → 早期
//!   自动规则提炼产生 357 条（2026-08-16~18）重复/过时存量从未被清理；
//! - 运维数据（extraction_state 遗物、演化报告）混入记忆命名空间；
//! - 同主题碎片遍布（发布流水、事件架构、环境事实、审查流程……）；
//! - 短条目概览（L1）曾被 LLM 扩写污染（prompt 修复前）。
//!
//! 本工具执行一次性清洗（**幂等**，可重复运行；建议先退出天演应用避免
//! LanceDB 写入冲突）：
//! 1. 硬删死数据：extraction_state 遗物 + 已删除任务的状态文件；
//! 2. 归档过时/重复/流水账记忆（软删除到 `memory/archive/`）；
//! 3. 合并同主题碎片为 8 条主题条目（记忆 4 组 + user 2 组 + 模式 1 组 +
//!    指令遵循 1 组），源条目归档；
//! 4. 对 User/Memory/Agent(learned·patterns)/Skill(learned) 全部保留条目
//!    重建 L1（= L2 全文，修正历史扩写污染）并重嵌入；
//! 5. 清理演化报告的摘要与向量（delete + rewrite 内容，报告降为纯日志）；
//! 6. 归档 2026-08-19 之前创建的规则（早期爆炸存量）+ 空壳/一次性规则。
//!
//! 用法：
//! ```text
//! cargo run -p tianyan-core --example memory_cleanup -- --dry-run   # 预览
//! cargo run -p tianyan-core --example memory_cleanup                # 执行
//! ```

use std::sync::Arc;

use chrono::TimeZone;
use tianyan::common::types::{AgentPath, ContentLevel, ContextNamespace, TianyanUri};
use tianyan::config::{ModelCapability, TianyanConfig};
use tianyan::db::Database;
use tianyan::model::ModelServices;
use tianyan::vfs::{
    ContentStore, LanceDbVectorStore, SqliteBackend, StorageBackend, VfsCore, VfsSearch,
    VirtualFileSystem, VirtualFileSystemBuilder,
};

/// 规则归档边界：此日期之前创建的规则视为早期爆炸存量（archived）。
const RULE_ARCHIVE_BEFORE: (i32, u32, u32) = (2026, 8, 19);

fn mem(segments: &[&str]) -> TianyanUri {
    uri_of(ContextNamespace::Memory, segments)
}

fn uri_of(ns: ContextNamespace, segments: &[&str]) -> TianyanUri {
    TianyanUri::new(ns, segments.iter().map(|s| s.to_string()).collect())
}

/// 合并计划：把同主题碎片归纳为一条主题条目（源条目归档）。
struct MergeSpec {
    target: TianyanUri,
    content: &'static str,
    sources: Vec<TianyanUri>,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("memory_cleanup 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), tianyan::TianyanError> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");

    // ── 装配 VFS（与 server initialize_vfs_for_app 同构）──────────────────
    let config = TianyanConfig::load()?;
    let db_path = config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"));
    println!("数据目录：{}", config.storage.data_dir.display());
    println!("数据库：{}", db_path.display());
    let database = Database::open(db_path)?;
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

    println!(
        "模式：{}",
        if dry_run {
            "dry-run（只读预览）"
        } else {
            "执行"
        }
    );

    // ── 1. 硬删死数据 ──────────────────────────────────────────────────
    hard_delete_dead_data(&vfs, dry_run).await?;
    // ── 2. 归档过时/重复/流水账记忆 ────────────────────────────────────
    archive_stale_memories(&vfs, dry_run).await?;
    // ── 3. 合并同主题碎片 ─────────────────────────────────────────────
    merge_topics(&vfs, dry_run).await?;
    // ── 4. 归档早期规则存量（先归档，避免 L1 重建浪费在待归档条目上）──
    archive_old_rules(&vfs, dry_run).await?;
    // ── 5. 重建保留条目 L1 + 重嵌入 ───────────────────────────────────
    rebuild_overviews(&vfs, dry_run).await?;
    // ── 6. 清理演化报告摘要/向量 ──────────────────────────────────────
    strip_operational_summaries(&vfs, dry_run).await?;

    println!("\n完成。");
    Ok(())
}

// ── 1. 硬删死数据 ────────────────────────────────────────────────────────

async fn hard_delete_dead_data(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let mut targets = Vec::new();
    // extraction_state：旧记忆提取管道遗物（代码已删除，数据残留）
    let es = mem(&["events", "extraction_state"]);
    for entry in vfs.list(&es).await.unwrap_or_default() {
        if !entry.is_directory() {
            targets.push(entry.uri().clone());
        }
    }
    // memory_extraction 任务状态：任务已删除（ADR-017），状态文件无用
    let me = mem(&["events", "task_states", "memory_extraction.md"]);
    if vfs.exists(&me).await.unwrap_or(false) {
        targets.push(me);
    }

    println!("\n[1] 硬删死数据：{} 条", targets.len());
    for uri in &targets {
        println!("    - {uri}");
        if !dry_run {
            vfs.delete(uri).await?;
        }
    }
    Ok(())
}

// ── 2. 归档过时/重复/流水账记忆 ──────────────────────────────────────────

/// 纯归档清单（不含合并源——合并源由 merge 步骤处理）。
fn stale_memory_list() -> Vec<TianyanUri> {
    let items: &[&[&str]] = &[
        // 过时/被实测证伪（shell 元字符禁令；正确知识见 env-powershell-5.1-verified 与技能）
        &["cases", "failed_tasks", "1786854838"],
        &["facts", "1787065682"],
        // 发布流水账（可复用知识已沉淀在 tianyan-release-build 技能与 CHANGELOG/REFACTOR_LOG）
        &["cases", "successful_tasks", "evo-1789105544"],
        &["cases", "successful_tasks", "evo-1789280062"],
        // 实现过程流水（与同名 facts 条目重复）
        &["cases", "successful_tasks", "evo-1788585333"],
        &["cases", "successful_tasks", "evo-1788844967"],
        &["cases", "successful_tasks", "evo-1788931688"],
        &["cases", "successful_tasks", "evo-1789018378"],
        &["cases", "successful_tasks", "evo-1788492487"],
        // 审查流水（技能 codebase-* 已覆盖方法论）
        &["cases", "successful_tasks", "1786885114"],
        &["cases", "successful_tasks", "1786890812"],
        &["cases", "successful_tasks", "1786891411"],
        // 命令教训（技能 windows-powershell-commanding / ps51-command-patterns 已覆盖）
        &["cases", "failed_tasks", "1786860719"],
        &["cases", "failed_tasks", "1786891411"],
        &["cases", "successful_tasks", "1786860111"],
        // ripgrep 重复对（保留 1786957130 完整版）
        &["cases", "failed_tasks", "1786953621"],
        // SNN 案例（技能 snn-circuit-debugging 已覆盖）
        &["cases", "successful_tasks", "snn-circuit-debugging-success"],
    ];
    items.iter().map(|p| mem(p)).collect()
}

async fn archive_stale_memories(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let archive_root = mem(&["archive"]);
    let targets = stale_memory_list();
    println!("\n[2] 归档过时/重复/流水账记忆：{} 条", targets.len());
    let mut archived = 0;
    for uri in &targets {
        if archive_entry(
            vfs,
            uri,
            &archive_root,
            "存量清洗：过时/重复/流水账",
            dry_run,
        )
        .await?
        {
            archived += 1;
        }
    }
    println!("    实际归档：{archived}（已不存在的条目已跳过）");
    Ok(())
}

// ── 3. 合并同主题碎片 ────────────────────────────────────────────────────

fn merge_specs() -> Vec<MergeSpec> {
    vec![
        MergeSpec {
            target: mem(&["facts", "project-environment"]),
            content: "\
天演（Tianyan）是 AI 自身的源码项目，位于 `F:\\work\\git\\tianyan`。

- **技术栈**：Rust（core / server / mcp / tauri 四个 crate）+ Tauri 2 桌面包装 + gui-vite（React/TypeScript，Vite 构建）
- **构建产物**：MSI 安装包（`Tianyan_<版本>_x64_<lang>.msi`，zh-CN/en-US；workspace `target/release/bundle/msi/`）
- **运行环境**：Windows + PowerShell 5.1；server 固定监听 `127.0.0.1:3000`（不解析 CLI 参数）
- **数据目录**：`D:\\tianyan`（由 `~/.tianyan/tianyan.toml` 的 `storage.data_dir` 指定；配置与数据目录分离）
- **用户偏好**：中文交流与输出；偏好简洁表格；提交默认仅本地（发布场景除外，见发布模式）",
            sources: vec![
                mem(&["facts", "1786854230"]),
                mem(&["facts", "1786890812"]),
                mem(&["facts", "1786891411"]),
                mem(&["facts", "evo-1788492487"]),
            ],
        },
        MergeSpec {
            target: mem(&["facts", "architecture-constraints"]),
            content: "\
天演架构硬约束与文档索引。

- **硬约束（AGENTS.md）**：VFS 为唯一存储/检索抽象——禁止独立向量库/RAG 管道、独立记忆存储、技能全量加载、chunk 分块；错误统一 `TianyanError`（仅 Io/Json/Toml/Custom 4 变体 + 语义谓词，ADR-014）；代码是唯一事实来源（文档可能过时）
- **文档索引**：架构决策 ADR-001~033（`docs/architecture/decisions/`，含 REJECTED.md 否决记录）；机制文档（context-pipeline / event-protocol / model-provider-notes / task-runtime）；运维手册（troubleshooting / data-health-check / release-msi）
- **子系统要点**：MCP 客户端基于 rust-mcp-sdk（stdio 传输，支持工具列表/调用 + base64 图片内容块）；前端含 CodeMirror 编辑器与 react-markdown 渲染、react-arborist 树",
            sources: vec![
                mem(&["facts", "1786860111"]),
                mem(&["facts", "1786869714"]),
                mem(&["facts", "1786885114"]),
                mem(&["events", "decisions", "1786890812"]),
            ],
        },
        MergeSpec {
            target: mem(&["facts", "event-push-architecture"]),
            content: "\
统一事件推送架构（ADR-028/029/031）。

- **核心**：落库即推送——持久化后经统一通道发送 `chat_stream` 边界事件；前端常驻 EventSource 接收（type 区分：消息/任务状态/命令输出/聊天流）；断线重连走快照对齐（DSH 模式）
- **消息去重**：前端 `mergeServerMessages` 按 id 去重合并入库
- **消息边界**：由 core 在持久化后立即发送（服务端不猜测轮次，避免竞态取到上一轮输出）
- **任务通知**：后台命令完成通知从「落库 + 轮询」升级为实时推送（`event_push.rs` 注入 `event_tx`）
- **前端归约**：流式归约器（createChatStreamReducer）为协议到 store 的唯一入口，组件仅负责启停；测试需手动模拟事件通道，订阅类事件须等连接就绪并在重连时强制重放",
            sources: vec![
                mem(&["facts", "evo-1788585333"]),
                mem(&["facts", "evo-1788844967"]),
                mem(&["facts", "evo-1788931688"]),
                mem(&["facts", "evo-1789018378"]),
                mem(&["facts", "evo-1788671915"]),
            ],
        },
        MergeSpec {
            target: mem(&["facts", "instruction-following"]),
            content: "\
指令遵循模式：用户要求「仅回复指定内容 / 严格输出格式」时必须完全照做——不添加解释、不附加额外内容；要求审查/分析时提供深度评估。历史多次违反属高频问题（含「只回复」与「审查前先读文档」两类），执行前先识别指令是否有严格限定。",
            sources: vec![
                mem(&["cases", "successful_tasks", "1786953621"]),
                mem(&["cases", "failed_tasks", "1787065682"]),
            ],
        },
        MergeSpec {
            target: uri_of(ContextNamespace::User, &["preferences", "communication"]),
            content: "\
用户交流偏好：中文交流与输出；偏好简洁、表格化呈现（曾要求一句话回复）；结论必须有证据——未验证论断会被否定，讨论数据结构/API 语义、框架行为须核实规范或源码，方案先列取舍与反例自检。",
            sources: vec![
                uri_of(ContextNamespace::User, &["preferences", "1786779936"]),
                uri_of(ContextNamespace::User, &["preferences", "1786854230"]),
                uri_of(ContextNamespace::User, &["preferences", "1786957130"]),
                uri_of(ContextNamespace::User, &["preferences", "evo-1789280062"]),
            ],
        },
        MergeSpec {
            target: uri_of(ContextNamespace::User, &["entities", "tianyan-project"]),
            content: "\
天演项目档案：基于 LLM 的本地智能代理系统（AI 自身源码），Rust workspace 含 core / server / tauri / mcp 四个 crate + gui-vite（React+TS），约 69K 行。技术栈：Tokio、Axum 0.8、reqwest、serde、async-openai、LanceDB（嵌入式向量）、tree-sitter、Tauri 2.2。内置约 29 个工具（文件读写、补丁应用、命令执行、代码搜索、LSP、网页搜索、知识库、委托、后台任务、审批等）。",
            sources: vec![
                uri_of(ContextNamespace::User, &["entities", "1786854838"]),
                uri_of(ContextNamespace::User, &["entities", "1786860111"]),
                uri_of(ContextNamespace::User, &["entities", "1786860719"]),
                uri_of(ContextNamespace::User, &["entities", "1786869714"]),
                uri_of(ContextNamespace::User, &["entities", "1786885114"]),
                uri_of(ContextNamespace::User, &["entities", "1786890812"]),
                uri_of(ContextNamespace::User, &["entities", "1786957130"]),
            ],
        },
        MergeSpec {
            target: uri_of(ContextNamespace::Agent, &["patterns", "code-review-workflow"]),
            content: "\
代码审查工作流（审查-改进闭环）：先读 README / Cargo.toml / AGENTS.md 把握定位与约束，统计 crate 规模；扫描 unwrap/expect/panic/todo 定位生产风险（区分测试与生产代码）；深入模块（agent loop、executor、工具注册表、服务端路由）逐层审查；并行运行 cargo check/clippy；重点核查安全边界（命令执行、SSRF、路径沙箱）与异步阻塞；输出可执行改进清单，迭代「审查 → 改进 → 再审查」而非一次性结论。",
            sources: vec![
                uri_of(ContextNamespace::Agent, &["patterns", "1786854230"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786854838"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786860111"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786860719"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786869714"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786891411"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1786957130"]),
                uri_of(ContextNamespace::Agent, &["patterns", "1787065682"]),
            ],
        },
    ]
}

async fn merge_topics(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let specs = merge_specs();
    println!("\n[3] 合并同主题碎片：{} 组", specs.len());
    for spec in &specs {
        let mut sources_note = String::new();
        if !spec.sources.is_empty() {
            sources_note = format!(" ← {} 条源", spec.sources.len());
        }
        println!("    - {}{sources_note}", spec.target);
        if dry_run {
            continue;
        }
        // 写目标（完整写路径：L2 + L0 + L1=全文 + 向量）
        write_entry(vfs, &spec.target, spec.content, "cleanup-merge").await?;
        // 归档源条目（各自命名空间归档目录）
        for src in &spec.sources {
            let archive_root = archive_root_for(src);
            archive_entry(
                vfs,
                src,
                &archive_root,
                &format!("已被合并到 {}", spec.target),
                false,
            )
            .await?;
        }
    }
    Ok(())
}

/// 源条目的归档根目录（按命名空间约定）。
fn archive_root_for(src: &TianyanUri) -> TianyanUri {
    match src.namespace() {
        ContextNamespace::Memory => mem(&["archive"]),
        ContextNamespace::User => uri_of(ContextNamespace::User, &["archive"]),
        ContextNamespace::Agent => uri_of(ContextNamespace::Agent, &["_archive"]),
        ContextNamespace::Skill => uri_of(ContextNamespace::Skill, &["_archive"]),
        ContextNamespace::Knowledge => uri_of(ContextNamespace::Knowledge, &["_archive"]),
        ContextNamespace::Session => uri_of(ContextNamespace::Session, &["archive"]),
    }
}

// ── 4. 重建保留条目 L1 + 重嵌入 ──────────────────────────────────────────

/// 重建范围：User（全部）+ Memory（排除运维子域）+ Agent(learned·patterns)
/// + Skill(learned) 的全部叶子条目。
async fn rebuild_overviews(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let roots = [
        uri_of(ContextNamespace::User, &[]),
        mem(&[]),
        uri_of(ContextNamespace::Agent, &["learned"]),
        uri_of(ContextNamespace::Agent, &["patterns"]),
        uri_of(ContextNamespace::Skill, &[]),
    ];

    let mut leaves = Vec::new();
    for root in &roots {
        // Agent/Skill 根的 learned 子目录由 VFS 自动建链；直接递归收集
        collect_leaves(vfs, root, &mut leaves, 0).await;
    }
    // Memory 运维子域 + 各命名空间归档目录排除
    leaves.retain(|u| {
        !tianyan::common::types::memory_paths::is_operational_path(u)
            && !u.as_str().contains("/archive/")
            && !u.as_str().contains("/_archive/")
            && !u.as_str().contains("/learned/archive/")
    });

    println!("\n[5] 重建 L1（= L2 全文）+ 重嵌入：{} 条", leaves.len());
    let mut rebuilt = 0;
    for uri in &leaves {
        let Ok(detail) = vfs.read_content(uri, ContentLevel::Detail).await else {
            continue;
        };
        if dry_run {
            rebuilt += 1;
            continue;
        }
        let Ok(abstract_text) = vfs.read_abstract(uri).await else {
            continue;
        };
        // 已满足 L1=L2 的跳过（幂等，避免重复嵌入）
        if vfs
            .read_content(uri, ContentLevel::Overview)
            .await
            .map(|ov| ov == detail)
            .unwrap_or(false)
        {
            continue;
        }
        vfs.write_overview(uri, &detail).await?;
        vfs.update_summary_vectors(uri, &abstract_text, &detail)
            .await?;
        rebuilt += 1;
        if rebuilt % 25 == 0 {
            println!("    …已重建 {rebuilt}");
        }
    }
    println!("    重建完成：{rebuilt}");
    Ok(())
}

/// 递归收集叶子（深度上限防失控）。
async fn collect_leaves(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dir: &TianyanUri,
    out: &mut Vec<TianyanUri>,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = vfs.list(dir).await else {
        return;
    };
    for entry in entries {
        if entry.is_directory() {
            if tianyan::common::types::memory_paths::is_operational_path(entry.uri()) {
                continue;
            }
            Box::pin(collect_leaves(vfs, entry.uri(), out, depth + 1)).await;
        } else {
            out.push(entry.uri().clone());
        }
    }
}

// ── 5. 清理演化报告摘要/向量 ─────────────────────────────────────────────

async fn strip_operational_summaries(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let reports = mem(&["events", "evolution_reports"]);
    let mut count = 0;
    println!("\n[6] 清理运维条目摘要与向量（delete + rewrite）");
    for entry in vfs.list(&reports).await.unwrap_or_default() {
        if entry.is_directory() {
            continue;
        }
        if dry_run {
            count += 1;
            continue;
        }
        if let Ok(content) = vfs.read_content(entry.uri(), ContentLevel::Detail).await {
            vfs.delete(entry.uri()).await?;
            vfs.write_content(entry.uri(), &content).await?;
            count += 1;
        }
    }
    // task_states：状态文件（read 仅依赖 Detail）——清 L1 与向量，
    // 保留 Detail 与首行 L0（与 task_state.write 的语义一致）
    let states = mem(&["events", "task_states"]);
    for entry in vfs.list(&states).await.unwrap_or_default() {
        if entry.is_directory() {
            continue;
        }
        if dry_run {
            count += 1;
            continue;
        }
        if let Ok(content) = vfs.read_content(entry.uri(), ContentLevel::Detail).await {
            vfs.delete(entry.uri()).await?;
            vfs.write_content(entry.uri(), &content).await?;
            let first_line = content.lines().next().unwrap_or("").to_string();
            vfs.write_abstract(entry.uri(), &first_line).await?;
            count += 1;
        }
    }
    println!("    已处理：{count}");
    Ok(())
}

// ── 6. 归档早期规则存量 ──────────────────────────────────────────────────

/// 强制归档清单（空壳/一次性事件——不受日期边界保护）。
const FORCE_ARCHIVE_RULES: &[&str] = &[
    "rule-20260903-085953",
    "rule-20260903-080010",
    "rule-20260903-081432",
];

async fn archive_old_rules(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    dry_run: bool,
) -> Result<(), tianyan::TianyanError> {
    let learned = AgentPath::Learned.uri();
    let archive_root = AgentPath::learned_archive();
    let cutoff = chrono::Utc
        .with_ymd_and_hms(
            RULE_ARCHIVE_BEFORE.0,
            RULE_ARCHIVE_BEFORE.1,
            RULE_ARCHIVE_BEFORE.2,
            0,
            0,
            0,
        )
        .unwrap();

    let mut targets = Vec::new();
    for entry in vfs.list(&learned).await.unwrap_or_default() {
        if entry.is_directory() {
            continue;
        }
        let id = entry.uri().path().last().cloned().unwrap_or_default();
        let force = FORCE_ARCHIVE_RULES.contains(&id.as_str());
        if force || entry.metadata.created_at < cutoff {
            targets.push(entry.uri().clone());
        }
    }

    println!(
        "\n[4] 归档早期规则存量（< 2026-08-19 创建）：{} 条",
        targets.len()
    );
    let mut moved = 0;
    for uri in &targets {
        let id = uri.path().last().cloned().unwrap_or_default();
        if dry_run {
            moved += 1;
            continue;
        }
        let dst = archive_root.append(&id);
        if let Some(parent) = dst.parent() {
            if !vfs.exists(&parent).await.unwrap_or(false) {
                vfs.create_directory(&parent).await?;
            }
        }
        if vfs.exists(uri).await.unwrap_or(false) {
            vfs.move_entry(uri, &dst).await?;
            moved += 1;
        }
    }
    println!("    实际归档：{moved}");
    Ok(())
}

// ── 公共操作 ─────────────────────────────────────────────────────────────

/// 归档条目：读原内容 → 写归档副本（标注原因）→ 删除原条目。
/// 返回是否实际归档（源不存在时 false；幂等）。
async fn archive_entry(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    uri: &TianyanUri,
    archive_root: &TianyanUri,
    marker: &str,
    dry_run: bool,
) -> Result<bool, tianyan::TianyanError> {
    if !vfs.exists(uri).await.unwrap_or(false) {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    let id = uri.path().last().cloned().unwrap_or_default();
    let archive = archive_root.append(&id);
    if let Some(parent) = archive.parent() {
        if !vfs.exists(&parent).await.unwrap_or(false) {
            vfs.create_directory(&parent).await?;
        }
    }
    if let Ok(content) = vfs.read_content(uri, ContentLevel::Detail).await {
        vfs.write_content(&archive, &content).await?;
    }
    if let Ok(abstract_text) = vfs.read_abstract(uri).await {
        vfs.write_abstract(&archive, &format!("[已归档: {marker}] {abstract_text}"))
            .await?;
    }
    if let Ok(overview) = vfs.read_content(uri, ContentLevel::Overview).await {
        let _ = vfs.write_overview(&archive, &overview).await;
    }
    vfs.delete(uri).await?;
    Ok(true)
}

/// 完整写条目：L2 + L0 + L1(=L2) + 向量（合并目标专用）。
async fn write_entry(
    vfs: &tianyan::vfs::VirtualFileSystemImpl,
    uri: &TianyanUri,
    content: &str,
    source_label: &str,
) -> Result<(), tianyan::TianyanError> {
    if let Some(parent) = uri.parent() {
        if !vfs.exists(&parent).await.unwrap_or(false) {
            vfs.create_directory(&parent).await?;
        }
    }
    let first_line: String = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let abstract_text = format!("{first_line} | 来源: {source_label}");
    vfs.write_content(uri, content).await?;
    vfs.write_abstract(uri, &abstract_text).await?;
    vfs.write_overview(uri, content).await?;
    vfs.update_summary_vectors(uri, &abstract_text, content)
        .await?;
    Ok(())
}
