//! Tianyan 代理系统的配置管理。
//!
//! 本模块提供整个应用程序的配置结构和加载机制。
//! 使用 once_cell 实现全局静态配置访问。

use serde::{Deserialize, Serialize};

mod agent;
mod logging;
mod memory;
mod model;
mod retrieval;
mod security;
mod storage;
pub mod validation;
pub mod wizard;

pub use agent::AgentConfig;
pub use logging::LoggingConfig;
pub use memory::MemoryConfig;
pub use model::{ModelServiceConfig, ModelServiceType, ModelsConfig};
pub use retrieval::RetrievalConfig;
pub use security::SecurityConfig;
pub use storage::{StorageConfig, VectorStorageConfig};
pub use validation::{
    validate_agent_config, validate_model_service, validate_models_config, validate_storage_config,
    validation_errors_to_strings, ConfigValidationError, ValidationResult,
};
pub use wizard::{
    ConfigStatus, TestConnectionRequest, TestConnectionResponse, WizardAgentConfig, WizardConfig,
    WizardModelService, WizardModelsConfig, WizardStorageConfig, WizardVectorStorageConfig,
};

/// Tianyan 代理的主配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TianyanConfig {
    /// 智能体配置。
    #[serde(default)]
    pub agent: AgentConfig,
    /// 存储配置。
    #[serde(default)]
    pub storage: StorageConfig,
    /// 模型配置。
    #[serde(default)]
    pub models: ModelsConfig,
    /// 日志配置。
    #[serde(default)]
    pub logging: LoggingConfig,
    /// 安全配置。
    #[serde(default)]
    pub security: SecurityConfig,
    /// 记忆配置。
    #[serde(default)]
    pub memory: MemoryConfig,
    /// 检索配置。
    #[serde(default)]
    pub retrieval: RetrievalConfig,
}

impl TianyanConfig {
    /// 配置文件名。
    const CONFIG_FILE_NAME: &'static str = "tianyan.toml";

    /// 检查配置状态。
    ///
    /// 返回配置状态，包括是否已配置、配置文件路径和错误列表。
    pub fn check_config_status() -> ConfigStatus {
        // 查找配置文件
        let config_path = Self::find_config_file();

        let Some(path) = config_path else {
            return ConfigStatus::not_configured(vec!["未找到配置文件".to_string()]);
        };

        // 尝试加载配置
        match Self::load_from_file(&path) {
            Ok(config) => {
                // 验证配置
                match config.validate() {
                    Ok(()) => ConfigStatus::configured(path),
                    Err(e) => ConfigStatus::invalid(path, vec![e]),
                }
            }
            Err(e) => ConfigStatus::invalid(path, vec![e]),
        }
    }

    /// 从默认位置加载配置。
    ///
    /// 搜索顺序：
    /// 1. 当前目录 `./tianyan.toml`
    /// 2. 配置目录 `~/.config/tianyan/tianyan.toml`
    /// 3. 主目录 `~/.tianyan/tianyan.toml`
    ///
    /// 如果找不到配置文件，返回错误。
    pub fn load() -> Result<Self, String> {
        let config_path = Self::find_config_file()
            .ok_or("未找到配置文件，请将 tianyan.toml 放置在当前目录、配置目录 (~/.config/tianyan) 或主目录 (~/.tianyan)")?;

        Self::load_from_file(&config_path)
    }

    /// 从指定文件加载配置。
    #[cfg_attr(test, allow(dead_code))]
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, String> {
        use config::{File, FileFormat};

        let config_raw = config::Config::builder()
            .add_source(File::from(path).format(FileFormat::Toml))
            .build()
            .map_err(|e| format!("构建配置失败：{}", e))?;

        let config: TianyanConfig = config_raw
            .try_deserialize()
            .map_err(|e| format!("反序列化配置失败：{}", e))?;

        config.validate()?;

        Ok(config)
    }

