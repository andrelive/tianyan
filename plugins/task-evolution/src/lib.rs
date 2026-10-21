//! task-evolution 插件：自演化综述任务的实现（ADR-049 期 2 序 5b）。
//!
//! 迁入本 crate（**整文件搬移，含 29 个内联测试**）：`evolution_task.rs`
//! （2,035 行）——ADR-017 的三阶段编排（采集 → 综述 → 记账提交）。
//!
//! ## 为什么搬（判据：变化原因，D7）
//!
//! 演化任务是**策略**，不是机制：采集哪些命名空间、清单条目上限、综述提示词与
//! JSON 输出格式、合并/删除的语义都会随需求变。搬走后改演化策略只重编**本 crate +
//! 装配包**，server 库（13.4K 源码）与 core（约 49K）不动。core 从此**没有任何具体
//! 任务实现**（`core/src/scheduler/tasks/` 整目录删除）。
//!
//! ## 契约留 core：`EvolutionReviewExecutor`
//!
//! 任务本身只做两件事：采集材料、记账提交。真正「跑一轮主 agent 产出 diff 计划」的是
//! `AgentEvolutionExecutor`——**server 侧业务实现**（它要 `AgentCoordinator` 与工作集
//! 注册表，在专用演化会话里委托 `evolution_reviewer` 角色）。若把它的 trait 随实现一
//! 起搬进本 crate，server 就必须依赖本插件（**D2-1 的反向**：通道层不得依赖插件）。
//!
//! 因此 trait 提到 core 并**原样**保留语义：
//! [`tianyan::scheduler::EvolutionReviewExecutor`]（`core/src/scheduler/evolution_executor.rs`）。
//! 分工与 `ToolContext` / `TaskHandler` 一致：**内核定义能力集合 → 插件消费 →
//! 装配层构造实现**。本 crate 不认识 `AgentEvolutionExecutor`，也不认识
//! `AgentCoordinator`；提议/提交分离仍由架构强制（执行器只能返回文本，注册表写入只能
//! 经任务自身的记账通道）。
//!
//! ## 装配靠 `TaskServices.evolution_executor`
//!
//! 装配层把 [`tasks`] 包成 `tianyan_server::TaskFactory` 注入 `AssemblyServices`；
//! server 在调用工厂前 await 好能力集合 [`TaskServices`]，其中新增一个字段
//! `evolution_executor: Option<Arc<dyn EvolutionReviewExecutor>>`——由 server 构造
//! （`AgentEvolutionExecutor::new(agent_lock, working_sets, model, review_role)`）：
//!
//! ```text
//! server: cfg.evolution.enabled ? Some(AgentEvolutionExecutor::new(..)) : None
//!         → TaskServices.evolution_executor
//!         → 插件 tasks() 用它构造 EvolutionTask
//! ```
//!
//! 这正是 D2 的形状（协作只走「装配期注入 `Arc<dyn Trait>`」），插件之间零互赖。
//! 启用条件与间隔换算因此也落在本插件：[`tasks`] 里判 `config.evolution.enabled`、
//! `evolution_executor` 是否为 `Some`、以及 `interval_from_cron(&cron)` 的回退。
//!
//! ## ★ 为什么「顺序拼接」在装配层，而不是插件里
//!
//! 注册顺序 = 调度器扫描顺序 = 同一轮到期任务的执行顺序（`RegisteredTask` 的注释）。
//! 演化任务在迁移前排在清单**第 2 位**（`summary_generation` 之后、`garbage_collection`
//! 之前），这条纪律必须保持。但「第 2 位」是**跨插件属性**：
//!
//! - `tianyan-task-builtin` 不知道演化任务存在（它只生产自己的 5 项，保持相对顺序）；
//! - 本插件只生产 0/1 项，也不知道别人排在哪；
//! - 只有**装配清单**知道「哪个插件该排在哪」。
//!
//! 让插件互相 import 来拼接会破 **D2-2（插件之间零互赖）**。所以装配层的
//! `task_factory` 做显式拼接，且用 `splice(1..1, …)` 而不是 `insert` 循环——语义是
//! 「在 index 1 之前插入」= **紧跟 `summary_generation`**，让这条纪律在一行里可见：
//!
//! ```text
//! let mut v = tianyan_task_builtin::tasks(s);
//! v.splice(1..1, tianyan_task_evolution::tasks(s)); // summary 之后
//! v
//! ```
//!
//! 这样「evolution 排第 2」由装配层一行表达，插件侧的顺序断言只锁自己的相对位置
//! （本插件 0/1 项，无需断言跨插件位置）。
//!
//! ## 降级语义（与迁移前逐字一致）
//!
//! - `config.evolution.enabled == false` → 不注册（迁移前 `if cfg.evolution.enabled`
//!   门控）；
//! - `evolution_executor` 为 `None` → 不注册 + `warn!` 一次（能力未提供却已启用＝装配
//!   缺陷，静默不注册会让症状变成「演化永远不跑」——ADR-038 可见失败）；
//! - `interval_from_cron(&cron)` 为 `None` → `warn!` + 回退 `24 * 3600`（文案照抄
//!   迁移前 server 里的那段，一字未改）；注意默认配置 `0 0 */24 * * *` **就是**
//!   不可换算的模式（`*/N` 只在秒/分位置被识别），所以生产默认走的是回退分支；
//! - `max_items_per_run` 透传进 `EvolutionTask::new`。
//!
//! ## 测试期 dev-dependency（如实记录）
//!
//! 本 crate 的 `real_vfs()` 用真实 `SqliteBackend`（`plugins/storage-sqlite`）锁定
//! 「文件条目 vs 目录条目」的生产形态——那两个测试正是为 `find_entry` 只匹配目录的
//! 历史缺陷写的，退化成 `MockVfs` 就是重演该事故。core 当年靠 `#[path]` 同源包含
//! 插件源码绕开「同一 crate 两个编译单元」；本 crate 是独立 crate，直接
//! `[dev-dependencies] tianyan-storage-sqlite` 即可。**生产依赖图不受影响**
//! （`cargo tree -p tianyan-task-evolution --edges normal` 无 storage-sqlite / 无
//! server）。这是**测试期插件间依赖边**的第二例（第一例是
//! `plugins/storage-vector/examples`）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod evolution_task;

