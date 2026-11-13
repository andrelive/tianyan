//! 数据库空间回收（`VACUUM`）：停服务后跑一次，把已删除行占用的页面还给文件系统。
//!
//! # 为什么需要
//! SQLite 删行只把页面标记为「可复用」，**文件不会缩小**——所以批量清理（executions
//! 90 天窗口、删会话、归档）之后看文件大小不变是正常的，需要显式 `VACUUM` 重建整库。
//!
//! 实测（2026-11-13 只读诊断 `D:\tianyan`）：`tianyan.db` 938.9 MB，
//! `freelist_count = 0`（当时没有可回收空洞），其中
//! `session_messages.content_parts` 407.6 MB、`executions` 73,427 行 / 79 MB。
//! 也就是说**要先真的删掉行，VACUUM 才有意义**。
//!
//! # 用法
//! ```text
//! # 先完全退出天演客户端（服务停止），再：
//! cargo run -p tianyan-tools-ops --bin vacuum_db
//! cargo run -p tianyan-tools-ops --bin vacuum_db -- --data-dir D:\tianyan
//! ```
//!
//! **必须在停服后跑**：`VACUUM` 持排他锁并重建整个文件，与运行中的写入并发会损坏库；
//! 脚本会探测 `127.0.0.1:3000`，有监听即拒绝执行。临时空间需求约 1 倍库大小。

use std::path::PathBuf;

use tianyan::common::error::{Result, TianyanError};
use tianyan::db::Database;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("vacuum_db 失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
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
    println!("数据目录：{}", config.storage.data_dir.display());
    println!("数据库：{}", db_path.display());

    // 竞态保护：VACUUM 持排他锁重建整库，运行中的服务写入会损坏它
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 3000));
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(800)).is_ok() {
        return Err(TianyanError::invalid_input(
            "检测到 127.0.0.1:3000 有服务在监听：VACUUM 必须在**停止服务**后执行\
             （持排他锁重建整库，与运行中的写入并发会损坏数据库）。请先完全退出天演客户端。",
        ));
    }
    if !db_path.is_file() {
        return Err(TianyanError::not_found(format!(
            "数据库不存在：{}",
            db_path.display()
        )));
    }

    let before = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let db = Database::open(db_path.clone())?;

    // 先报当前空洞（决定 VACUUM 值不值得跑）
    {
        let conn = db.lock().await;
        let page_count: i64 = conn
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap_or(0);
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap_or(0);
        let freelist: i64 = conn
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap_or(0);
        println!(
            "现状：{} 页 × {} B = {:.1} MB；空闲页 {}（{:.1} MB 可回收）",
            page_count,
            page_size,
            (page_count * page_size) as f64 / 1_048_576.0,
            freelist,
            (freelist * page_size) as f64 / 1_048_576.0
        );
        // WAL 先归零，避免 VACUUM 后 WAL 里还留着旧页
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    println!(
        "执行 VACUUM（临时空间需求约 {:.0} MB）…",
        before as f64 / 1_048_576.0
    );
    let started = std::time::Instant::now();
    {
        let conn = db.lock().await;
        // VACUUM 不能在事务里执行
        conn.execute_batch("VACUUM")
            .map_err(|e| TianyanError::Custom(format!("vacuum_db: VACUUM 失败：{e}")))?;
    }
    // 释放连接后再读文件大小（否则可能拿到旧元数据）
    drop(db);

    let after = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "\n完成：{:.1} MB → {:.1} MB（回收 {:.1} MB，耗时 {:.1} s）",
        before as f64 / 1_048_576.0,
        after as f64 / 1_048_576.0,
        before.saturating_sub(after) as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
