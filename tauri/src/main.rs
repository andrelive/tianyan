//! Tauri 桌面应用入口。

// Windows GUI 子系统：不弹控制台窗口（否则启动时多一个黑窗口）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tianyan_tauri_lib::run;

fn main() {
    run();
}
