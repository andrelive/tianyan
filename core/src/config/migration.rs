//! 数据目录搬迁（0.2.3 重做：真"搬"，配置目录与数据目录分离，ADR-023）。
//!
//! 流程（关 DB → 复制数据 → 校验 → 更新配置 → 删除源 → 重启）：
//! 1. API 校验新目录并写迁移请求文件 `{data_dir}/.tianyan-migrate.json`；
//! 2. API 触发服务器优雅关停（watch 信号）——SQLite/LanceDB 随 AppState
//!    drop 释放文件锁（Windows 上移动打开的文件会失败）；
//! 3. 服务器停止后，监督循环（Tauri）/主循环（独立 server）调用
//!    [`run_pending_migration`]:
//!    - **排除清单**：配置文件 `tianyan.toml`（固定于 `~/.tianyan/`——
//!      即使历史安装把它放在数据目录里，也留在原地不搬）与迁移请求文件；
//!    - 复制 → 逐文件校验（相对路径集合 + 字节大小与源一致）；
//!    - 更新配置 `storage.data_dir`（失败 → 删除已复制内容，源目录未动）；
//!    - **强制删除**源数据条目（进入本步后不再回滚——新目录数据已校验
//!      完整、配置已指向新目录；个别文件被占用 → 留作无害残留并在结果
//!      中报告，绝不留下"两份全量数据"或"半份源数据"的静默状态）；
//! 4. 以新配置重启（监督循环/主循环重读配置文件）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::common::error::{Result, TianyanError};
use crate::config::TianyanConfig;

/// 迁移请求文件名（位于旧 data_dir 内）。
pub const MIGRATION_REQUEST_FILE: &str = ".tianyan-migrate.json";

/// 搬迁时忽略的文件：配置文件（固定于 `~/.tianyan/`，ADR-023）。
const CONFIG_FILE_NAME: &str = "tianyan.toml";

/// 迁移请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRequest {
    /// 目标数据目录（绝对路径）。
    pub new_dir: String,
    /// 请求时间（epoch 秒）。
    pub requested_at: i64,
}

/// 搬迁结果。
#[derive(Debug, Default, Clone)]
pub struct MigrationOutcome {
    /// 是否执行了搬迁（false = 无待处理请求）。
    pub migrated: bool,
    /// 因被占用未能删除、留在旧目录的残留条目。搬迁本身已成功：
    /// 数据已完整位于新目录、配置已指向新目录，残留只是无害副本。
    pub leftovers: Vec<PathBuf>,
}

/// 根条目是否在搬迁排除清单内（配置文件 + 迁移请求文件）。
fn is_excluded(name: &std::ffi::OsStr) -> bool {
    name == MIGRATION_REQUEST_FILE || name == CONFIG_FILE_NAME
}

/// 校验迁移目标目录（绝对路径 / 非当前目录 / 非父子关系 / 可写 / 为空）。
pub fn validate_migration_target(
    current: &Path,
    new_dir: &Path,
) -> std::result::Result<(), String> {
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
    std::fs::create_dir_all(new_dir).map_err(|e| format!("无法创建新数据目录：{e}"))?;
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

/// 复制 old_dir 全部条目到 new_dir（排除配置文件与迁移请求文件）并逐文件校验。
///
/// 不修改源目录——删除由 [`delete_source_entries`] 在配置更新成功后单独执行。
/// 返回复制的顶层条目数。
pub fn perform_migration(old_dir: &Path, new_dir: &Path) -> Result<usize> {
    validate_migration_target(old_dir, new_dir).map_err(TianyanError::config)?;
    std::fs::create_dir_all(new_dir)
        .map_err(|e| TianyanError::Custom(format!("migration: 创建目标目录失败：{e}")))?;
    let entries = std::fs::read_dir(old_dir)
        .map_err(|e| TianyanError::Custom(format!("migration: 读取数据目录失败：{e}")))?;
    let mut copied = 0usize;
    for entry in entries {
        let entry =
            entry.map_err(|e| TianyanError::Custom(format!("migration: 读取目录条目失败：{e}")))?;
        let name = entry.file_name();
        if is_excluded(&name) {
            continue;
        }
        let to = new_dir.join(&name);
        if let Err(e) = copy_recursive(&entry.path(), &to) {
            // 回滚：删除已 copy 的目标
            cleanup_partial_copy(new_dir);
            return Err(TianyanError::Custom(format!(
                "migration: 复制 {} 失败：{e}（已回滚）",
                entry.path().display()
            )));
        }
        copied += 1;
    }
    verify_copy(old_dir, new_dir)?;
    Ok(copied)
}

/// 清理复制中断留下的不完整目标目录内容（保留目录本身）。
fn cleanup_partial_copy(new_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(new_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path())
                .or_else(|_| std::fs::remove_file(entry.path()));
        }
    }
}

