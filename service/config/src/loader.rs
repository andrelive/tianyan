//! 配置的**加载 / 持久化**（ADR-049 期 3-T4 从 `core/src/config/mod.rs` 整段搬出）。
//!
//! 本文件承载三类职责，全部是「服务层怎么读写磁盘」，不是「配置长什么样」：
//!
//! 1. **加载**：[`load`] / [`load_from_file`] / [`find_config_file`] /
//!    [`config_exists`] / [`default_config_path`]——配置路径的**单点定义**
//!    （`~/.tianyan/tianyan.toml`，`TIANYAN_CONFIG` 覆盖优先，ADR-023：配置目录
//!    不随数据目录搬迁）；
//! 2. **持久化**：[`save_to_file`]（段保全：保留程序未建模的键，T0-6）+
//!    [`generate_default_toml`]；
//! 3. **槽位装配助手**：[`load_and_install`]——把
//!    「加载失败 → 记错误 + 回退默认配置」这条**旧 `get_config()` 首次调用语义**
//!    搬到装配期，好让内核只保留一个纯槽位（见 crate 文档「槽位化」一节）。
//!
//! 逐字保留的行为契约（搬移时一行未改）：配置路径与环境变量优先级、`~` 展开、
//! `security.normalize()`、`validate` 的调用顺序与错误前缀（`配置加载失败：…`）、
//! 段保全的合并规则（未建模键保留 / 建模键以内存值为准）、`config 加载失败（路径：…）：…`
//! 这条诊断文案。

use std::path::{Path, PathBuf};

use tianyan::common::error::TianyanError;
use tianyan::config::TianyanConfig;

/// 配置文件名。
///
/// 与迁移模块的排除清单同名（`migration::CONFIG_FILE_NAME` 是**另一个**常量：
/// 那个用于「搬迁时不要动配置文件」，两者取值相同但语义独立——历史上就是
/// 两份字面量，搬移时未合并，以免把两处判据耦合成一处）。
const CONFIG_FILE_NAME: &str = "tianyan.toml";

/// 从固定位置加载配置：`~/.tianyan/tianyan.toml`。
///
/// 配置目录与数据目录是两个概念（ADR-023）：配置文件位置写死、
/// 不随 `storage.data_dir` 搬迁；数据目录从配置内容读取。
/// 如果找不到配置文件，返回错误。
pub fn load() -> Result<TianyanConfig, TianyanError> {
    let config_path = find_config_file().ok_or_else(|| {
        TianyanError::Custom(
            "配置加载失败：未找到配置文件 ~/.tianyan/tianyan.toml（可用环境变量 TIANYAN_CONFIG 指定其他路径）".to_string(),
        )
    })?;

    load_from_file(&config_path)
}

/// 从指定文件加载配置。
pub fn load_from_file(path: &Path) -> Result<TianyanConfig, TianyanError> {
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
    // 归一安全配置旧字段（ADR-033）：wait_for_approval / allow_all_operations
    // → approval_mode（仅读取兼容；下次写回时旧字段自然消失）。
    config.security.normalize();

    validate(&config).map_err(|e| TianyanError::Custom(format!("配置加载失败：{e}")))?;

    Ok(config)
}

/// 在固定位置查找配置文件。
///
/// 位置：`~/.tianyan/tianyan.toml`（写死，不随数据目录搬迁）；
/// 环境变量 `TIANYAN_CONFIG` 显式指定时最高优先级（e2e/CI 用）。
pub fn find_config_file() -> Option<PathBuf> {
    // 0. 环境变量显式覆盖（空值视为未设置；路径不存在时原样返回，由 load 报错）
    if let Ok(path) = std::env::var("TIANYAN_CONFIG") {
        if !path.trim().is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    // 1. 固定位置：~/.tianyan/tianyan.toml
    canonical_config_path().filter(|p| p.exists())
}

/// 固定配置路径（不检查存在性）：`~/.tianyan/tianyan.toml`。
fn canonical_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".tianyan").join(CONFIG_FILE_NAME))
}

/// 检查配置文件是否存在。
pub fn config_exists() -> bool {
    find_config_file().is_some()
}

/// 获取默认配置文件保存路径（配置不存在时首次写入用）：
/// `~/.tianyan/tianyan.toml`（与查找位置一致）。
pub fn default_config_path() -> Option<PathBuf> {
    canonical_config_path()
}

