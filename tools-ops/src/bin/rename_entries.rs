//! 存量条目重命名：把机器名改成 LLM 起的英文 kebab-case 语义名。
//!
//! 背景（2026-11 实测 `D:\tianyan` 真实库）：三个命名空间共 **141 条**活跃条目的
//! URI 名是机器名——`evo-{时间戳}`（记忆/画像）、`rule-evo-{日期}-{时间}`（演化规则）、
//! 以及更早的纯 Unix 秒（`1786773164`）。名字里没有语义，模型检索到也只能靠 L0
//! 摘要猜，人工在界面上浏览同样不可读。
//!
//! 本工具按批（默认 25 条/次）把「原 URI + 摘要」发给 LLM，要它给出英文 kebab-case
//! 短名，然后用 `move_entry` 改名（向量点随之迁移，检索不中断）。
//!
//! 安全边界：
//! - slug 校验复用 `core::common::types::sanitize_slug`（与演化写入侧**同一份实现**）；
//!   不合格的建议直接跳过，不让半成品名字落地；
//! - 目标重名时加 `-2` 后缀，绝不覆盖已有条目；
//! - `--dry-run` 也调 LLM（否则看不到拟用名），但**不动任何数据**。
//!
//! 用法：
//! ```text
//! cargo run -p tianyan-tools-ops --bin rename_entries -- --dry-run
//! cargo run -p tianyan-tools-ops --bin rename_entries -- --namespace memory
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use tianyan::common::error::{Result, TianyanError};
use tianyan::common::llm_judge::parse_llm_json;
use tianyan::common::types::{sanitize_slug, ContentLevel, ContextNamespace, Message, TianyanUri};
use tianyan::vfs::{VfsCore, VirtualFileSystem, VirtualFileSystemImpl};
use tianyan_provider_llm::services as llm_services;

/// 每次请求 LLM 的条目数（太大反而容易漏项或串行；实测 25 条稳定）。
const BATCH: usize = 25;

/// 一个待重命名的条目。
struct Candidate {
    uri: TianyanUri,
    name: String,
    abstract_text: String,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("rename_entries 失败：{e}");
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
    let ns_filter: Option<String> = args
        .iter()
        .position(|a| a == "--namespace")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let config = tianyan_tools_ops::load_config(data_dir_override)?;
    tianyan_tools_ops::print_targets(&config);
    if dry_run {
        println!("\n>>> dry-run：会调用 LLM 取名，但不修改任何数据\n");
    }
    let vfs = tianyan_tools_ops::open_vfs(&config).await?;
    let llm = llm_services(&config.models)?;

    let candidates = collect_candidates(&vfs, ns_filter.as_deref()).await?;
    println!("待重命名 {} 条\n", candidates.len());
    if candidates.is_empty() {
        return Ok(());
    }

    let mut renamed = 0usize;
    let mut skipped = 0usize;
    for (idx, chunk) in candidates.chunks(BATCH).enumerate() {
        println!("--- 第 {} 批（{} 条）---", idx + 1, chunk.len());
        let prompt = build_prompt(chunk);
        let reply = llm
            .chat
            .chat(
                &llm.chat_model,
                vec![Message::system(SYSTEM_PROMPT), Message::user(prompt)],
            )
            .await?;
        let Some(mapping) = parse_mapping(&reply) else {
            println!("  ⚠️ 本批响应无法解析为 JSON 映射，整批跳过");
            skipped += chunk.len();
            continue;
        };

        for c in chunk {
            let Some(raw) = mapping.get(c.uri.as_str()) else {
                println!("  ⚠️ 未收到 {} 的建议名，跳过", c.name);
                skipped += 1;
                continue;
            };
            let Some(slug) = sanitize_slug(raw) else {
                println!("  ⚠️ 建议名不可用（{raw}），跳过 {}", c.name);
                skipped += 1;
                continue;
            };
            let Some(parent) = c.uri.parent() else {
                println!("  ⚠️ {} 无父目录，跳过", c.name);
                skipped += 1;
                continue;
            };
            if slug == c.name {
                println!("  = {}（名字未变）", c.name);
                continue;
            }

            if dry_run {
                println!("  {} → {slug}", c.name);
                renamed += 1;
                continue;
            }

            let dst = unique_destination(&vfs, &parent, &slug).await?;
            match vfs.move_entry(&c.uri, &dst).await {
                Ok(()) => {
                    println!(
                        "  {} → {}",
                        c.name,
                        dst.path().last().cloned().unwrap_or_default()
                    );
                    renamed += 1;
                }
                Err(e) => {
                    println!("  ⚠️ 改名失败 {}：{e}", c.name);
                    skipped += 1;
                }
            }
        }
    }