/// 递归收集 (相对路径, 字节大小)；根级排除清单条目跳过。
fn collect_files(
    dir: &Path,
    rel: &Path,
    is_root: bool,
    out: &mut Vec<(PathBuf, u64)>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        TianyanError::Custom(format!("migration: 校验读取 {} 失败：{e}", dir.display()))
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|e| TianyanError::Custom(format!("migration: 校验读取条目失败：{e}")))?;
        let name = entry.file_name();
        if is_root && is_excluded(&name) {
            continue;
        }
        let child_rel = rel.join(&name);
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &child_rel, false, out)?;
        } else {
            let size = entry
                .metadata()
                .map_err(|e| TianyanError::Custom(format!("migration: 校验读取元数据失败：{e}")))?
                .len();
            out.push((child_rel, size));
        }
    }
    Ok(())
}

/// 逐文件校验复制结果：相对路径集合一致 + 字节大小一致。
fn verify_copy(old_dir: &Path, new_dir: &Path) -> Result<()> {
    let mut old_files = Vec::new();
    let mut new_files = Vec::new();
    collect_files(old_dir, Path::new(""), true, &mut old_files)?;
    collect_files(new_dir, Path::new(""), true, &mut new_files)?;
    old_files.sort();
    new_files.sort();
    if old_files != new_files {
        return Err(TianyanError::config(
            "migration: 校验失败：源与目标的文件清单或大小不一致",
        ));
    }
    Ok(())
}