/// 验证配置（**逐字搬自 `TianyanConfig::validate`**，只把 `self` 改成参数）。
///
/// 返回 `Result<(), String>`（错误文案原样上抛）；各子配置的 `validate`
/// 方法仍留 core（它们随结构走，是字段自身的约束）。
pub fn validate(config: &TianyanConfig) -> Result<(), String> {
    config.agent.validate()?;
    config.storage.validate()?;
    config.models.validate()?;
    config.logging.validate()?;
    config.security.validate()?;
    config.evolution.validate()?;
    config.retrieval.validate()?;
    config.mcp.validate().map_err(|e| e.to_string())?;
    Ok(())
}

/// 生成默认配置文件内容。
pub fn generate_default_toml() -> Result<String, TianyanError> {
    Ok(toml::to_string_pretty(&TianyanConfig::default())?)
}

/// 将配置保存到文件。
///
/// **段保全（T0-6）**：保存前读取现有文件，把程序**未建模**的键（用户
/// 自定义段、未来版本字段、版本回滚场景）合并回输出——此前全量序列化
/// 覆盖会把它们静默清掉（跨版本回滚时配置退化）。程序建模的键以内存值
/// 为准。
pub fn save_to_file(config: &TianyanConfig, path: &Path) -> Result<(), TianyanError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TianyanError::Custom(format!("配置保存失败：创建目录失败：{}", e)))?;
    }

    let mut new_value = toml::Value::try_from(config)
        .map_err(|e| TianyanError::Custom(format!("配置保存失败：序列化失败：{}", e)))?;
    // 现有文件可解析时合并未建模键（不存在/解析失败则跳过——不阻断保存）
    if let Ok(existing_raw) = std::fs::read_to_string(path) {
        if let Ok(existing_value) = existing_raw.parse::<toml::Value>() {
            new_value = merge_unknown_keys(existing_value, new_value);
        }
    }

    let content = toml::to_string_pretty(&new_value)
        .map_err(|e| TianyanError::Custom(format!("配置保存失败：序列化失败：{}", e)))?;

    std::fs::write(path, content)
        .map_err(|e| TianyanError::Custom(format!("配置保存失败：写入文件失败：{}", e)))?;

    Ok(())
}

/// 合并“程序未建模的键”（段保全，T0-6）：以 `new`（当前配置序列化结果）为
/// 基准，保留 `existing`（磁盘现有文件）中 `new` 缺失的键；双方都有的表递归
/// 合并，标量/数组以 `new` 为准（程序建模值优先）。
fn merge_unknown_keys(existing: toml::Value, new: toml::Value) -> toml::Value {
    use toml::Value;

    match (existing, new) {
        (Value::Table(old), Value::Table(mut new_table)) => {
            for (key, old_val) in old {
                match new_table.remove(&key) {
                    Some(new_val) => {
                        new_table.insert(key, merge_unknown_keys(old_val, new_val));
                    }
                    // 未建模键：原样保留（自定义段 / 未来版本字段）
                    None => {
                        new_table.insert(key, old_val);
                    }
                }
            }
            Value::Table(new_table)
        }
        // 非表（标量 / 数组 / 类型不一致）：以程序当前值为准
        (_, new_val) => new_val,
    }
}

