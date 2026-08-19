//! Tianyan 代理系统的配置管理。
//!
//! 本模块提供整个应用程序的配置结构和加载机制。
//! 使用 std::sync::OnceLock 实现全局静态配置访问。

use serde::{Deserialize, Serialize};

use crate::common::error::TianyanError;

mod agent;
pub mod api_types;
mod clipboard;
mod events;
mod evolution;
pub mod mcp;
mod memory;
mod model;
mod reminder;
mod retrieval;
mod roles;
mod security;
mod storage;
pub mod validation;
pub mod web;
pub mod wizard;

pub use crate::common::logging::LoggingConfig;
pub use agent::AgentConfig;
pub use clipboard::ClipboardConfig;
pub use events::EventsConfig;
pub use evolution::EvolutionConfig;
pub use mcp::{McpConfig, McpServerEntry};
pub use memory::MemoryConfig;
pub use model::{
    find_provider, ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig,
    ProviderConfig,
};
pub use reminder::ReminderConfig;
pub use retrieval::RetrievalConfig;
pub use roles::AgentRolesConfig;
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
    /// 剪贴板配置（复制即记忆；隐私敏感，默认关闭）。
    #[serde(default)]
    pub clipboard: ClipboardConfig,
    /// 事件驱动触发配置（文件监听 + webhook；默认关闭）。
    #[serde(default)]
    pub events: EventsConfig,
    /// 自演化任务配置（ADR-017：每日演化智能体任务；默认启用）。
    #[serde(default)]
    pub evolution: EvolutionConfig,
    /// 主动提醒配置（记忆/规则 relevant-now 评估；默认关闭）。
    #[serde(default)]
    pub reminder: ReminderConfig,
    /// 子 Agent 角色配置（delegate_to_agent role 参数；同名覆盖内置角色，
    /// 新名字新增角色）。
    #[serde(default)]
    pub agent_roles: AgentRolesConfig,
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
    pub fn load() -> Result<Self, TianyanError> {
        let config_path = Self::find_config_file().ok_or_else(|| {
            TianyanError::Custom(
                "配置加载失败：未找到配置文件，请将 tianyan.toml 放置在当前目录、配置目录 (~/.config/tianyan) 或主目录 (~/.tianyan)".to_string(),
            )
        })?;

        Self::load_from_file(&config_path)
    }

    /// 从指定文件加载配置。
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, TianyanError> {
        use config::{File, FileFormat};

        let config_raw = config::Config::builder()
            .add_source(File::from(path).format(FileFormat::Toml))
            .build()
            .map_err(|e| TianyanError::Custom(format!("配置加载失败：构建配置失败：{}", e)))?;

        let mut config: TianyanConfig = config_raw
            .try_deserialize()
            .map_err(|e| TianyanError::Custom(format!("配置加载失败：反序列化配置失败：{}", e)))?;

        // 规范化路径类配置：展开 `~` 前缀（`~/.tianyan` 等常见写法；
        // 不展开时会被当作相对路径落在进程 cwd 下——历史上数据落到了
        // `{cwd}/~/.tianyan`）。
        config.storage.data_dir = expand_user_dir(&config.storage.data_dir);
        if let Some(ref sqlite_path) = config.storage.sqlite_path {
            config.storage.sqlite_path = Some(expand_user_dir(sqlite_path));
        }
        config.security.trash_directory = expand_user_dir(&config.security.trash_directory);
        config.security.allowed_directories = config
            .security
            .allowed_directories
            .iter()
            .map(|p| expand_user_dir(p))
            .collect();
        config.security.blocked_directories = config
            .security
            .blocked_directories
            .iter()
            .map(|p| expand_user_dir(p))
            .collect();

        config
            .validate()
            .map_err(|e| TianyanError::Custom(format!("配置加载失败：{e}")))?;

        Ok(config)
    }

    /// 在默认位置查找配置文件。
    ///
    /// 搜索顺序：
    /// 0. 环境变量 `TIANYAN_CONFIG` 显式指定（最高优先级，README/.env.example 已声明）
    /// 1. 当前目录 `./tianyan.toml`
    /// 2. %APPDATA%/tianyan/tianyan.toml (Windows) 或 ~/.config/tianyan/tianyan.toml
    /// 3. ~/.tianyan/tianyan.toml
    pub fn find_config_file() -> Option<std::path::PathBuf> {
        // 0. 环境变量显式覆盖（空值视为未设置）
        if let Ok(path) = std::env::var("TIANYAN_CONFIG") {
            if !path.trim().is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }

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
        self.evolution.validate()?;
        self.retrieval.validate()?;
        self.mcp.validate().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 生成默认配置文件内容。
    pub fn generate_default_toml() -> Result<String, TianyanError> {
        Ok(toml::to_string_pretty(&Self::default())?)
    }

    /// 将配置保存到文件。
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), TianyanError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TianyanError::Custom(format!("配置保存失败：创建目录失败：{}", e)))?;
        }

        let content = toml::to_string_pretty(self)?;

        std::fs::write(path, content)
            .map_err(|e| TianyanError::Custom(format!("配置保存失败：写入文件失败：{}", e)))?;

        Ok(())
    }
}

