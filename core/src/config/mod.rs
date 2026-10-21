//! Tianyan 代理系统的配置**结构**与全局**槽位**。
//!
//! 本模块只做两件事：定义配置的形状（[`TianyanConfig`] 与各子 Config）与提供
//! 进程级配置槽位（[`get_config`] / [`set_config`]）。
//!
//! # 加载器已外移（ADR-049 期 3-T4）
//!
//! 「配置怎么落到磁盘」整段搬去 `tianyan-service-config`：`load` / `load_from_file` /
//! `find_config_file` / `config_exists` / `default_config_path` / `generate_default_toml` /
//! `save_to_file`（段保全）/ `validate`（汇总编排）/ `migration`（数据目录搬迁）/
//! `validation` / `wizard`。判据是 ADR-049 D7「按变化原因切」：改一条校验文案、
//! 调一次搬迁排除清单、换一个配置路径，不该重编内核。
//!
//! 内核对配置的依赖因此**只剩数据形状**：它们仍是内核与插件的共同构造参数
//! （`TaskServices.config` / `ContextServices.config` / `PluginServices.web_config` …），
//! 搬走会让所有插件反向依赖服务层（破 D2-1）。预设表 `presets` 同理留在这里
//! （server 的配置 API 要展示预设，搬进插件会让 server 反向依赖插件）。
//!
//! 槽位语义：**装配层注入、内核只读**。未注入时 [`get_config`] 回退
//! `TianyanConfig::default()`（保持「没有配置文件也能跑」），注入走 [`set_config`]。

use serde::{Deserialize, Serialize};

use crate::common::error::TianyanError;

mod agent;
pub mod api_types;
mod clipboard;
mod events;
mod evolution;
mod executor;
pub mod mcp;
mod memory;
mod model;
mod presets;
mod reminder;
mod retrieval;
mod roles;
mod security;
mod storage;
mod tool_output;
pub mod web;

pub use crate::common::logging::LoggingConfig;
pub use agent::AgentConfig;
pub use clipboard::ClipboardConfig;
pub use events::EventsConfig;
pub use evolution::EvolutionConfig;
pub use executor::ExecutorConfig;
pub use mcp::{McpConfig, McpServerEntry};
pub use memory::MemoryConfig;
pub use model::{
    find_provider, CacheField, DialectPreset, EmbeddingUsageShape, HeaderBinding, HeaderSource,
    ModelCapability, ModelEntry, ModelPreferences, ModelRef, ModelsConfig, ProviderConfig,
    ProviderDialect, ThinkingField, ThinkingParam,
};
pub use presets::{find_preset, ProviderPreset, PROVIDER_PRESETS};
pub use reminder::ReminderConfig;
pub use retrieval::RetrievalConfig;
pub use roles::AgentRolesConfig;
pub use security::{ApprovalMode, SafetyMode, SecurityConfig};
pub use storage::{StorageBackendType, StorageConfig, VectorStorageConfig};
pub use tool_output::ToolOutputConfig;
pub use web::WebConfig;

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
    /// 执行器配置（命令执行底层 shell provider；ADR-037）。
    #[serde(default)]
    pub executor: ExecutorConfig,
    /// 工具输出落盘配置（超预算结果落盘，会话只保留预览 + 路径；ADR-047）。
    #[serde(default)]
    pub tool_output: ToolOutputConfig,
}

/// 全局配置槽位（进程级单例，**只由装配层注入**）。
///
/// 迁移前本槽位用 `get_or_init` 在首次访问时**加载**配置（读文件 + 迁移 + 校验）；
/// 加载器搬去 `tianyan-service-config` 之后（ADR-049 期 3-T4），内核不再依赖服务层
/// （D2-1），槽位因此只承担两件事：**存**（[`set_config`]）与**读**
/// （[`get_config`] / [`try_get_config`]）。
static CONFIG: std::sync::OnceLock<TianyanConfig> = std::sync::OnceLock::new();