/// 展开用户目录前缀 `~`（`~` / `~/x` / `~\x`）；无 `~` 前缀时原样返回。
///
/// 配置中的 data_dir 常见写法 `~/.tianyan`：不展开会被当作相对路径落在
/// 进程 cwd 下（历史上数据落在了 `{cwd}/~/.tianyan`）。
fn expand_user_dir(path: &Path) -> PathBuf {
    let Some(home) = dirs::home_dir() else {
        return path.to_path_buf();
    };
    if path == Path::new("~") {
        return home;
    }
    match path.strip_prefix("~") {
        Ok(rest) if rest.as_os_str().is_empty() => home,
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// 构造配置加载错误（带路径上下文）。
///
/// `path` 为实际失败路径（文件存在但损坏）或预期配置路径（文件缺失）。
///
/// **逐字搬自 `core/src/config/mod.rs::config_load_error`**：失败时既写进 stderr
/// （见 [`load_and_install`]），也写进内核的诊断槽位（`tianyan::config::last_config_error`），
/// 两处共用这一份文案。
fn config_load_error(path: Option<&Path>, detail: TianyanError) -> TianyanError {
    let location = match path {
        Some(p) => p.display().to_string(),
        None => "默认搜索路径".to_string(),
    };
    TianyanError::Custom(format!("config 加载失败（路径：{location}）：{detail}"))
}

/// **装配期单点**：加载配置 → 注入内核槽位（失败时把错误记进诊断槽位并回退默认配置）
/// → 返回生效配置。
///
/// 这是「旧 `config::get_config()` 首次调用语义」（加载 → 失败时记错误并回退默认配置
/// → 写入进程级单例）在分层之后的**唯一落点**。装配层（`app` / `tauri`）在日志初始化
/// **之前**调用它，内核侧只剩 `tianyan::config::get_config()`（纯读槽位）。
///
/// 三处装配因此各写**一行**，而不是各抄一遍「加载 + 记错误 + 注入 + 回退默认」——后者
/// 是「多份口径同时漂移」的经典来源（与 `tianyan_policy_approval::gate` /
/// `tianyan_policy_context::pipeline` 同一手法）。
///
/// 失败路径的输出是**两条**，都是迁移前就有的行为，逐字保留：
///
/// - `eprintln!`：迁移前 `app/src/main.rs` 与 `tauri/src/lib.rs` 的兜底。配置破损时
///   `[logging]` 节读不出来、日志订阅者尚未装配（本函数就在 `init_logging` 之前被调
///   用），`tracing` 事件会被丢弃——只有 stderr 一定可见。
/// - `tracing::error!` + 诊断槽位：迁移前 `core::config::get_config()` 首次调用失败
///   的路径（含路径上下文的错误文案，由 [`config_load_error`] 构造）。
///
/// 槽位只在首次注入时生效（`OnceLock`）：同一进程内重复装配（重启前重读配置、
/// 热重载）**不会**改写内核槽位——返回值仍是**本次新加载**的配置，调用方照常用它
/// 重启服务（与迁移前 `TianyanConfig::load()` 的调用点语义一致）。见 crate
/// 文档「槽位化」一节的第二份真相说明。
pub fn load_and_install() -> TianyanConfig {
    let config = match load() {
        Ok(config) => config,
        Err(detail) => {
            let path = find_config_file().or_else(default_config_path);
            let error = config_load_error(path.as_deref(), detail);
            eprintln!("配置加载失败（使用默认配置）: {}", error);
            tracing::error!(error = %error, "加载配置文件失败，使用默认配置");
            tianyan::config::set_config_error(error);
            TianyanConfig::default()
        }
    };
    if tianyan::config::set_config(config.clone()).is_err() {
        tracing::debug!("配置槽位已初始化，保留首次注入的配置");
    }
    config
}

/// 测试共享的环境变量互斥锁（`std::env::set_var` 是进程级的，多线程下不安全）。
///
/// 与 `tianyan::config::test_support` 的锁**不共享**：那是 core 的 `--lib`
/// 测试二进制内部的锁，而本 crate 的测试跑在**另一个进程**里（cargo 每个
/// crate 一个测试二进制），`environ` 不跨进程。因此这里需要自己的锁——
/// 不是重复实现，是同一纪律在另一个地址空间里的实例。
#[cfg(test)]
fn with_env_var<T>(key: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_user_dir() {
        let home = dirs::home_dir().unwrap();
        // 纯 `~` → home
        assert_eq!(expand_user_dir(Path::new("~")), home);
        // `~/x` → home/x
        assert_eq!(
            expand_user_dir(Path::new("~/tianyan")),
            home.join("tianyan")
        );
        // 无 `~` 前缀原样
        let abs = PathBuf::from("C:\\data\\tianyan");
        assert_eq!(expand_user_dir(&abs), abs);
        // 非前缀 `~` 不展开
        let rel = PathBuf::from("a~b");
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
            blocked_directories = ["~/etc", "~/blocked"]

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

        let config = load_from_file(&path).unwrap();
        assert_eq!(config.storage.data_dir, home.join("data"));
        assert_eq!(config.storage.sqlite_path, Some(home.join("db.sqlite")));
        assert_eq!(config.security.trash_directory, home.join("trash"));
        assert_eq!(config.security.allowed_directories[0], home.join("docs"));
        assert_eq!(
            config.security.allowed_directories[1],
            PathBuf::from("C:\\abs")
        );
        assert_eq!(config.security.blocked_directories[0], home.join("etc"));
        assert_eq!(config.security.blocked_directories[1], home.join("blocked"));
    }

    /// T0-6 主回归：保存时保留程序未建模的段/字段——
    /// 此前 `save_to_file` 全量序列化覆盖，用户自定义段（或未来版本字段 /
    /// 版本回滚场景）会被静默清掉。
    #[test]
    fn test_save_to_file_preserves_unmodeled_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tianyan.toml");
        std::fs::write(
            &path,
            "[storage]\ndata_dir = 'D:/tianyan'\n\n[my_custom_section]\nkeep_me = 'yes'\n",
        )
        .unwrap();

        save_to_file(&TianyanConfig::default(), &path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("my_custom_section") && content.contains("keep_me"),
            "未建模段必须保全（此前全量覆盖会静默清掉）: {content}"
        );
    }

    /// 回归：程序建模的字段以内存值为准（段保全不得反向覆盖当前配置）。
    #[test]
    fn test_save_to_file_modeled_values_take_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tianyan.toml");
        std::fs::write(&path, "[agent]\nmax_turns = 7\n").unwrap();

        let mut config = TianyanConfig::default();
        config.agent.max_turns = 99;
        save_to_file(&config, &path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("max_turns = 99"),
            "建模字段以内存值为准: {content}"
        );
    }

    #[test]
    fn test_generate_default_toml() {
        let toml = generate_default_toml().unwrap();
        assert!(toml.contains("[storage]"));
        assert!(toml.contains("[models]"));
    }

    #[test]
    fn test_load_without_config_file() {
        // 测试在没有配置文件的情况下，从指定路径加载会返回错误
        let non_existent_path = PathBuf::from("/non/existent/path/tianyan.toml");
        assert!(load_from_file(&non_existent_path).is_err());
    }

    #[test]
    fn test_config_load_error_contains_path_and_detail() {
        // 纯函数验证加载失败路径记录的错误格式：
        // 模块前缀 + 配置路径 + 错误详情
        let bad_path = Path::new("/tmp/bad-config/tianyan.toml");
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

        let result = load_from_file(&path);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err(), "非法 TOML 应加载失败");
    }

    /// 保存 / 恢复 TIANYAN_CONFIG 环境变量。
    ///
    /// 走本 crate 的 env 锁（[`with_env_var`] 内的 `ENV_LOCK`）——core 的锁
    /// 只在 core 的测试进程内互斥，管不到这里。
    fn with_tianyan_config_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        with_env_var("TIANYAN_CONFIG", value, f)
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

        let found = with_tianyan_config_env(Some(path.to_str().unwrap()), find_config_file);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found, Some(path), "TIANYAN_CONFIG 指定的路径应优先返回");
    }

    #[test]
    fn test_find_config_file_ignores_empty_tianyan_config_env() {
        // 空字符串环境变量不生效，回落到默认搜索（与未设置时结果一致）
        let normal = with_tianyan_config_env(None, find_config_file);
        let with_empty = with_tianyan_config_env(Some(""), find_config_file);
        assert_eq!(with_empty, normal, "空 TIANYAN_CONFIG 应回落默认搜索");
    }

    #[test]
    fn test_find_config_file_surfaces_missing_env_path() {
        // 显式指定但文件不存在：返回该路径（由 load 报错），不静默回落
        let missing = std::env::temp_dir().join(format!(
            "tianyan-config-missing-{}.toml",
            std::process::id()
        ));
        let found = with_tianyan_config_env(Some(missing.to_str().unwrap()), find_config_file);
        assert_eq!(
            found,
            Some(missing),
            "显式 TIANYAN_CONFIG 路径应原样返回（含不存在场景）"
        );
    }

    /// 装配期单点回归（**新增**，ADR-049 期 3-T4）：配置破损 → 回退默认配置、注入内核
    /// 槽位、错误进诊断槽位。这是迁移前 `get_config()` 首次调用失败路径的等价语义，
    /// 此前只由 core 的 `test_get_config_once_semantics_and_error_query` 间接覆盖
    /// （且覆盖是条件式的——「若 Some 才检查」），现在直接断言。
    #[test]
    fn test_load_and_install_falls_back_to_default_on_broken_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "this is not [valid toml = [").unwrap();

        let config = with_tianyan_config_env(Some(path.to_str().unwrap()), load_and_install);

        assert!(
            !config.storage.data_dir.as_os_str().is_empty(),
            "加载失败应回退默认配置（不是空配置）"
        );
        assert!(
            tianyan::config::try_get_config().is_some(),
            "回退的默认配置也应注入槽位（调用方永远拿得到一份可用配置）"
        );
        assert!(
            tianyan::config::last_config_error().is_some(),
            "失败原因应可从诊断槽位查询（含路径上下文）"
        );
    }
}
