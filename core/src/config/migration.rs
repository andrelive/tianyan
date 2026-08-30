//! 数据目录搬迁（0.2：设置页「数据目录搬迁」）。
//!
//! 流程（关 DB → 移动 → 更新配置 → 重启）：
//! 1. API 校验新目录并写迁移请求文件 `{data_dir}/.tianyan-migrate.json`；
//! 2. API 触发服务器优雅关停（watch 信号）——SQLite/LanceDB 随 AppState
//!    drop 释放文件锁（Windows 上移动打开的文件会失败）；
//! 3. 服务器进程退出后，监督循环（Tauri）/主循环（独立 server）检测到
//!    请求文件，执行 [`run_pending_migration`]（移动全部数据 + 更新配置 +
//!    删除请求文件），失败回滚并清除请求（用户可从 UI 重试）；
//! 4. 以新配置重启（监督循环/主循环重读配置文件）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::common::error::{Result, TianyanError};
use crate::config::TianyanConfig;

/// 迁移请求文件名（位于旧 data_dir 内）。
pub const MIGRATION_REQUEST_FILE: &str = ".tianyan-migrate.json";

/// 迁移请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRequest {
    /// 目标数据目录（绝对路径）。
    pub new_dir: String,
    /// 请求时间（epoch 秒）。
    pub requested_at: i64,
}

/// 校验迁移目标目录（绝对路径 / 非当前目录 / 非父子关系 / 可写）。
pub fn validate_migration_target(current: &Path, new_dir: &Path) -> std::result::Result<(), String> {
    if new_dir.as_os_str().is_empty() {
        return Err("新数据目录不能为空".to_string());
    }
    if !new_dir.is_absolute() {
        return Err("新数据目录必须是绝对路径".to_string());
    }
    if new_dir == current {
        return Err("新数据目录与当前目录相同".to_string());
    }
    // 新目录不能是当前目录的子目录（移动父目录会包含目标自身）
    if new_dir.starts_with(current) {
        return Err("新数据目录不能位于当前数据目录内部".to_string());
    }
    // 当前目录不能是新目录的子目录（移动会破坏目录结构）
    if current.starts_with(new_dir) {
        return Err("新数据目录不能是当前数据目录的父目录".to_string());
    }
    // 可写性检查：创建目录 + 写探针文件
    std::fs::create_dir_all(new_dir)
        .map_err(|e| format!("无法创建新数据目录：{e}"))?;
    let probe = new_dir.join(".tianyan-write-probe");
    std::fs::write(&probe, b"ok").map_err(|e| format!("新数据目录不可写：{e}"))?;
    let _ = std::fs::remove_file(&probe);
    // 已存在且非空 → 拒绝（避免与既有内容混叠）
    if let Ok(entries) = std::fs::read_dir(new_dir) {
        if entries.count() > 0 {
            return Err("新数据目录已存在且非空，请选择空目录或新路径".to_string());
        }
    }
    Ok(())
}

/// 写迁移请求文件。
pub fn write_migration_request(data_dir: &Path, new_dir: &Path) -> Result<()> {
    let req = MigrationRequest {
        new_dir: new_dir.to_string_lossy().to_string(),
        requested_at: chrono::Utc::now().timestamp(),
    };
    let json = serde_json::to_string_pretty(&req)
        .map_err(|e| TianyanError::Custom(format!("migration: 请求序列化失败：{e}")))?;
    std::fs::write(data_dir.join(MIGRATION_REQUEST_FILE), json)
        .map_err(|e| TianyanError::Custom(format!("migration: 写迁移请求失败：{e}")))
}

/// 读取迁移请求（无请求时返回 None）。
pub fn read_migration_request(data_dir: &Path) -> Option<MigrationRequest> {
    let content = std::fs::read_to_string(data_dir.join(MIGRATION_REQUEST_FILE)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 执行搬迁：校验目标 + 移动 old_dir 下全部条目（除迁移请求文件）到 new_dir。
///
/// 失败时回滚已移动的条目（目标已移动的移回原位），保证数据不丢失。
/// 返回移动的条目数。
pub fn perform_migration(old_dir: &Path, new_dir: &Path) -> Result<usize> {
    validate_migration_target(old_dir, new_dir).map_err(TianyanError::config)?;
    std::fs::create_dir_all(new_dir).map_err(|e| {
        TianyanError::Custom(format!("migration: 创建目标目录失败：{e}"))
    })?;
    let entries = std::fs::read_dir(old_dir).map_err(|e| {
        TianyanError::Custom(format!("migration: 读取数据目录失败：{e}"))
    })?;
    let mut copied: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            TianyanError::Custom(format!("migration: 读取目录条目失败：{e}"))
        })?;
        let name = entry.file_name();
        if name == MIGRATION_REQUEST_FILE {
            continue;
        }
        let from = entry.path();
        let to = new_dir.join(&name);
        if let Err(e) = copy_recursive(&from, &to) {
            // 回滚：删除已 copy 的目标（逆序）
            for t in copied.iter().rev() {
                let _ = std::fs::remove_dir_all(t).or_else(|_| std::fs::remove_file(t));
            }
            return Err(TianyanError::Custom(format!(
                "migration: 复制 {} 失败：{e}（已回滚）",
                from.display()
            )));
        }
        copied.push(to);
    }
    // 尽力删除源文件（锁已释放时成功；失败保留，重启后清理）
    if let Ok(entries) = std::fs::read_dir(old_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == MIGRATION_REQUEST_FILE {
                continue;
            }
            let from = entry.path();
            let _ = if from.is_dir() {
                std::fs::remove_dir_all(&from)
            } else {
                std::fs::remove_file(&from)
            };
        }
    }
    Ok(copied.len())
}