    /// 在默认位置查找配置文件。
    ///
    /// 搜索顺序：
    /// 1. 当前目录 `./tianyan.toml`
    /// 2. %APPDATA%/tianyan/tianyan.toml (Windows) 或 ~/.config/tianyan/tianyan.toml
    /// 3. ~/.tianyan/tianyan.toml
    pub fn find_config_file() -> Option<std::path::PathBuf> {
        // 1. 检查当前目录
        let current_dir = std::env::current_dir().ok()?;
        let config_in_current = current_dir.join(Self::CONFIG_FILE_NAME);
        if config_in_current.exists() {
            return Some(config_in_current);
        }

        // 2. 检查配置目录 (%APPDATA% 或 ~/.config)
        if let Some(config_dir) = dirs::config_dir() {
            // Windows: %APPDATA%/tianyan/tianyan.toml
            // Linux/macOS: ~/.config/tianyan/tianyan.toml
            let config_in_config = config_dir.join("tianyan").join(Self::CONFIG_FILE_NAME);
            if config_in_config.exists() {
                return Some(config_in_config);
            }
        }

        // 3. 检查主目录 (向后兼容)
        if let Some(home_dir) = dirs::home_dir() {
            let config_in_home = home_dir.join(".tianyan").join(Self::CONFIG_FILE_NAME);
            if config_in_home.exists() {
                return Some(config_in_home);
            }
        }

        None
    }

    /// 检查配置文件是否存在。
    pub fn config_exists() -> bool {
        Self::find_config_file().is_some()
    }

    /// 获取默认配置文件保存路径。
    ///
    /// 优先使用配置目录 (%APPDATA%/tianyan 或 ~/.config/tianyan)
    pub fn default_config_path() -> Option<std::path::PathBuf> {
        dirs::config_dir().map(|dir| dir.join("tianyan").join(Self::CONFIG_FILE_NAME))
    }

    /// 验证配置。
    pub fn validate(&self) -> Result<(), String> {
        self.agent.validate()?;
        self.storage.validate()?;
        self.models.validate()?;
        self.logging.validate()?;
        self.security.validate()?;
        self.memory.validate()?;
        self.retrieval.validate()?;
        Ok(())
    }

    /// 生成默认配置文件内容。
    pub fn generate_default_toml() -> Result<String, String> {
        toml::to_string_pretty(&Self::default()).map_err(|e| format!("序列化配置失败：{}", e))
    }

    /// 将配置保存到文件。
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{}", e))?;
        }

        let content = toml::to_string_pretty(self).map_err(|e| format!("序列化配置失败：{}", e))?;

        std::fs::write(path, content).map_err(|e| format!("写入文件失败: {}", e))?;

        Ok(())
    }
}

/// 全局静态配置实例。
///
/// 使用 once_cell::sync::OnceCell 实现延迟加载，配置只在首次访问时加载。
/// 与 Lazy 不同，OnceCell 允许加载失败时返回错误而非 panic。
static CONFIG: once_cell::sync::OnceCell<TianyanConfig> = once_cell::sync::OnceCell::new();

/// 获取全局配置的引用。
///
/// - returns: 全局配置的静态引用
///
/// # Errors
/// - `TianyanError::Config` - 配置文件未找到或格式无效
///
/// # 示例
///
/// ```rust,ignore
/// use tianyan::config::get_config;
///
/// fn example() -> tianyan::common::error::Result<()> {
///     let config = get_config()?;
///     println!("数据目录：{:?}", config.storage.data_dir);
///     Ok(())
/// }
/// ```
pub fn get_config() -> crate::common::error::Result<&'static TianyanConfig> {
    CONFIG.get_or_try_init(|| {
        TianyanConfig::load().map_err(crate::common::error::TianyanError::Config)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TianyanConfig::default();
        assert!(!config.storage.data_dir.as_os_str().is_empty());
        assert!(!config.models.default_chat_model.is_empty());
    }

    #[test]
    fn test_config_serialization() {
        let config = TianyanConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: TianyanConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            config.models.default_chat_model,
            parsed.models.default_chat_model
        );
    }

    #[test]
    fn test_config_validation() {
        let config = TianyanConfig::default();
        assert!(config.storage.validate().is_ok());
        assert!(config.logging.validate().is_ok());
        assert!(config.security.validate().is_ok());
        assert!(config.memory.validate().is_ok());
        assert!(config.retrieval.validate().is_ok());
    }

    #[test]
    fn test_generate_default_toml() {
        let toml = TianyanConfig::generate_default_toml().unwrap();
        assert!(toml.contains("[storage]"));
        assert!(toml.contains("[models]"));
    }

    #[test]
    fn test_load_without_config_file() {
        // 测试在没有配置文件的情况下，从指定路径加载会返回错误
        let non_existent_path = std::path::PathBuf::from("/non/existent/path/tianyan.toml");
        assert!(TianyanConfig::load_from_file(&non_existent_path).is_err());
    }
}