/// 未注入时的回退配置（保持「没有配置文件也能跑」的语义）。
///
/// 刻意与 [`CONFIG`] 分成两个 `OnceLock`：这样 [`get_config`] 在注入**之前**被
/// 调用也不会把槽位占死。若沿用迁移前的 `get_or_init`（回退值直接写进同一个
/// 槽位），「先有人读了配置、装配层再注入」就会静默失败——那是个只在装配顺序
/// 出错时才暴露的陷阱，不值得为了少一个 `static` 留着。
static FALLBACK_CONFIG: std::sync::OnceLock<TianyanConfig> = std::sync::OnceLock::new();

/// 全局配置加载错误（仅首次写入生效）。
///
/// 与 [`CONFIG`] 同生命周期：进程内只记录首次加载的结果，不可重置；加载成功时
/// 保持为空。**写入方是装配层**（`tianyan_service_config::load_and_install` 在加载
/// 失败时调用 [`set_config_error`]），读侧仍是 [`last_config_error`]。
static CONFIG_LOAD_ERROR: std::sync::OnceLock<TianyanError> = std::sync::OnceLock::new();

/// 获取全局配置的引用。
///
/// 读的是装配层注入的槽位（[`set_config`]）；**未注入时回退
/// `TianyanConfig::default()`**——与迁移前「加载失败回退默认配置」的对外语义一致
/// （调用方永远拿得到一份可用配置，不会 panic）。
///
/// 返回 `&'static`（`OnceLock` 语义）：同一进程内指针稳定，可安全长期持有。
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
    match CONFIG.get() {
        Some(config) => config,
        None => FALLBACK_CONFIG.get_or_init(TianyanConfig::default),
    }
}

/// 注入全局配置（**装配层职责**，内核自身不调用）。
///
/// 只在槽位为空时生效：重复注入返回 `Err(`[`ConfigSlotOccupied`]`)`，已生效的配置保持不变
/// （进程内「首次注入即权威」，与迁移前 `OnceLock::get_or_init` 的单次初始化语义
/// 一致）。返回 `Result` 而不是静默忽略，是为了让装配层能**察觉**「有人在我之前
/// 注入过」——热重载 / 搬迁重启是唯一会出现重复注入的场景。
///
/// 与迁移前的差异只有一处：**加载动作发生在调用方**（
/// `tianyan_service_config::load` / `load_and_install`），本函数不碰磁盘。
///
/// 失败类型刻意做成零大小的 [`ConfigSlotOccupied`] 而不是交还整份配置
/// （`Result<(), TianyanConfig>` 会被 `clippy::result_large_err` 判为「Err 变体过大」：
/// 这份配置 1.2KB，而调用方在注入失败时本来就用的是自己手里那份，无需回传）。
pub fn set_config(config: TianyanConfig) -> Result<(), ConfigSlotOccupied> {
    CONFIG.set(config).map_err(|_| ConfigSlotOccupied)
}

/// [`set_config`] 的失败原因：槽位已被注入过。
///
/// 零大小类型：调用方只需要知道「晚了」，不需要拿回被拒的配置副本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigSlotOccupied;

/// 查询已注入的配置（未注入时 `None`）。
///
/// 与 [`get_config`] 的区别：本函数**不做默认值回退**，用于装配期探针
/// （「我到底注入了没有」）与测试。
pub fn try_get_config() -> Option<&'static TianyanConfig> {
    CONFIG.get()
}

/// 记录配置加载失败的原因（**装配层职责**，仅首次写入生效）。
///
/// 语义与迁移前 `get_config()` 首次调用失败的路径一致：错误只在第一次写入时留下，
/// 之后不可重置；读侧是 [`last_config_error`]。
pub fn set_config_error(error: TianyanError) {
    let _ = CONFIG_LOAD_ERROR.set(error);
}

