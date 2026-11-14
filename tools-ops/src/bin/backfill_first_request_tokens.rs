//! 回填 `session_meta.first_request_tokens` 缓存列（一次性迁移，停服执行）。
//!
//! # 为什么需要
//! 会话列表要显示「会话首条请求的输入 token」（前端的压缩后占用基线）。此前它是
//! 一条子查询：找 `tokens.input > 0` 的**首条** assistant —— 而那个 `json_extract`
//! 条件要求**逐条解析 JSON** 才能判断是否命中。实测（2026-11-14 只读诊断 `D:\tianyan`）
//! 52 个会话 / 136,417 条消息时该查询 **1802 ms**，且每次会话列表刷新都付一次。
//!
//! 现在值缓存在 `session_meta.first_request_tokens` 列：新数据由 `append_message`
//! 首条落库时固化、`rewrite` 时按内存链重算；**老数据由本工具一次性回填**。
//! 未回填期间列表返回 NULL，前端按「基线缺失」回退原行为（不影响正确性）。
//!
//! # 用法
//! ```text
//! # 先启动一次新版天演（让 session_meta 的 ALTER TABLE 迁移跑完），再退出客户端：
//! cargo run -p tianyan-tools-ops --bin backfill_first_request_tokens -- --dry-run
//! cargo run -p tianyan-tools-ops --bin backfill_first_request_tokens
//! cargo run -p tianyan-tools-ops --bin backfill_first_request_tokens -- --data-dir D:\tianyan
//! ```
//!
//! **须停服执行**：回填要在一次 `UPDATE` 里解析所有会话的 assistant 消息 JSON，
//! 长时间持有写锁；脚本会探测 `127.0.0.1:3000`，有监听即拒绝。

use std::path::PathBuf;

use tianyan::common::error::{Result, TianyanError};
use tianyan::db::Database;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("backfill_first_request_tokens 失败：{e}");
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
    let db_path = config
        .storage
        .sqlite_path
        .clone()
        .unwrap_or_else(|| config.storage.data_dir.join("tianyan.db"));
    println!("数据库：{}", db_path.display());
    if !db_path.is_file() {
        return Err(TianyanError::not_found(format!(
            "数据库不存在：{}",
            db_path.display()
        )));
    }

    // 竞态保护：回填长时间持有写锁
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 3000));
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(800)).is_ok() {
        return Err(TianyanError::invalid_input(
            "检测到 127.0.0.1:3000 有服务在监听：回填须在**停止服务**后执行\
             （一次 UPDATE 会长时间持有写锁）。请先完全退出天演客户端。",
        ));
    }

    let db = Database::open(db_path)?;
    let conn = db.lock().await;

    // 列必须先由新版服务的启动迁移建好（否则本工具的 UPDATE 会报 no such column）
    let has_col: bool = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(session_meta)")
            .map_err(|e| TianyanError::Custom(format!("backfill: 读取表结构失败：{e}")))?;
        let cols = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| TianyanError::Custom(format!("backfill: 读取表结构失败：{e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TianyanError::Custom(format!("backfill: 读取表结构失败：{e}")))?;
        cols.iter().any(|c| c == "first_request_tokens")
    };
    if !has_col {
        return Err(TianyanError::invalid_input(
            "session_meta 缺 first_request_tokens 列：先启动一次新版服务让启动迁移\
             （ALTER TABLE）完成，再跑本工具。",
        ));
    }

    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_meta \
              WHERE parent_session_id IS NULL AND first_request_tokens IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| TianyanError::Custom(format!("backfill: 统计待回填数失败：{e}")))?;
    println!("待回填的主会话：{pending} 个");

    if pending == 0 {
        println!("无需回填。");
        return Ok(());
    }
    if dry_run {
        println!("--dry-run：不写库，退出。");
        return Ok(());
    }

    let started = std::time::Instant::now();
    // 一条 UPDATE 完成（子查询与旧版 list_meta 同口径：seq 最小的、tokens.input > 0 的 assistant）。
    // `EXISTS` 只挑「确有 assistant 消息」的会话，避免把空会话写成 NULL→NULL 的白跑。
    let changed = conn
        .execute(
            "UPDATE session_meta SET first_request_tokens = ( \
               SELECT CASE WHEN json_valid(m.content_parts) \
                           THEN json_extract(m.content_parts, '$.tokens.input') END \
                 FROM session_messages m \
                WHERE m.session_id = session_meta.session_id AND m.role = 'assistant' \
                  AND (CASE WHEN json_valid(m.content_parts) \
                            THEN json_extract(m.content_parts, '$.tokens.input') END) > 0 \
                ORDER BY m.seq ASC LIMIT 1) \
             WHERE parent_session_id IS NULL AND first_request_tokens IS NULL \
               AND EXISTS (SELECT 1 FROM session_messages m2 \
                            WHERE m2.session_id = session_meta.session_id \
                              AND m2.role = 'assistant')",
            [],
        )
        .map_err(|e| TianyanError::Custom(format!("backfill: 回填失败：{e}")))?;

    let filled: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_meta \
              WHERE parent_session_id IS NULL AND first_request_tokens IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|e| TianyanError::Custom(format!("backfill: 统计结果失败：{e}")))?;
    println!(
        "完成：更新 {changed} 行；现有 {filled} 个主会话已有基线；耗时 {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
