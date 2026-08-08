//! Tianyan 代理系统的配置管理。
//!
//! 本模块提供整个应用程序的配置结构和加载机制。
//! 使用 std::sync::OnceLock 实现全局静态配置访问。

use serde::{Deserialize, Serialize};

use crate::common::error::TianyanError;

mod agent;
pub mod api_types;
pub mod mcp;
mod memory;
mod model;
mod retrieval;
mod security;
mod storage;
pub mod validation;
pub mod web;
pub mod wizard;

pub use crate::common::logging::LoggingConfig;
pub use agent::AgentConfig;
pub use mcp::{McpConfig, McpServerEntry};
pub use memory::MemoryConfig;
pub use model::{
    find_provider, ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig,
    ProviderConfig,
};
pub use retrieval::RetrievalConfig;
pub use security::{SafetyMode, SecurityConfig};
pub use storage::{StorageBackendType, StorageConfig, VectorStorageConfig};
pub use validation::{
    validate_agent_config, validate_models_config, validate_provider, validate_storage_config,
    validation_errors_to_strings, ValidationResult,
};
pub use web::WebConfig;
pub use wizard::{TestConnectionRequest, TestConnectionResponse};

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
    /// MCP 服务器配置。
    #[serde(default)]
    pub mcp: McpConfig,
    /// Web 工具配置（web_search / web_fetch）。
    #[serde(default)]
    pub web: WebConfig,
}

impl TianyanConfig {
    /// 配置文件名。
    const CONFIG_FILE_NAME: &'static str = "tianyan.toml";

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
        self.mcp.validate().map_err(|e| e.to_string())?;
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
/// 使用 `std::sync::OnceLock` 实现延迟加载，配置只在首次访问时加载。
/// 通过 `get_or_init` 保证线程安全的单次初始化，消除 TOCTOU 竞态条件。
static CONFIG: std::sync::OnceLock<TianyanConfig> = std::sync::OnceLock::new();

/// 全局配置加载错误（仅首次加载失败时写入）。
///
/// 与 [`CONFIG`] 同生命周期：进程内只记录首次加载的结果，不可重置；
/// 加载成功时保持为空。调用方通过 [`last_config_error`] 查询。
static CONFIG_LOAD_ERROR: std::sync::OnceLock<TianyanError> = std::sync::OnceLock::new();

/// 加载配置；失败时构造带路径上下文的错误并回退到默认配置。
///
/// 返回 `(配置, 错误)`：加载成功时错误为 `None`，失败时错误为 `Some`。
/// 独立函数便于测试直接验证失败路径（`CONFIG` 为进程级单例，无法重置）。
fn load_config_or_default() -> (TianyanConfig, Option<TianyanError>) {
    match TianyanConfig::load() {
        Ok(config) => (config, None),
        Err(detail) => {
            let path =
                TianyanConfig::find_config_file().or_else(TianyanConfig::default_config_path);
            let err = config_load_error(path.as_deref(), detail);
            (TianyanConfig::default(), Some(err))
        }
    }
}

/// 构造配置加载错误（带路径上下文）。
///
/// `path` 为实际失败路径（文件存在但损坏）或预期配置路径（文件缺失）。
fn config_load_error(path: Option<&std::path::Path>, detail: String) -> TianyanError {
    let location = match path {
        Some(p) => p.display().to_string(),
        None => "默认搜索路径".to_string(),
    };
    TianyanError::Custom(format!("config 加载失败（路径：{location}）：{detail}"))
}

/// 获取全局配置的引用。
///
/// 使用 `OnceLock::get_or_init` 保证线程安全的延迟初始化，
/// 消除 TOCTOU 竞态条件。
///
/// 配置加载失败时**不会** panic：记录 `error` 级别日志（含配置路径与错误详情）
/// 并回退到默认配置，错误可通过 [`last_config_error`] 查询。
///
/// # 示例
///
/// ```rust,ignore
/// use tianyan::config::get_config;
///
/// fn example() {
///     let config = get_config();
///     println!("数据目录：{:?}", config.storage.data_dir);
/// }
/// ```
pub fn get_config() -> &'static TianyanConfig {
    CONFIG.get_or_init(|| {
        let (config, load_error) = load_config_or_default();
        if let Some(error) = load_error {
            tracing::error!(error = %error, "加载配置文件失败，使用默认配置");
            let _ = CONFIG_LOAD_ERROR.set(error);
        }
        config
    })
}

/// 查询全局配置加载错误。
///
/// 若 [`get_config`] 首次调用时配置加载失败（文件缺失或内容损坏），
/// 返回 `Some`，错误消息包含配置路径与失败详情；否则返回 `None`。
///
/// 用于诊断"配置损坏却静默回退默认配置"的场景：
/// 调用方（如 server 诊断接口）可据此向用户暴露配置异常。
pub fn last_config_error() -> Option<&'static TianyanError> {
    CONFIG_LOAD_ERROR.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TianyanConfig::default();
        assert!(!config.storage.data_dir.as_os_str().is_empty());
    }

    #[test]
    fn test_config_serialization() {
        let config = TianyanConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: TianyanConfig = toml::from_str(&toml_str).unwrap();
        // 默认配置 providers 和 preferences 都为空
        assert!(parsed.models.providers.is_empty());
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

    #[test]
    fn test_config_load_error_contains_path_and_detail() {
        // 纯函数验证 get_config 失败路径记录的错误格式：
        // 模块前缀 + 配置路径 + 错误详情
        let bad_path = std::path::Path::new("/tmp/bad-config/tianyan.toml");
        let err = config_load_error(Some(bad_path), "反序列化配置失败：syntax".to_string());
        let msg = err.to_string();
        assert!(msg.contains("config 加载失败"), "应含模块前缀：{msg}");
        assert!(
            msg.contains("/tmp/bad-config/tianyan.toml"),
            "应含配置路径：{msg}"
        );
        assert!(msg.contains("反序列化配置失败"), "应含错误详情：{msg}");

        // 路径未知时给出"默认搜索路径"占位
        let err = config_load_error(None, "未找到配置文件".to_string());
        assert!(err.to_string().contains("默认搜索路径"));
    }

    #[test]
    fn test_load_invalid_toml_fails() {
        // 损坏配置（非法 TOML）→ load_from_file 返回错误，
        // 这是 get_config 记录错误并回退默认配置的触发源
        let dir = std::env::temp_dir().join(format!(
            "tianyan-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tianyan.toml");
        std::fs::write(&path, "this is not [valid toml = [").unwrap();

        let result = TianyanConfig::load_from_file(&path);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err(), "非法 TOML 应加载失败");
    }

    #[test]
    fn test_get_config_once_semantics_and_error_query() {
        // CONFIG / CONFIG_LOAD_ERROR 为进程级单例且不可重置（OnceLock），
        // 本测试是 lib 测试二进制中唯一调用 get_config() 的测试，
        // 因此此处观察到的即"首次调用"行为。
        let config = get_config();

        // 重复调用返回同一实例（OnceLock 单次初始化语义）
        assert!(std::ptr::eq(config, get_config()));

        // 若首次加载失败：错误可查询，且消息含模块前缀与路径上下文
        if let Some(err) = last_config_error() {
            let msg = err.to_string();
            assert!(msg.contains("config 加载失败"), "应含模块前缀：{msg}");
            assert!(msg.contains("路径"), "应含路径上下文：{msg}");
        }
    }
}
