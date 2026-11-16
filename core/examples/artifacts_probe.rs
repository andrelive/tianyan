//! 临时诊断：直接对真实快照库跑 `changed_paths` / `diff_trees`，看错误到底出在哪。
//!
//! 用法（会话 id 与锚点 id 从 `GET /sessions/{id}/messages` 里取）：
//! `cargo run -p tianyan-core --example artifacts_probe -- <snapshots_root> <workdir> <session_id> <anchor>`

use tianyan::snapshot::SnapshotManager;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("snapshots root");
    let workdir = args.next().expect("workdir");
    let session = args.next().expect("session id");
    let anchor = args.next().expect("anchor message id");

    println!("root     = {root}");
    println!("workdir  = {workdir}");
    println!("session  = {session}");
    println!("anchor   = {anchor}");
    println!(
        "workdir exists = {}",
        std::path::Path::new(&workdir).exists()
    );

    let mgr = SnapshotManager::new(root.into(), workdir.into());
    match mgr.changed_paths(&session, &anchor).await {
        Ok(files) => {
            println!("changed_paths OK: {} files", files.len());
            for f in files.iter().take(10) {
                println!("  {} {} {}", f.status, f.size, f.path);
            }
        }
        Err(e) => println!("changed_paths ERR: {e}"),
    }
}
