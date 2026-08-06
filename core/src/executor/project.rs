//! 项目格式探测注册表（verify_build / discover_tests 共享）。
//!
//! 从起始目录向上探测项目格式（最近 marker 优先），并为每种格式提供
//! 对应的验证命令模板。仅 Cargo 项目产生 JSON 结构化诊断
//! （`cargo check --message-format=json-render-diagnostics`）；
//! TypeScript / Python 依赖退出码 + 错误文本语义。

use std::path::{Path, PathBuf};

use crate::common::error::TianyanError;

/// 项目格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFormat {
    /// Rust/Cargo 项目（Cargo.toml）。
    Cargo,
    /// TypeScript 项目（tsconfig.json）。
    TypeScript,
    /// Python 项目（pyproject.toml）。
    Python,
    /// 无法识别。
    Unknown,
}

/// 项目探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    /// 项目格式。
    pub format: ProjectFormat,
    /// 包含标记文件的目录。
    pub root: PathBuf,
    /// 标记文件名（"Cargo.toml" | "tsconfig.json" | "pyproject.toml"；Unknown 时为空字符串）。
    pub marker: String,
}

/// 标记文件 → 格式的优先级表（同一目录出现多个标记时 Cargo > Python > TypeScript）。
const MARKER_PRIORITY: &[(&str, ProjectFormat)] = &[
    ("Cargo.toml", ProjectFormat::Cargo),
    ("pyproject.toml", ProjectFormat::Python),
    ("tsconfig.json", ProjectFormat::TypeScript),
];

/// 从 `start_dir` 向上逐级探测项目格式（最近 marker 优先）。
///
/// 每一级按 [`MARKER_PRIORITY`] 顺序检查标记文件，命中即返回；
/// 一路到文件系统根仍未命中则返回 [`ProjectFormat::Unknown`]。
pub fn probe_project(start_dir: &Path) -> Result<ProjectInfo, TianyanError> {
    let mut dir = Some(start_dir);
    while let Some(current) = dir {
        for (marker, format) in MARKER_PRIORITY {
            if current.join(marker).is_file() {
                return Ok(ProjectInfo {
                    format: *format,
                    root: current.to_path_buf(),
                    marker: (*marker).to_string(),
                });
            }
        }
        dir = current.parent();
    }
    Ok(ProjectInfo {
        format: ProjectFormat::Unknown,
        root: start_dir.to_path_buf(),
        marker: String::new(),
    })
}

/// 项目格式对应的验证命令模板。
///
/// - Cargo：`cargo check --message-format=json-render-diagnostics`（产生 JSON 结构化诊断）
/// - TypeScript：`tsc --noEmit`（退出码 + 错误文本）
/// - Python：`pytest --tb=short`（退出码 + 错误文本）
/// - Unknown：`None`
pub fn verification_command(info: &ProjectInfo) -> Option<Vec<String>> {
    match info.format {
        ProjectFormat::Cargo => Some(vec![
            "cargo".to_string(),
            "check".to_string(),
            "--message-format=json-render-diagnostics".to_string(),
        ]),
        ProjectFormat::TypeScript => Some(vec!["tsc".to_string(), "--noEmit".to_string()]),
        ProjectFormat::Python => Some(vec!["pytest".to_string(), "--tb=short".to_string()]),
        ProjectFormat::Unknown => None,
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
