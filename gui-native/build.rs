//! 构建脚本：把应用图标与版本信息嵌进 exe 的 Windows 资源段。
//!
//! 为什么必须做：GPUI 的**窗口与任务栏图标取自可执行文件资源**（代码里没有显式设
//! 图标），不嵌就是白板图标——做成 MSI 发给用户后，任务栏、开始菜单、Alt+Tab 里
//! 都认不出是天演，观感比 tauri 版差一大截。
//!
//! 图标**复用 tauri 版那一套**（`tauri/icons/icon.ico`，内含 16~256 多尺寸），
//! 这样两个客户端在系统里长得一模一样：替换安装时用户不会觉得「换了个软件」。
//!
//! 失败不致命：嵌不进去只影响观感，构建照常继续（`cargo:warning` 会提示）。

fn main() {
    // 图标或版本变了要重新跑本脚本（否则 cargo 认为 exe 无需重链）
    println!("cargo:rerun-if-changed=../tauri/icons/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let ico = std::path::Path::new("../tauri/icons/icon.ico");
        if !ico.exists() {
            println!("cargo:warning=找不到图标 {ico:?}，跳过嵌入（exe 将使用系统默认图标）");
            return;
        }
        let mut res = winres::WindowsResource::new();
        res.set_icon(ico.to_string_lossy().as_ref());
        // 资源管理器「属性 → 详细信息」里显示的名字；与 MSI 的 Name 对齐
        res.set("ProductName", "天演");
        res.set("FileDescription", "天演 · GPUI 原生端");
        res.set("InternalName", "tianyan-gui-native");
        res.set("OriginalFilename", "tianyan-gui-native.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=嵌入 exe 资源失败（不影响功能）：{e}");
        }
    }
}