/// 查询全局配置加载错误。
///
/// 若配置加载失败（文件缺失或内容损坏），返回 `Some`，错误消息包含配置路径与
/// 失败详情；否则返回 `None`。
///
/// 用于诊断"配置损坏却静默回退默认配置"的场景：
/// 调用方（如 server 诊断接口）可据此向用户暴露配置异常。
pub fn last_config_error() -> Option<&'static TianyanError> {
    CONFIG_LOAD_ERROR.get()
}

/// 测试共享设施（`#[cfg(test)]`）。
///
/// **进程环境变量测试的全局互斥**：`std::env::set_var/remove_var` 会改写进程级
/// `environ`（glibc 上要 realloc 该数组，与分配器锁交互，多线程下素来脆弱）。
/// 此前 `config` 下各测试模块**各持一把私有锁**（`ENV_CONFIG_LOCK` /
/// `ENV_DATA_DIR_LOCK`），彼此不互斥 —— 全量并发跑时这些测试仍会与其它线程交错。
/// 0.6.3 CI 实测的指纹：**全量 + 多线程挂死、单线程或按模块跑必过**（详见
/// docs/operations/ci-pipeline.md §3）。收敛到这一把锁，"全量并发"才真正安全。
#[cfg(test)]
pub(crate) mod test_support {
    /// 跨模块共享的环境变量测试互斥锁。
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 持锁设置环境变量 → 执行 `f` → 恢复原值，返回 `f` 的结果。
    ///
    /// 锁中毒（前一个持锁测试 panic）时取回内部值继续，避免一个失败拖垮其余 env 测试。
    pub(crate) fn with_env_var<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let result = f();
        match original {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        result
    }
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
        assert!(config.retrieval.validate().is_ok());
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

    /// 槽位语义（ADR-049 期 3-T4）：注入前回退默认 / 首次注入生效 / 重复注入被拒 /
    /// 诊断错误槽位只记首次。
    ///
    /// `CONFIG` / `FALLBACK_CONFIG` / `CONFIG_LOAD_ERROR` 都是进程级单例且不可重置
    /// （`OnceLock`），本测试是 lib 测试二进制中**唯一**触碰它们的测试（其余模块只用
    /// `TianyanConfig::default()`），因此这里观察到的即全进程行为。加载器一侧的断言
    /// （路径解析 / `~` 展开 / 段保全 / 错误文案）已随实现迁去 `tianyan-service-config`
    /// （`loader.rs` 的同名测试）。
    #[test]
    fn test_config_slot_injection_semantics() {
        // 1. 未注入 → `get_config()` 回退默认配置（「没有配置文件也能跑」），
        //    指针稳定（`OnceLock` 语义：可安全长期持有 `&'static`）。
        assert!(try_get_config().is_none(), "测试开始时槽位应为空");
        let fallback = get_config();
        assert!(!fallback.storage.data_dir.as_os_str().is_empty());
        assert!(std::ptr::eq(fallback, get_config()));
        assert!(
            try_get_config().is_none(),
            "回退默认值不得占用注入槽位（否则装配层的注入会静默失败）"
        );

        // 2. 首次注入生效（装配层职责；内核自身不调用）。
        let injected = TianyanConfig {
            agent: AgentConfig {
                max_turns: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(set_config(injected).is_ok());
        assert_eq!(get_config().agent.max_turns, 42);

        // 3. 重复注入被拒：交还被拒的配置，已生效配置不变。
        let conflicting = TianyanConfig {
            agent: AgentConfig {
                max_turns: 7,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(set_config(conflicting).is_err(), "槽位已满时应拒绝重复注入");
        assert_eq!(get_config().agent.max_turns, 42, "首次注入即权威");

        // 4. 诊断槽位：初值为空 → 首次写入可见 → 文案不被改写。
        assert!(last_config_error().is_none());
        set_config_error(TianyanError::Custom(
            "config 加载失败（路径：/tmp/x/tianyan.toml）：坏 TOML".to_string(),
        ));
        let err = last_config_error().expect("首次写入后应可查询");
        assert!(err.to_string().contains("config 加载失败"), "应含模块前缀");
    }
}
