//! 配置热重载 · 快照管理器重建回归（T0-12）。
//!
//! 缺陷：`reload_agent` 沿用启动期的 `snapshot_manager` 实例。改工作目录后
//! 工具在**新**目录工作，而快照/回退（delete/redo、workspace diff 都读
//! `state.snapshot_manager()`）仍在**旧**目录；若初始 `working_directory`
//! 为 `None`，热更新后快照永久不可用（直到重启）。
//!
//! 修复：抽 `build_snapshot_manager(config)` 构造单点，`new` 与 `reload_agent`
//! 共用；`snapshot_manager` 字段改 `std::sync::RwLock`（同步 getter 不变），
//! 热重载在 agent 构建成功后就地替换。

// 测试代码中 unwrap 是有意的（失败即 panic 即测试失败），豁免以保持测试可读性。
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use tempfile::tempdir;

use common::factory::test_tianyan_config_with_data_dir;

/// 改工作目录热重载 → 快照管理器必须指向**新**目录。
#[tokio::test]
async fn test_reload_agent_rebuilds_snapshot_manager_for_new_workdir() {
    let dir = tempdir().expect("创建临时目录失败");
    let ws_a = dir.path().join("ws-a");
    let ws_b = dir.path().join("ws-b");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    let mut config = test_tianyan_config_with_data_dir(dir.path().join("data"));
    config.agent.working_directory = Some(ws_a.to_string_lossy().into_owned());
    let (_router, state) = tianyan_server::create_app(config)
        .await
        .expect("create_app 失败");

    let before = state
        .snapshot_manager()
        .expect("配置了 working_directory 应启用快照");
    assert_eq!(before.workdir(), ws_a.as_path(), "启动期快照根 = 配置目录");

    // 热重载到新工作目录（update_config → reload_agent）
    let mut new_config = state.config().read().await.clone();
    new_config.agent.working_directory = Some(ws_b.to_string_lossy().into_owned());
    state.update_config(new_config).await.expect("热重载失败");

    let after = state.snapshot_manager().expect("热重载后仍应启用快照");
    assert_eq!(
        after.workdir(),
        ws_b.as_path(),
        "热重载必须按新配置重建快照管理器（T0-12：旧实现在此仍指向旧目录）"
    );
}

/// 初始未配置工作目录 → 热重载新增后快照必须可用（旧行为永久禁用）。
#[tokio::test]
async fn test_reload_agent_enables_snapshot_manager_when_workdir_added() {
    let dir = tempdir().expect("创建临时目录失败");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let mut config = test_tianyan_config_with_data_dir(dir.path().join("data"));
    config.agent.working_directory = None;
    let (_router, state) = tianyan_server::create_app(config)
        .await
        .expect("create_app 失败");
    assert!(
        state.snapshot_manager().is_none(),
        "未配置 working_directory 时快照应禁用"
    );

    let mut new_config = state.config().read().await.clone();
    new_config.agent.working_directory = Some(ws.to_string_lossy().into_owned());
    state.update_config(new_config).await.expect("热重载失败");

    let after = state
        .snapshot_manager()
        .expect("热重载新增工作目录后快照必须启用（T0-12：旧实现永久禁用）");
    assert_eq!(after.workdir(), ws.as_path());
}