use std::sync::Arc;

use tianyan::scheduler::{TaskRegistration, TaskServices};

pub use evolution_task::EvolutionTask;

/// 本插件提供的定时任务（0 或 1 项；装配层一行拼接用）。
///
/// 返回 1 项时是 `evolution`（自演化综述，ADR-017）：名字 / 显示名 / 间隔 / 启用条件
/// 与迁移前 `server/src/lib.rs` 里那段 `register_task` **逐字一致**（本插件的
/// `test_tasks_registers_evolution_with_migrated_metadata` 锁住）。
///
/// **不注册的两种情形**（都返回空清单，见模块文档「降级语义」）：
/// 1. `config.evolution.enabled == false`；
/// 2. `services.evolution_executor` 为 `None`（装配层没有构造综述执行器实现）。
///
/// 位置纪律：本函数**不**决定自己在清单里的绝对位置——那是装配层的
/// `splice(1..1, …)`（跨插件属性，D2-2）。本函数只保证「要么 0 项，要么就是那 1 项」。
pub fn tasks(services: &TaskServices) -> Vec<TaskRegistration> {
    let cfg = &services.config;

    // 迁移前：`if cfg.evolution.enabled { … }`
    if !cfg.evolution.enabled {
        return Vec::new();
    }

    // 能力未提供：任务无从构造。留痕（否则症状是「演化永远不跑」，看不出是降级还是
    // 配置没生效——ADR-038 可见失败）。生产装配里 server 用同一个 `enabled` 判定是否
    // 构造执行器，因此这条分支只在嵌入式/测试装配里出现。
    let Some(executor) = services.evolution_executor.clone() else {
        tracing::warn!(
            "演化任务已启用但未提供 EvolutionReviewExecutor：不注册自演化综述任务——\
             装配层须在构造 TaskServices 时注入执行器实现"
        );
        return Vec::new();
    };

    // 旧 cron 配置自动换算为间隔（ADR-024 迁移）；换算不了回退每日一次。
    // 文案照抄迁移前的 `server/src/lib.rs`（一字未改）。
    let evolution_interval = match tianyan::scheduler::interval_from_cron(&cfg.evolution.cron) {
        Some(secs) => secs,
        None => {
            tracing::warn!(
                old = %cfg.evolution.cron,
                fallback_secs = 24 * 3600,
                "evolution.cron 无法换算为间隔（ADR-024 已弃用 cron），回退每日一次；可从配置中删除该字段"
            );
            24 * 3600
        }
    };

    vec![TaskRegistration::new(
        // 注册名（= TaskHandler::name()）/ 显示名 / 间隔与迁移前逐字一致
        "evolution",
        "自演化综述",
        evolution_interval,
        Arc::new(EvolutionTask::new(
            executor,
            cfg.evolution.max_items_per_run,
        )),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use tianyan::common::error::TianyanError;
    use tianyan::common::types::Message;
    use tianyan::config::TianyanConfig;
    use tianyan::db::Database;
    use tianyan::model::{ChatService, MockChatService};
    use tianyan::notification::{NoopNotificationSink, SharedNotificationSink};
    use tianyan::observability::usage_stats::UsageStats;
    use tianyan::scheduler::EvolutionReviewExecutor;
    use tianyan::session::{Session, SessionManager};

    /// 综述执行器桩（本插件的清单判定不真的跑综述）。
    struct StubExecutor;

    #[async_trait]
    impl EvolutionReviewExecutor for StubExecutor {
        async fn review(&self, _input: &str) -> Result<String, TianyanError> {
            Ok("{}".to_string())
        }
    }

    /// 空会话管理器（装配期只用其引用，清单判定不读会话）。
    struct StubSessionManager;

    #[async_trait]
    impl SessionManager for StubSessionManager {
        async fn create_session(&self, id: &str, _message: Message) -> tianyan::Result<Session> {
            Ok(Session::new(id))
        }

        async fn get_session(&self, _id: &str) -> tianyan::Result<Option<Session>> {
            Ok(None)
        }

        async fn list_sessions(&self) -> tianyan::Result<Vec<Session>> {
            Ok(Vec::new())
        }
    }

    /// 构造隔离的 `UsageStats`（内存 SQLite，模式同 `usage_stats::tests::setup`）。
    async fn make_usage_stats() -> Arc<UsageStats> {
        let db = Database::open_in_memory().unwrap();
        db.init_schemas().await.unwrap();
        UsageStats::new(db).unwrap()
    }

    /// 构造装配能力集合；`executor` 由调用方指定（`None` = 未注入实现）。
    async fn make_services(
        config: TianyanConfig,
        executor: Option<Arc<dyn EvolutionReviewExecutor>>,
    ) -> TaskServices {
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let sink: SharedNotificationSink = Arc::new(NoopNotificationSink);
        TaskServices {
            config,
            usage_stats: make_usage_stats().await,
            snapshot_manager: None,
            session_manager: Arc::new(StubSessionManager),
            notification_sink: sink,
            chat,
            chat_model: "test-model".to_string(),
            working_sets: None,
            evolution_executor: executor,
        }
    }

    /// 默认配置（`enabled = true`）+ 已注入执行器 → 恰好 1 项；名字 / 显示名 / 间隔
    /// 与迁移前 server 里那段 `register_task` 逐字一致（间隔取默认 cron 的回退值）。
    #[tokio::test]
    async fn test_tasks_registers_evolution_with_migrated_metadata() {
        let services = make_services(
            TianyanConfig::default(),
            Some(Arc::new(StubExecutor) as Arc<dyn EvolutionReviewExecutor>),
        )
        .await;
        let registrations = tasks(&services);

        assert_eq!(registrations.len(), 1, "默认配置下应注册 1 个演化任务");
        let r = &registrations[0];
        assert_eq!(r.name, "evolution", "注册名与迁移前一致");
        assert_eq!(r.display_name, "自演化综述", "显示名与迁移前一致");
        assert_eq!(
            r.handler.name(),
            r.name,
            "处理器 name() 必须等于注册名（状态面板/日志口径）"
        );
        // 默认 cron `0 0 */24 * * *` 不可换算（`*/N` 只识别秒/分位置）→ 回退 24h。
        // 迁移前同款：`interval_from_cron(..).unwrap_or(fallback 24*3600)`。
        assert_eq!(
            r.interval_secs,
            24 * 3600,
            "默认 cron 走回退分支：每日一次（迁移前行为）"
        );
    }

    /// 间隔来自 `interval_from_cron`：可换算的 cron 必须被采用（不是恒取 24h）。
    #[tokio::test]
    async fn test_tasks_interval_follows_convertible_cron() {
        let mut config = TianyanConfig::default();
        // 「0 */6 * * * *」= 每 6 分钟 → 360 秒
        config.evolution.cron = "0 */6 * * * *".to_string();
        let services = make_services(
            config,
            Some(Arc::new(StubExecutor) as Arc<dyn EvolutionReviewExecutor>),
        )
        .await;

        let interval = tasks(&services)
            .into_iter()
            .find(|r| r.name == "evolution")
            .map(|r| r.interval_secs);
        assert_eq!(
            interval,
            Some(360),
            "可换算的 cron 必须转成间隔（ADR-024 迁移语义）"
        );
    }

    /// 启用条件：`evolution.enabled == false` → 不注册（迁移前 `if cfg.evolution.enabled`）。
    #[tokio::test]
    async fn test_tasks_skipped_when_disabled() {
        let mut config = TianyanConfig::default();
        config.evolution.enabled = false;
        let services = make_services(
            config,
            Some(Arc::new(StubExecutor) as Arc<dyn EvolutionReviewExecutor>),
        )
        .await;
        assert!(
            tasks(&services).is_empty(),
            "enabled = false 时不得注册（即便执行器可用）"
        );
    }

    /// 能力未提供：`evolution_executor = None` → 不注册（任务无从构造），且不 panic。
    ///
    /// 判别力：若这里改成「用默认实现兜底」，插件就得知道 server 的业务实现（D2-1 破）；
    /// 若改成 panic，嵌入式/测试装配全部起不来。
    #[tokio::test]
    async fn test_tasks_skipped_when_executor_missing() {
        let services = make_services(TianyanConfig::default(), None).await;
        assert!(
            tasks(&services).is_empty(),
            "未提供综述执行器时不得注册（能力缺失 ≠ 世界缺一块）"
        );
    }

    /// 判别力：`tasks()` 不消费能力集合（`&TaskServices`），可重复调用——装配入口在
    /// 数据目录搬迁重启时会再次装配。
    #[tokio::test]
    async fn test_tasks_is_repeatable() {
        let services = make_services(
            TianyanConfig::default(),
            Some(Arc::new(StubExecutor) as Arc<dyn EvolutionReviewExecutor>),
        )
        .await;
        let first: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();
        let second: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
    }
}