    println!("\n=== 汇总 ===");
    println!(
        "  重命名 {} 条，跳过 {} 条{}",
        renamed,
        skipped,
        if dry_run { "（dry-run 预览）" } else { "" }
    );
    Ok(())
}

const SYSTEM_PROMPT: &str =
    "你是知识库命名助手。用户会给你若干条知识条目（URI + 摘要），你要为每条起一个\
     **英文 kebab-case 语义短名**。\n\
     要求：\n\
     - 3-48 个字符，只含小写字母、数字、连字符；\n\
     - 让人只看名字就知道主题（如 `gnss-ztd-duty-cycle`、`rules-consolidation-by-topic`），\
     但不要写成一整句；\n\
     - 要具体，禁止 `note`、`misc`、`info`、`item-1` 这类无信息量的名字；\n\
     - 只输出一个 JSON 对象：键是原 URI，值是 slug。不要输出任何解释、不要用代码块包裹。";

/// 筛出机器名条目。
async fn collect_candidates(
    vfs: &VirtualFileSystemImpl,
    ns_filter: Option<&str>,
) -> Result<Vec<Candidate>> {
    let namespaces: &[ContextNamespace] = &[
        ContextNamespace::Agent,
        ContextNamespace::Memory,
        ContextNamespace::User,
    ];
    let mut out = Vec::new();
    for ns in namespaces {
        if let Some(f) = ns_filter {
            if f != ns.dir_name() {
                continue;
            }
        }
        let root = TianyanUri::new(*ns, vec![]);
        collect_in_dir(vfs, &root, &mut out, 0).await?;
    }
    Ok(out)
}

/// 递归收集（跳过系统/归档目录）。
async fn collect_in_dir(
    vfs: &VirtualFileSystemImpl,
    dir: &TianyanUri,
    out: &mut Vec<Candidate>,
    depth: usize,
) -> Result<()> {
    if depth > 4 {
        return Ok(());
    }
    let entries = match vfs.list(dir).await {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let uri = entry.uri();
        if tianyan::common::types::system_paths::is_system_path(uri) {
            continue;
        }
        if entry.is_directory() {
            Box::pin(collect_in_dir(vfs, uri, out, depth + 1)).await?;
            continue;
        }
        let name = uri.path().last().cloned().unwrap_or_default();
        if !is_machine_name(&name) {
            continue;
        }
        let abstract_text = vfs
            .read_content(uri, ContentLevel::Abstract)
            .await
            .unwrap_or_default();
        out.push(Candidate {
            uri: uri.clone(),
            name,
            abstract_text,
        });
    }
    Ok(())
}

/// 机器名形态：`evo-{数字}[-{n}]`、`rule-evo-{8位}-{6位}`、纯 10 位 Unix 秒。
fn is_machine_name(name: &str) -> bool {
    let is_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = name.strip_prefix("evo-") {
        let mut parts = rest.split('-');
        let first = parts.next().unwrap_or("");
        let second = parts.next();
        return is_digits(first) && parts.next().is_none() && second.is_none_or(is_digits);
    }
    if let Some(rest) = name.strip_prefix("rule-evo-") {
        let b = rest.as_bytes();
        return b.len() == 15
            && b[8] == b'-'
            && b[..8].iter().all(u8::is_ascii_digit)
            && b[9..].iter().all(u8::is_ascii_digit);
    }
    name.len() == 10 && is_digits(name)
}

fn build_prompt(chunk: &[Candidate]) -> String {
    let mut s = String::from("条目：\n");
    for c in chunk {
        let abstract_text = c.abstract_text.replace('\n', " ");
        let head = tianyan::common::truncate::truncate_utf8_boundary(&abstract_text, 240);
        s.push_str(&format!("\n{}\n摘要：{}\n", c.uri.as_str(), head.trim()));
    }
    s
}

fn parse_mapping(reply: &str) -> Option<BTreeMap<String, String>> {
    let value = parse_llm_json(reply)?;
    let obj = value.as_object()?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            out.insert(k.clone(), s.to_string());
        }
    }
    Some(out)
}

/// 目标已存在时加 `-2`..`-99` 后缀（绝不覆盖已有条目）。
async fn unique_destination(
    vfs: &VirtualFileSystemImpl,
    dir: &TianyanUri,
    slug: &str,
) -> Result<TianyanUri> {
    let mut candidate = dir.append(slug);
    if !vfs.exists(&candidate).await? {
        return Ok(candidate);
    }
    for n in 2..=99u32 {
        candidate = dir.append(&format!("{slug}-{n}"));
        if !vfs.exists(&candidate).await? {
            return Ok(candidate);
        }
    }
    Err(TianyanError::conflict(format!(
        "rename_entries: 目标目录已满（{slug} 的序号后缀用尽）"
    )))
}
