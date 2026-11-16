//! 临时诊断：直接对真实快照库跑 `changed_paths`，绕开 HTTP 层看清「算不出来」发生在哪一层。
//!
//! 用法（会话 id 与锚点 id 从 `GET /sessions/{id}/messages` 里取）：
//! `cargo run -p tianyan-core --example artifacts_probe -- <snapshots_root> <workdir> <session_id> <anchor>`

use tianyan::snapshot::SnapshotManager;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("用法: artifacts_probe <snapshots_root> <workdir> <session_id> <anchor>");
        std::process::exit(2);
    }
    let root = &args[0];
    let workdir = &args[1];
    let session = &args[2];
    let anchor = &args[3];

    println!("root     = {root}");
    println!("workdir  = {workdir}");
    println!("session  = {session}");
    println!("anchor   = {anchor}");
    println!(
        "workdir exists = {}",
        std::path::Path::new(workdir).exists()
    );

    let mgr = SnapshotManager::new(root.into(), workdir.into());
    match mgr.changed_paths(session, anchor).await {
        Ok(files) => {
            println!("changed_paths OK: {} files", files.len());
            for f in files.iter().take(10) {
                println!("  {} {} {}", f.status, f.size, f.path);
            }
        }
        Err(e) => println!("changed_paths ERR: {e}"),
    }
}