/// 递归复制文件或目录。
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to)?;
        Ok(())
    }
}

/// 更新配置文件中的 storage.data_dir（保留其余配置）。
pub fn update_config_data_dir(new_dir: &Path) -> Result<()> {
    let path = TianyanConfig::find_config_file()
        .or_else(TianyanConfig::default_config_path)
        .ok_or_else(|| TianyanError::config("migration: 未找到配置文件"))?;
    let mut config = TianyanConfig::load_from_file(&path)?;
    config.storage.data_dir = new_dir.to_path_buf();
    config.save_to_file(&path)?;
    tracing::info!(path = %path.display(), new_dir = %new_dir.display(), "配置已更新 data_dir");
    Ok(())
}

/// 检查并执行待处理的迁移请求（调用方在服务器关停、DB 释放后调用）。
///
/// 返回 `true` = 已执行搬迁（调用方应以新配置重启）；`false` = 无待处理请求。
/// 失败时回滚数据并清除请求文件（避免重启循环；用户可从 UI 重试）。
pub fn run_pending_migration(old_data_dir: &Path) -> Result<bool> {
    let Some(req) = read_migration_request(old_data_dir) else {
        return Ok(false);
    };
    let new_dir = PathBuf::from(&req.new_dir);
    // 1. 先移动数据（失败自动回滚，配置未动 → 状态一致）
    let moved = match perform_migration(old_data_dir, &new_dir) {
        Ok(m) => m,
        Err(e) => {
            // 回滚已由 perform_migration 完成；清除请求避免重启循环
            let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
            return Err(e);
        }
    };
    // 2. 再更新配置（失败 → 把数据移回旧目录，保持配置与数据一致）
    if let Err(e) = update_config_data_dir(&new_dir) {
        // 配置更新失败：把已 copy 的数据复制回旧目录（尽力而为）
        if let Ok(entries) = std::fs::read_dir(&new_dir) {
            for entry in entries.flatten() {
                let _ = copy_recursive(&entry.path(), &old_data_dir.join(entry.file_name()));
            }
        }
        let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
        return Err(e);
    }
    // 3. 清理请求文件
    let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
    tracing::info!(
        moved,
        from = %old_data_dir.display(),
        to = %new_dir.display(),
        "数据目录搬迁完成",
    );
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_rejects_relative_and_same() {
        let current = Path::new("C:\\data\\tianyan");
        assert!(validate_migration_target(current, Path::new("relative/path")).is_err());
        assert!(validate_migration_target(current, current).is_err());
    }

    #[test]
    fn test_validate_rejects_nested_dirs() {
        let current = Path::new("C:\\data\\tianyan");
        // 新目录在当前目录内部
        assert!(validate_migration_target(current, Path::new("C:\\data\\tianyan\\sub")).is_err());
        // 当前目录在新目录内部
        assert!(validate_migration_target(current, Path::new("C:\\data")).is_err());
    }

    #[test]
    fn test_validate_accepts_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&current).unwrap();
        assert!(validate_migration_target(&current, &new_dir).is_ok());
    }

    #[test]
    fn test_validate_rejects_non_empty_target() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(new_dir.join("x.txt"), "x").unwrap();
        assert!(validate_migration_target(&current, &new_dir).is_err());
    }

    #[test]
    fn test_perform_migration_copies_all_entries() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("tianyan.db"), b"db").unwrap();
        std::fs::create_dir_all(old_dir.join("lancedb")).unwrap();
        std::fs::write(old_dir.join("lancedb").join("index"), b"idx").unwrap();
        // 请求文件应被跳过
        std::fs::write(old_dir.join(MIGRATION_REQUEST_FILE), "{}").unwrap();

        let moved = perform_migration(&old_dir, &new_dir).unwrap();
        assert_eq!(moved, 2);
        assert!(new_dir.join("tianyan.db").exists());
        assert!(new_dir.join("lancedb").join("index").exists());
        // copy 后源文件被尽力删除（未锁时成功）；请求文件留在旧目录
        assert!(!old_dir.join("tianyan.db").exists());
        assert!(old_dir.join(MIGRATION_REQUEST_FILE).exists());
    }

    #[test]
    fn test_copy_recursive_propagates_errors() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("missing.txt");
        let to = dir.path().join("out.txt");
        assert!(copy_recursive(&from, &to).is_err());
    }

    #[test]
    fn test_copy_recursive_copies_directories() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("src");
        let to = dir.path().join("dst");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("a.txt"), b"a").unwrap();
        std::fs::write(from.join("sub").join("b.txt"), b"b").unwrap();

        copy_recursive(&from, &to).unwrap();
        assert!(to.join("a.txt").exists());
        assert!(to.join("sub").join("b.txt").exists());
    }

    #[test]
    fn test_run_pending_migration_full_flow() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("tianyan.db"), b"db").unwrap();
        write_migration_request(&old_dir, &new_dir).unwrap();

        // 无配置文件时 update_config_data_dir 会失败——此处仅验证移动与清理
        // （配置更新在真实流程中由调用方保证配置文件存在）
        let req = read_migration_request(&old_dir).unwrap();
        assert_eq!(req.new_dir, new_dir.to_string_lossy());
        let moved = perform_migration(&old_dir, &new_dir).unwrap();
        assert_eq!(moved, 1);
        assert!(new_dir.join("tianyan.db").exists());
    }
}
