//! Server module for Tauri application
//!
//! This module handles the Axum server lifecycle within the Tauri application.
//!
//! **实现已迁至 `tianyan-desktop`**（与 `gui-native/` 共用同一份启动编排）：
//! 本模块只做 re-export，保持 `tianyan_tauri_lib::server::*` 的既有路径不变。
//!
//! 为什么迁：`find_available_port` + `start_axum_server` 此前在本模块与
//! `gui-native/src/main.rs` 各写了一遍，而**端口探测只能避免端口冲突，不能避免
//! 数据目录双写**——实测撞上过「桌面版占 3000、gui-native 顺延 3001，两个 server
//! 同时写 `D:\tianyan`」：定时任务各跑一遍、SQLite 并发写。现在两侧都走
//! `tianyan-desktop::ensure_server`：先找「同一数据目录」的已有实例并复用，
//! 找不到才自己起。
//!
//! 装配清单仍在 `tianyan-assembly`（ADR-049 期 4），本模块与 `desktop/` 都不
//! 直接认识任何插件 crate。

pub use tianyan_desktop::{
    discover_existing, ensure_server, find_available_port, start_axum_server, ServerEndpoint,
    PREFERRED_PORT, PORT_SCAN_SPAN, SERVER_HOST,
};
