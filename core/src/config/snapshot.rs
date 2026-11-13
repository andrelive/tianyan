//! 工作区快照配置（`[snapshot]` 节）。
//!
//! 为什么要有这节：快照的两个边界此前是**硬编码常量**——`MAX_FILE_BYTES = 2 MiB`
//! 与空 `excludes`。前者让「大于 2 MB 的文件永远不进快照、回退也不恢复它」成为一条
//! 不可调的规则（大 JSON、数据集、模型文件都落在外面），后者让用户除改项目
//! `.gitignore` 之外没有别的排除入口。这两条都属于「默认合理、但必须可调」。

use serde::{Deserialize, Serialize};

/// 默认的单文件纳入上限（2 MiB，与 opencode 一致）。
///
/// 再大就不进快照：捕获要读全文 + gzip 写对象，回退要写回全文，代价随体积线性上升；
/// 而超大文件通常也不是「AI 改坏的」那一类内容。
pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 工作区快照配置（`[snapshot]` 节）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// 纳入快照的单文件大小上限（字节，默认 2 MiB）。
    ///
    /// 超过它的文件**不参与回退**（既不恢复也不删除）：捕获时跳过、并记入树的
    /// `skipped` 属主，因此回退时也不会被误删。调大能覆盖大 JSON / 数据集，代价是
    /// 快照体积与捕获耗时同步上升。
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    /// 额外排除的目录/文件名（大小写不敏感，**按名字精确匹配**，不支持通配）。
    ///
    /// 与内置排除（`.git` / `node_modules` / `target`）和 `.gitignore` 叠加生效。
    /// 只影响快照，不影响工具读写。
    #[serde(default)]
    pub excludes: Vec<String>,
}

/// `max_file_bytes` 的 serde 默认值。
fn default_max_file_bytes() -> u64 {
    DEFAULT_MAX_FILE_BYTES
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            excludes: Vec::new(),
        }
    }
}

impl SnapshotConfig {
    /// 生效的单文件上限（`0` 按 1 字节兜底，避免「配了 0 就什么都不进快照」这种
    /// 静默失效——真要排除某类文件应该用 `excludes`）。
    pub fn effective_max_file_bytes(&self) -> u64 {
        self.max_file_bytes.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_matches_legacy_constant() {
        let cfg = SnapshotConfig::default();
        assert_eq!(cfg.max_file_bytes, 2 * 1024 * 1024, "默认值应等于旧常量");
        assert!(cfg.excludes.is_empty());
    }

    #[test]
    fn test_effective_max_file_bytes_guards_zero() {
        let cfg = SnapshotConfig {
            max_file_bytes: 0,
            excludes: Vec::new(),
        };
        assert_eq!(cfg.effective_max_file_bytes(), 1, "0 应兜底为 1");
    }

    #[test]
    fn test_config_roundtrip_missing_fields_use_defaults() {
        // 旧配置文件没有 `[snapshot]` 节 / 只写了一个字段都要能加载
        let empty: SnapshotConfig = toml::from_str("").unwrap();
        assert_eq!(empty.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
        let partial: SnapshotConfig = toml::from_str("excludes = [\"fixtures\"]").unwrap();
        assert_eq!(partial.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
        assert_eq!(partial.excludes, vec!["fixtures".to_string()]);
    }
}