/// 展开用户目录前缀 `~`（`~` / `~/x` / `~\x`）；无 `~` 前缀时原样返回。
///
/// 配置中的 data_dir 常见写法 `~/.tianyan`：不展开会被当作相对路径落在
/// 进程 cwd 下（历史上数据落在了 `{cwd}/~/.tianyan`）。
fn expand_user_dir(path: &std::path::Path) -> std::path::PathBuf {
    let Some(home) = dirs::home_dir() else {
        return path.to_path_buf();
    };
    if path == std::path::Path::new("~") {
        return home;
    }
    match path.strip_prefix("~") {
        Ok(rest) if rest.as_os_str().is_empty() => home,
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
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
fn config_load_error(path: Option<&std::path::Path>, detail: TianyanError) -> TianyanError {
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
    fn test_expand_user_dir() {
        let home = dirs::home_dir().unwrap();
        // 纯 `~` → home
        assert_eq!(expand_user_dir(std::path::Path::new("~")), home);
        // `~/x` → home/x
        assert_eq!(
            expand_user_dir(std::path::Path::new("~/tianyan")),
            home.join("tianyan")
        );
        // 无 `~` 前缀原样
        let abs = std::path::PathBuf::from("C:\\data\\tianyan");
        assert_eq!(expand_user_dir(&abs), abs);
        // 非前缀 `~` 不展开
        let rel = std::path::PathBuf::from("a~b");
        assert_eq!(expand_user_dir(&rel), rel);
    }
    #[test]
    fn test_load_from_file_expands_tilde_in_path_configs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tianyan.toml");
        let home = dirs::home_dir().unwrap();
        let toml = r#"
            [storage]
            data_dir = "~/data"
            backend = "sqlite"
            sqlite_path = "~/db.sqlite"

            [storage.vector]
            collection_name = "t"
            vector_dimension = 768

            [security]
            trash_directory = "~/trash"
            allowed_directories = ['~/docs', 'C:\abs']
            blocked_directories = ["~/etc", "/"]

            [[models.providers]]
            name = "mock"
            endpoint = "http://127.0.0.1:9/v1"
            api_key = "k"
            timeout = 10
            enabled = true

            [[models.providers.models]]
            name = "m"
            capabilities = ["chat"]
        "#;
        std::fs::write(&path, toml).unwrap();

        let config = TianyanConfig::load_from_file(&path).unwrap();
        assert_eq!(config.storage.data_dir, home.join("data"));
        assert_eq!(config.storage.sqlite_path, Some(home.join("db.sqlite")));
        assert_eq!(config.security.trash_directory, home.join("trash"));
        assert_eq!(config.security.allowed_directories[0], home.join("docs"));
        assert_eq!(
            config.security.allowed_directories[1],
            std::path::PathBuf::from("C:\\abs")
        );
        assert_eq!(config.security.blocked_directories[0], home.join("etc"));
        assert_eq!(
            config.security.blocked_directories[1],
            std::path::PathBuf::from("/")
        );
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
    fn test_config_agent_roles_section_roundtrip() {
        // [agent_roles.<name>] 生产形式：随 TianyanConfig 序列化/反序列化往返
        let toml_str = r#"
            [agent_roles.researcher]
            model = "deepseek-r1"
            tools = ["web_search", "read_file"]
            max_turns = 30
        "#;
        let config: TianyanConfig = toml::from_str(toml_str).unwrap();
        let role = config
            .agent_roles
            .roles
            .get("researcher")
            .expect("应解析出角色");
        assert_eq!(role.model.as_deref(), Some("deepseek-r1"));
        assert_eq!(role.max_turns, Some(30));

        // 序列化后重新解析仍一致（旧配置无该节时默认空）
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed: TianyanConfig = toml::from_str(&serialized).unwrap();
        let role2 = reparsed
            .agent_roles
            .roles
            .get("researcher")
            .expect("往返后角色应存在");
        assert_eq!(role2.model, role.model);
        assert_eq!(role2.tools, role.tools);

        let legacy: TianyanConfig = toml::from_str("[agent]\n").unwrap();
        assert!(
            legacy.agent_roles.roles.is_empty(),
            "旧配置无 [agent_roles] 节应默认空"
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

    /// B5 分层覆盖：用户层只写部分键时，缺失字段继承默认层（serde default 即默认层）。
    #[test]
    fn test_user_layer_merges_over_default_layer() {
        // 用户层：只配置安全模式与一个 provider，其余全部缺失
        let user_toml = r#"
            [security]
            safety_mode = "strict"

            [[models.providers]]
            name = "user-provider"
            endpoint = "http://localhost:9999/v1"
            api_key = "test-key"
            enabled = true
        "#;
        let config: TianyanConfig = toml::from_str(user_toml).unwrap();

        // 用户层字段生效
        assert_eq!(config.security.safety_mode, SafetyMode::Strict);
        assert_eq!(config.models.providers.len(), 1);
        assert_eq!(config.models.providers[0].name, "user-provider");

        // 默认层继承：未写字段取默认值（不因用户层缺失而崩溃）
        assert!(
            !config.storage.data_dir.as_os_str().is_empty(),
            "data_dir 应继承默认层"
        );
        assert_eq!(
            config.agent.max_turns,
            AgentConfig::default().max_turns,
            "agent 应继承默认层"
        );
        assert!(config.mcp.servers.is_empty(), "mcp 应继承默认层（空）");
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
        let err = config_load_error(
            Some(bad_path),
            TianyanError::Custom("反序列化配置失败：syntax".to_string()),
        );
        let msg = err.to_string();
        assert!(msg.contains("config 加载失败"), "应含模块前缀：{msg}");
        assert!(
            msg.contains("/tmp/bad-config/tianyan.toml"),
            "应含配置路径：{msg}"
        );
        assert!(msg.contains("反序列化配置失败"), "应含错误详情：{msg}");

        // 路径未知时给出"默认搜索路径"占位
        let err = config_load_error(None, TianyanError::Custom("未找到配置文件".to_string()));
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

    /// 串行化 TIANYAN_CONFIG 环境变量测试（进程级全局副作用，避免并行测试互相干扰）。
    static ENV_CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 保存 / 恢复 TIANYAN_CONFIG 环境变量。
    fn with_tianyan_config_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_CONFIG_LOCK.lock().unwrap();
        let original = std::env::var("TIANYAN_CONFIG").ok();
        match value {
            Some(v) => std::env::set_var("TIANYAN_CONFIG", v),
            None => std::env::remove_var("TIANYAN_CONFIG"),
        }
        let result = f();
        match original {
            Some(v) => std::env::set_var("TIANYAN_CONFIG", v),
            None => std::env::remove_var("TIANYAN_CONFIG"),
        }
        result
    }

    #[test]
    fn test_find_config_file_prefers_tianyan_config_env() {
        // README/.env.example 声明的 TIANYAN_CONFIG 覆盖：显式指定路径应优先于默认搜索
        let dir = std::env::temp_dir().join(format!(
            "tianyan-config-env-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom.toml");
        std::fs::write(&path, "[storage]\n").unwrap();

        let found = with_tianyan_config_env(Some(path.to_str().unwrap()), || {
            TianyanConfig::find_config_file()
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found, Some(path), "TIANYAN_CONFIG 指定的路径应优先返回");
    }

    #[test]
    fn test_find_config_file_ignores_empty_tianyan_config_env() {
        // 空字符串环境变量不生效，回落到默认搜索（与未设置时结果一致）
        let normal = with_tianyan_config_env(None, TianyanConfig::find_config_file);
        let with_empty = with_tianyan_config_env(Some(""), TianyanConfig::find_config_file);
        assert_eq!(with_empty, normal, "空 TIANYAN_CONFIG 应回落默认搜索");
    }

    #[test]
    fn test_find_config_file_surfaces_missing_env_path() {
        // 显式指定但文件不存在：返回该路径（由 load 报错），不静默回落
        let missing = std::env::temp_dir().join(format!(
            "tianyan-config-missing-{}.toml",
            std::process::id()
        ));
        let found = with_tianyan_config_env(Some(missing.to_str().unwrap()), || {
            TianyanConfig::find_config_file()
        });
        assert_eq!(
            found,
            Some(missing),
            "显式 TIANYAN_CONFIG 路径应原样返回（含不存在场景）"
        );
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