/// 删除源数据条目（排除配置文件与迁移请求文件）。
///
/// 仅在「复制已校验 + 配置已指向新目录」之后调用——删除失败不再回滚，
/// 失败条目（被占用等）作为无害残留返回，由调用方记录/报告。
pub fn delete_source_entries(old_dir: &Path) -> Vec<PathBuf> {
    let mut leftovers = Vec::new();
    let Ok(entries) = std::fs::read_dir(old_dir) else {
        return leftovers;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if is_excluded(&name) {
            continue;
        }
        let from = entry.path();
        let removed = if from.is_dir() {
            std::fs::remove_dir_all(&from)
        } else {
            std::fs::remove_file(&from)
        };
        if let Err(e) = removed {
            tracing::warn!(
                path = %from.display(),
                error = %e,
                "搬迁：源条目删除失败（被占用？），留作无害残留"
            );
            leftovers.push(from);
        }
    }
    leftovers
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
///
/// 使用生效配置的解析路径（环境变量覆盖优先，否则固定 `~/.tianyan/tianyan.toml`）。
pub fn update_config_data_dir(new_dir: &Path) -> Result<()> {
    let path = TianyanConfig::find_config_file()
        .or_else(TianyanConfig::default_config_path)
        .ok_or_else(|| TianyanError::config("migration: 未找到配置文件"))?;
    update_config_data_dir_at(&path, new_dir)
}

/// 写入 data_dir 到指定配置文件（测试注入配置路径用）。
pub fn update_config_data_dir_at(config_path: &Path, new_dir: &Path) -> Result<()> {
    let mut config = TianyanConfig::load_from_file(config_path)?;
    config.storage.data_dir = new_dir.to_path_buf();
    config.save_to_file(config_path)?;
    tracing::info!(path = %config_path.display(), new_dir = %new_dir.display(), "配置已更新 data_dir");
    Ok(())
}

/// 检查并执行待处理的迁移请求（调用方在服务器关停、DB 释放后调用）。
///
/// 返回 `migrated = true` 表示已搬迁（调用方应以新配置重启）；
/// `migrated = false` 表示无待处理请求。
/// 失败时清理已复制内容并清除请求文件（避免重启循环；用户可从 UI 重试），
/// 配置文件全程不被移动——即使搬迁失败也绝无丢失风险。
pub fn run_pending_migration(old_data_dir: &Path) -> Result<MigrationOutcome> {
    let config_path = TianyanConfig::find_config_file()
        .or_else(TianyanConfig::default_config_path)
        .ok_or_else(|| TianyanError::config("migration: 未找到配置文件"))?;
    run_pending_migration_at(old_data_dir, &config_path)
}

/// 指定配置文件路径的搬迁执行（生产经 [`run_pending_migration`] 解析配置路径）。
pub fn run_pending_migration_at(
    old_data_dir: &Path,
    config_path: &Path,
) -> Result<MigrationOutcome> {
    let Some(req) = read_migration_request(old_data_dir) else {
        return Ok(MigrationOutcome::default());
    };
    let new_dir = PathBuf::from(&req.new_dir);

    // 1. 复制 + 校验（纯新增操作，源目录未动；失败 → 清理不完整副本）
    if let Err(e) = perform_migration(old_data_dir, &new_dir) {
        cleanup_partial_copy(&new_dir);
        let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
        return Err(e);
    }

    // 2. 更新配置（失败 → 删除已复制内容；源目录未动，配置保持指向旧目录）
    if let Err(e) = update_config_data_dir_at(config_path, &new_dir) {
        cleanup_partial_copy(&new_dir);
        let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
        return Err(e);
    }

    // 3. 强制删除源数据条目（进入本步不回滚：新目录已校验完整、配置已指向
    //    新目录；个别被占用文件留作无害残留并报告）
    let leftovers = delete_source_entries(old_data_dir);

    // 4. 清理请求文件
    let _ = std::fs::remove_file(old_data_dir.join(MIGRATION_REQUEST_FILE));
    tracing::info!(
        from = %old_data_dir.display(),
        to = %new_dir.display(),
        leftovers = leftovers.len(),
        "数据目录搬迁完成",
    );
    Ok(MigrationOutcome {
        migrated: true,
        leftovers,
    })
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
        assert!(validate_migration_target(current, Path::new("C:\\data\\tianyan\\sub")).is_err());
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
    fn test_perform_migration_excludes_config_and_keeps_source() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("tianyan.db"), b"db").unwrap();
        std::fs::create_dir_all(old_dir.join("lancedb")).unwrap();
        std::fs::write(old_dir.join("lancedb").join("index"), b"idx").unwrap();
        std::fs::write(old_dir.join("tianyan.toml"), b"config").unwrap();
        std::fs::write(old_dir.join(MIGRATION_REQUEST_FILE), "{}").unwrap();

        let moved = perform_migration(&old_dir, &new_dir).unwrap();
        assert_eq!(moved, 2);
        assert!(new_dir.join("tianyan.db").exists());
        assert!(new_dir.join("lancedb").join("index").exists());
        assert!(!new_dir.join("tianyan.toml").exists(), "配置文件应被排除");
        assert!(!new_dir.join(MIGRATION_REQUEST_FILE).exists());
        assert!(
            old_dir.join("tianyan.db").exists(),
            "复制阶段源目录不应被改动"
        );
        assert!(old_dir.join("tianyan.toml").exists());
    }

    #[test]
    fn test_verify_copy_detects_incomplete_copy() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("a.txt"), b"aaaa").unwrap();
        std::fs::write(old_dir.join("b.txt"), b"b").unwrap();
        perform_migration(&old_dir, &new_dir).unwrap();

        // 目标丢失文件 → 校验失败
        std::fs::remove_file(new_dir.join("b.txt")).unwrap();
        assert!(verify_copy(&old_dir, &new_dir).is_err());

        // 大小不一致 → 校验失败
        std::fs::write(new_dir.join("b.txt"), b"b").unwrap();
        std::fs::write(new_dir.join("a.txt"), b"a").unwrap();
        assert!(verify_copy(&old_dir, &new_dir).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_delete_source_entries_reports_locked_leftovers() {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("a.txt"), b"a").unwrap();
        std::fs::write(old_dir.join("tianyan.toml"), b"config").unwrap();
        // 独占打开（不共享）→ 删除必然失败
        let mut locked = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(old_dir.join("locked.db"))
            .unwrap();
        locked.write_all(b"locked").unwrap();

        let leftovers = delete_source_entries(&old_dir);

        assert_eq!(leftovers, vec![old_dir.join("locked.db")]);
        assert!(!old_dir.join("a.txt").exists(), "未锁定文件应被删除");
        assert!(old_dir.join("tianyan.toml").exists(), "配置文件应被排除");
        assert!(old_dir.join("locked.db").exists(), "被占用文件应残留");
        drop(locked);
    }

    #[cfg(not(windows))]
    #[test]
    fn test_delete_source_entries_excludes_config() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("a.txt"), b"a").unwrap();
        std::fs::write(old_dir.join("tianyan.toml"), b"config").unwrap();

        let leftovers = delete_source_entries(&old_dir);

        assert!(leftovers.is_empty());
        assert!(!old_dir.join("a.txt").exists());
        assert!(old_dir.join("tianyan.toml").exists(), "配置文件应被排除");
    }

    #[test]
    fn test_run_pending_migration_full_flow() {
        // 真实场景（0.2.2 用户报告的形态）：配置文件位于数据目录内
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("tianyan.db"), b"db").unwrap();
        let config_path = old_dir.join("tianyan.toml");
        // 与真实配置同构：storage + 至少一个模型 provider（load_from_file 会校验）
        std::fs::write(
            &config_path,
            "[storage]\ndata_dir = 'OLD'\n\n[[models.providers]]\nname = 'mock'\nendpoint = 'http://localhost:1/v1'\n\n[[models.providers.models]]\nname = 'test-model'\ncapabilities = ['chat']\n",
        )
        .unwrap();
        write_migration_request(&old_dir, &new_dir).unwrap();

        let outcome = run_pending_migration_at(&old_dir, &config_path).unwrap();

        assert!(outcome.migrated);
        assert!(outcome.leftovers.is_empty());
        assert!(new_dir.join("tianyan.db").exists(), "数据应搬至新目录");
        assert!(
            !new_dir.join("tianyan.toml").exists(),
            "配置文件不应被复制到新目录"
        );
        assert!(old_dir.join("tianyan.toml").exists(), "配置文件应留在原地");
        assert!(!old_dir.join("tianyan.db").exists(), "源数据应被删除");
        assert!(
            !old_dir.join(MIGRATION_REQUEST_FILE).exists(),
            "请求文件应清理"
        );
        let updated = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            updated.contains(new_dir.to_string_lossy().as_ref()),
            "配置应指向新目录：{updated}"
        );
    }

    #[test]
    fn test_run_pending_migration_config_failure_cleans_copy() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("old");
        let new_dir = dir.path().join("new");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("tianyan.db"), b"db").unwrap();
        let missing_config = dir.path().join("missing.toml");
        write_migration_request(&old_dir, &new_dir).unwrap();

        let result = run_pending_migration_at(&old_dir, &missing_config);

        assert!(result.is_err());
        let empty = std::fs::read_dir(&new_dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
        assert!(empty, "失败的搬迁应清理已复制内容");
        assert!(old_dir.join("tianyan.db").exists(), "源目录应保持原样");
        assert!(
            !old_dir.join(MIGRATION_REQUEST_FILE).exists(),
            "请求文件应清理（避免重启循环）"
        );
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
}
