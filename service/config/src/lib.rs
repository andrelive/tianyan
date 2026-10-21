//! 天演**服务层配置 crate**：加载 / 持久化 / 数据目录搬迁 / 校验 / 向导 DTO。
//!
//! ## 为什么外移（ADR-049 期 3-T4）
//!
//! `core/src/config/` 里有两类东西，此前混在一起：
//!
//! | 类别 | 例子 | 变化原因 |
//! |---|---|---|
//! | **配置的形状** | `TianyanConfig` + `agent` / `storage` / `model` / `security` / … 子结构与其 `Default` | 「模型多了一个能力档位」「安全模式多了一态」——内核与插件共同消费的**构造参数** |
//! | **配置怎么落到磁盘** | `load` / `load_from_file` / `find_config_file` / `save_to_file`（段保全）/ `migration`（数据目录搬迁）/ `validation` / `wizard` | 「配置文件放哪」「坏配置怎么回退」「搬迁怎么保数据」「校验错误怎么说」——**服务层的运维语义** |
//!
//! 按 ADR-049 D7「按变化原因切」，第二类不该在内核里：改一条校验文案、调一次搬迁
//! 的排除清单、换一个配置路径，都要重编 `tianyan-core`（42.4K 生产行）与全部下游。
//! 本 crate 承接第二类，共 **753 生产行**（`migration.rs` 319、`validation.rs` 115、
//! `wizard.rs` 49、`config/mod.rs` 的加载/持久化段约 270）。
//!
//! ## 为什么结构与预设表留 core（不是遗漏）
//!
//! - **`TianyanConfig` 与全部子 Config 结构留 core**：它们是内核与插件的**共同构造
//!   参数**——`TaskServices.config`、`ContextServices.config`、`PluginServices.web_config`、
//!   `TaskScheduler.config` 等都以它为字段。搬走等于让所有插件反向依赖服务层
//!   （破 D2-1「插件只依赖内核」）。
//! - **`presets.rs`（`PROVIDER_PRESETS` / `find_preset`）留 core**：它是配置**数据表**，
//!   server 的配置 API 要展示预设列表（`api/config/services.rs` 的
//!   `resolve_all_preset_hits`）。搬进 `provider-llm` 会让 server 反向依赖插件。
//! - **各子配置的 `validate`（`AgentConfig::validate` 等）留 core**：那是字段自身的
//!   约束（随结构走）；本 crate 的 [`validate`] 只做**汇总编排**（顺序与错误前缀
//!   与迁移前逐字一致）。
//!
//! ## ★ `get_config()` 的槽位化（本批唯一的设计点）
//!
//! 迁移前：`tianyan::config::get_config()` 内部会 `load()`（读文件 + 迁移 + 校验），
//! 用 `OnceLock::get_or_init` 保证「首次访问时加载一次」。加载器搬走后，内核不能
//! `use tianyan_service_config::…`（D2-1 反向依赖），于是改成**槽位 + 注入**：
//!
//! | 侧 | 职责 |
//! |---|---|
//! | 内核（`core/src/config/mod.rs`） | `get_config()` 纯读槽位；未注入时回退 `TianyanConfig::default()`（保持「无配置文件也能跑」）；`set_config(cfg)` 只在未设置时生效并返回 `Result`（重复设置被拒并交还入参）；`try_get_config()`；`set_config_error` / `last_config_error`（诊断槽位，与配置槽位同生命周期） |
//! | 服务层（本 crate） | [`load`] / [`load_from_file`] / [`load_and_install`] / [`save_to_file`] / [`generate_default_toml`] / 路径解析 / [`migration`] / [`validation`] / [`wizard`] |
//! | 装配层（`app/src/main.rs`、`tauri/src/lib.rs`） | 启动时调用 [`load_and_install`]（**先注入再 `tianyan::init()`**，因为 `init()` 要读 `logging` 节），再把同一份配置交给 `start_server_*` |
//!
//! 为什么用槽位而不是让内核持一个 `dyn` 配置源：`get_config() -> &'static TianyanConfig`
//! 是既有公开签名（`init()` 与下游都在用），`OnceLock` 是唯一能给出 `&'static` 且零成本的
//! 形态；改成 trait 对象要么引入 `Arc` + 生命周期传染，要么改变返回类型（破坏 S 级面）。
//!
//! **第二份真相（既有问题，本批照原样保留）**：server 的配置热更新走
//! `AppState::config`（`Arc<RwLock<TianyanConfig>>`）——它持久化到磁盘、更新 `AppState`、
//! 热重载 Agent，但**不**改写内核槽位（`OnceLock` 本就不可改写）。迁移前也是如此
//! （此前 `get_config()` 在 server 侧根本没有调用点，槽位与 `AppState` 从来不是同一份）。
//! 本批不改变这一行为，只在文档里把它记明：**槽位 = 进程启动快照（供 `init()` 读日志配置），
//! `AppState::config` = 运行期权威（供 API 与热重载）**。期 4 引入真正的配置源 seam 时，
//! 这两者才应合并。
//!
//! ## 测试守恒
//!
//! `core --lib` 的 1108 条基线里，与「加载 / 持久化 / 迁移 / 校验 / 向导」直接相关的
//! **28 条**随实现整体迁入本 crate（逐条同名同断言）：`loader` 11（`~` 展开 / 段保全 ×2 /
//! 默认 TOML / 缺文件 / 错误文案 / 非法 TOML / `TIANYAN_CONFIG` ×3），`migration` 11
//! （Windows 上编译的那一组），`validation` 5，`wizard` 1。算式见
//! ADR-049「实施记录（期 3-T4）」。
//!
//! ## 公开面
//!
//! 本 crate 是**服务层 crate**（不是能力插件、没有插槽），消费者有三个：`tianyan-app`、
//! `tianyan-tauri`、`tianyan-server`（迁移 API 与热更新写盘）。因此 ADR-049 的
//! 「每插件 crate 公开符号 ≤ 15」这条反指标不适用于此处的符号面判据；导出集与迁移前
//! `tianyan::config::{…}` 的对应项**逐个等价**（函数名从 `TianyanConfig::load` 这类
//! 关联函数改成自由函数，语义与文案不变）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod loader;
pub mod migration;
pub mod validation;
pub mod wizard;

pub use loader::{
    config_exists, default_config_path, find_config_file, generate_default_toml, load,
    load_and_install, load_from_file, save_to_file, validate,
};
pub use migration::{
    delete_source_entries, perform_migration, read_migration_request, run_pending_migration,
    run_pending_migration_at, update_config_data_dir, update_config_data_dir_at,
    validate_migration_target, write_migration_request, MigrationOutcome, MigrationRequest,
    MIGRATION_REQUEST_FILE,
};
pub use validation::{
    validate_agent_config, validate_models_config, validate_provider, validate_storage_config,
    validation_errors_to_strings, ValidationResult,
};
pub use wizard::{TestConnectionRequest, TestConnectionResponse};
