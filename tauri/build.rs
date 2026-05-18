//! Tauri 构建脚本。

fn main() {
    // 尝试构建，如果图标不存在则使用默认配置
    if std::path::Path::new("icons/icon.ico").exists() {
        tauri_build::build();
    } else {
        // 使用 try_build 允许失败时继续
        let _ = tauri_build::try_build(tauri_build::Attributes::new());
    }
}
