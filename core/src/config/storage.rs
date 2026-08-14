//! 存储配置模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// VFS 结构化存储后端类型（ADR-005）。
///
/// 默认使用本地文件系统；配置 `backend = "sqlite"` 切换为 SQLite。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackendType {
    /// 本地文件系统后端（默认）。
    #[default]
    Local,
    /// SQLite 后端（替代 LocalFileBackend，需共享 SqliteDb 连接）。
    Sqlite,
}

/// 存储配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// 数据存储的根目录。
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// 结构化存储后端（local | sqlite，默认 local）。
    #[serde(default)]
    pub backend: StorageBackendType,
    /// SQLite 数据库文件路径（backend = "sqlite" 时使用；缺省为 data_dir/tianyan.db）。
    #[serde(default)]
    pub sqlite_path: Option<PathBuf>,
    /// 最大存储大小（字节，0 = 无限制）。
    #[serde(default)]
    pub max_storage_size: u64,
    /// 启用旧数据自动清理。
    #[serde(default = "default_true")]
    pub auto_cleanup: bool,
    /// 清理前数据保留天数。
    #[serde(default = "default_cleanup_days")]
    pub cleanup_days: u32,
    /// 向量存储配置（嵌入式向量存储）。
    #[serde(default)]
    pub vector: VectorStorageConfig,
}

/// 向量存储配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStorageConfig {
    /// 向量集合/表名。
    #[serde(default = "default_collection_name")]
    pub collection_name: String,
    /// 向量维度。
    #[serde(default = "default_vector_dimension")]
    pub vector_dimension: usize,
}

fn default_data_dir() -> PathBuf {
    // README/.env.example 声明的 TIANYAN_DATA_DIR 覆盖（空值视为未设置）
    if let Ok(dir) = std::env::var("TIANYAN_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tianyan")
}

fn default_true() -> bool {
    true
}

fn default_cleanup_days() -> u32 {
    365
}

fn default_collection_name() -> String {
    "tianyan_data".to_string()
}

fn default_vector_dimension() -> usize {
    1536
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            backend: StorageBackendType::default(),
            sqlite_path: None,
            max_storage_size: 0,
            auto_cleanup: true,
            cleanup_days: 365,
            vector: VectorStorageConfig::default(),
        }
    }
}

impl Default for VectorStorageConfig {
    fn default() -> Self {
        Self {
            collection_name: default_collection_name(),
            vector_dimension: default_vector_dimension(),
        }
    }
}

impl StorageConfig {
    /// 验证存储配置。
    pub fn validate(&self) -> Result<(), String> {
        if self.cleanup_days == 0 {
            return Err("cleanup_days 必须大于 0".to_string());
        }

        if self.vector.vector_dimension == 0 {
            return Err("vector_dimension 必须大于 0".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_storage_config() {
        let config = StorageConfig::default();
        assert!(!config.data_dir.as_os_str().is_empty());
        assert_eq!(config.cleanup_days, 365);
        assert!(config.auto_cleanup);
        assert_eq!(config.vector.collection_name, "tianyan_data");
        assert_eq!(config.vector.vector_dimension, 1536);
    }

    /// 串行化 TIANYAN_DATA_DIR 环境变量测试（进程级全局副作用）。
    static ENV_DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_default_data_dir_honors_tianyan_data_dir_env() {
        // README/.env.example 声明的 TIANYAN_DATA_DIR 覆盖：默认数据目录应取环境变量
        let _guard = ENV_DATA_DIR_LOCK.lock().unwrap();
        let original = std::env::var("TIANYAN_DATA_DIR").ok();
        std::env::set_var("TIANYAN_DATA_DIR", "C:\\tianyan-e2e-data");

        let config = StorageConfig::default();
        let restored = || match &original {
            Some(v) => std::env::set_var("TIANYAN_DATA_DIR", v),
            None => std::env::remove_var("TIANYAN_DATA_DIR"),
        };
        assert_eq!(
            config.data_dir,
            std::path::PathBuf::from("C:\\tianyan-e2e-data"),
            "TIANYAN_DATA_DIR 应覆盖默认数据目录"
        );
        restored();
    }

    #[test]
    fn test_default_data_dir_ignores_empty_env() {
        let _guard = ENV_DATA_DIR_LOCK.lock().unwrap();
        let original = std::env::var("TIANYAN_DATA_DIR").ok();
        let normal = {
            std::env::remove_var("TIANYAN_DATA_DIR");
            StorageConfig::default().data_dir.clone()
        };
        std::env::set_var("TIANYAN_DATA_DIR", "");
        let with_empty = StorageConfig::default().data_dir.clone();
        match &original {
            Some(v) => std::env::set_var("TIANYAN_DATA_DIR", v),
            None => std::env::remove_var("TIANYAN_DATA_DIR"),
        }
        assert_eq!(with_empty, normal, "空 TIANYAN_DATA_DIR 应回落到默认目录");
    }

    #[test]
    fn test_storage_validation() {
        let mut config = StorageConfig::default();
        assert!(config.validate().is_ok());

        config.cleanup_days = 0;
        assert!(config.validate().is_err());

        config.cleanup_days = 365;
        config.vector.vector_dimension = 0;
        assert!(config.validate().is_err());
    }
}
