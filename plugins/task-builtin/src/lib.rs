//! task-builtin 插件：5 个内置定时任务的实现（ADR-049 期 2 序 5a）。
//!
//! 迁入本 crate（**整文件搬移，含各自的内联测试**）：
//!
//! | 文件 | 任务 | 注册名 | 间隔 |
//! |---|---|---|---|
//! | `summary_task.rs` | 摘要生成（扫描 VFS 补 L0/L1 + 向量） | `summary_generation` | 6h |
//! | `gc_task.rs` | 垃圾回收（规则归档 / 记忆 TTL / 向量-内容对账） | `garbage_collection` | 6h |
//! | `usage_stats_flush_task.rs` | 使用统计刷盘（内存计数器 → SQLite） | `usage_stats_flush` | 60s |
//! | `snapshot_gc_task.rs` | 快照垃圾回收（孤儿对象 + 孤儿重做缓存） | `snapshot_gc` | 12h |
//! | `reminder_task.rs` | 主动提醒（LLM 评估记忆 → 通知 + 注入会话） | `reminder` | `interval_min*60` |
//!
//! ## 为什么搬（判据：变化原因）
//!
//! 这 5 个任务是**策略**，不是机制：摘要阈值、GC 的 TTL 白名单、提醒的 prompt 与
//! 采样条数都会随需求变（ADR-049 D7「按变化原因切」）。搬走后改任务策略 / 新增内置
//! 任务只重编**本 crate + 装配包**，server 库（13.4K 源码）与 core（49K）不动。
//!
//! ## 契约与机制仍留在 core
//!
//! - [`tianyan::scheduler::TaskHandler`]：任务执行契约（`execute(&TaskContext)` + `name()`）；
//! - [`tianyan::scheduler::TaskContext`]：运行时世界（vfs / summary_engine /
//!   session_recall / config / task_state），由调度器在执行时提供；
//! - [`tianyan::scheduler::TaskScheduler`]：间隔扫描 + 宕机补跑机制（ADR-024）；
//! - [`tianyan::scheduler::TaskRegistration`] / [`tianyan::scheduler::TaskServices`]：
//!   **装配原语**（本插件返回的清单项、接收的能力集合）。
//!
//! 因此本 crate 只依赖 `tianyan`（kernel-api），**不依赖 `tianyan-server`**：若
//! 清单项类型住在 server，插件就得反向依赖通道层——那是 ADR-049 D2-1 的第一条铁律。
//!
//! ## 装配靠工厂（类型擦除）
//!
//! 装配层（`app` bin / `tauri`）把 [`tasks`] 包成
//! `tianyan_server::TaskFactory`（`Arc<dyn Fn(&TaskServices) -> Vec<TaskRegistration>>`）
//! 注入 `AssemblyServices`；server 只按返回顺序 `register_task`，不认识任何具体任务。
//! 类型别名在 server 侧（本插件不能命名它——那需要依赖 server），装配层一行：
//! `Arc::new(tianyan_task_builtin::tasks)`。
//!
//! ## 降级语义
//!
//! **未装配工厂 = 不注册任何内置任务**（server 侧 `warn!` 一次指路，不静默、不报错）。
//! 装配后，各任务自身的启用条件照旧在 [`tasks`] 里判定（与迁移前那 5 段
//! `register_task` 逐字一致）：
//!
//! - `snapshot_manager` 为 `None`（未配置工作区）→ **跳过** `snapshot_gc`；
//! - `config.reminder.enabled == false` → **跳过** `reminder`；启用时间隔取整分钟且
//!   至少 60s（`interval_secs.max(60) / 60 * 60`）；
//! - 其余三个无条件注册（摘要 / GC / 统计刷盘）。
//!
//! 注册顺序固定 `summary_generation` → `garbage_collection` → `usage_stats_flush` →
//! `snapshot_gc` → `reminder`（间隔制调度是「单一扫描循环按注册顺序串行执行」，
//! 顺序即执行顺序，迁移前后一致）。
//!
//! **演化任务不在本清单里**（ADR-049 期 2 序 5b）：它由 `tianyan-task-evolution` 提供，
//! 装配层把它 `splice` 到本清单**第 2 位**（紧跟 `summary_generation`，与迁移前一致）
//! ——跨插件的相对位置只有装配清单知道，插件之间不互相 import（D2-2）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod gc_task;
pub mod reminder_task;
pub mod snapshot_gc_task;
pub mod summary_task;
pub mod usage_stats_flush_task;

use std::sync::Arc;

use tianyan::scheduler::{TaskRegistration, TaskServices};

pub use gc_task::GcTask;
pub use reminder_task::ReminderTask;
pub use snapshot_gc_task::SnapshotGcTask;
pub use summary_task::SummaryTask;
pub use usage_stats_flush_task::UsageStatsFlushTask;

/// 本插件提供的全部定时任务（装配层一行注册用）。
///
/// 返回顺序即注册顺序（= 扫描顺序 = 同一轮到期任务的执行顺序）：
/// `summary_generation` → `garbage_collection` → `usage_stats_flush` →
/// `snapshot_gc` → `reminder`。名字 / 显示名 / 间隔与迁移前
/// `server/src/lib.rs` 里那 5 段 `register_task` **逐字一致**（本插件的
/// `test_tasks_order_names_and_intervals` 锁住）。
///
/// 条件注册（迁移前同款，见模块文档「降级语义」）：未配置工作区跳过 `snapshot_gc`，
/// `reminder.enabled == false` 跳过 `reminder`。
pub fn tasks(services: &TaskServices) -> Vec<TaskRegistration> {
    let cfg = &services.config;
    let mut registrations = Vec::with_capacity(5);

    // 摘要任务（ADR-017：降频每 6 小时；知识导入同步摘要，本任务仅兜底；
    // 范围排除 Session 命名空间——会话回忆改 FTS5）
    registrations.push(TaskRegistration::new(
        "summary_generation",
        "摘要生成",
        6 * 3600,
        Arc::new(SummaryTask::new()),
    ));

    // 垃圾回收任务（每 6 小时；auto_cleanup 来自存储配置）
    registrations.push(TaskRegistration::new(
        "garbage_collection",
        "垃圾回收",
        6 * 3600,
        Arc::new(GcTask::new(&cfg.storage)),
    ));

    // 使用统计刷盘任务（每 1 分钟；内存计数器 → SQLite，
    // 保证 /api/v1/stats 数据时效与内存有界，退出前由 shutdown 兜底）
    registrations.push(TaskRegistration::new(
        "usage_stats_flush",
        "使用统计刷盘",
        60,
        Arc::new(UsageStatsFlushTask::new(services.usage_stats.clone())),
    ));

    // 快照垃圾回收任务（每 12 小时；未配置工作区时 snapshot_manager 为 None，跳过）
    if let Some(snapshot_manager) = services.snapshot_manager.clone() {
        registrations.push(TaskRegistration::new(
            "snapshot_gc",
            "快照垃圾回收",
            12 * 3600,
            Arc::new(SnapshotGcTask::new(snapshot_manager)),
        ));
    }

    // 主动提醒任务（reminder.enabled 时；间隔取整分钟，至少 1 分钟）
    if cfg.reminder.enabled {
        // 与迁移前逐字同式（`(interval_secs.max(60) / 60) as u64`）；`interval_secs`
        // 本就是 `u64`，clippy `unnecessary_cast` 要求去掉这个恒等转换——语义不变。
        let interval_min = cfg.reminder.interval_secs.max(60) / 60;
        registrations.push(TaskRegistration::new(
            "reminder",
            "主动提醒",
            interval_min * 60,
            Arc::new(
                ReminderTask::new(
                    // 复用共享 ModelServices（不重复创建 HTTP client 池）
                    services.chat.clone(),
                    services.chat_model.clone(),
                    services.notification_sink.clone(),
                    services.session_manager.clone(),
                    cfg.reminder.max_per_run,
                    cfg.reminder.inject_to_session,
                )
                .with_working_sets(services.working_sets.clone()),
            ),
        ));
    }

    registrations
}

#[cfg(test)]
mod tests {
    use super::*;

    use tianyan::common::types::Message;
    use tianyan::config::TianyanConfig;
    use tianyan::db::Database;
    use tianyan::model::{ChatService, MockChatService};
    use tianyan::notification::{NoopNotificationSink, SharedNotificationSink};
    use tianyan::observability::usage_stats::UsageStats;
    use tianyan::session::{Session, SessionManager};
    use tianyan::snapshot::SnapshotManager;

    use async_trait::async_trait;

    /// 空会话管理器（装配期只用其引用，本插件的清单判定不读会话）。
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

    /// 构造快照管理器（临时目录，仅用于「已配置工作区」这一分支）。
    fn make_snapshot_manager() -> (tempfile::TempDir, Arc<SnapshotManager>) {
        let dir = tempfile::tempdir().unwrap();
        let workdir = dir.path().join("work");
        let snap_root = dir.path().join("snapshots");
        std::fs::create_dir_all(&workdir).unwrap();
        (dir, Arc::new(SnapshotManager::new(snap_root, workdir)))
    }

    /// 构造装配能力集合（`snapshot_manager` 与 `reminder` 由调用方指定）。
    async fn make_services(
        config: TianyanConfig,
        snapshot_manager: Option<Arc<SnapshotManager>>,
    ) -> TaskServices {
        let chat: Arc<dyn ChatService> = Arc::new(MockChatService::new());
        let sink: SharedNotificationSink = Arc::new(NoopNotificationSink);
        TaskServices {
            config,
            usage_stats: make_usage_stats().await,
            snapshot_manager,
            session_manager: Arc::new(StubSessionManager),
            notification_sink: sink,
            chat,
            chat_model: "test-model".to_string(),
            working_sets: None,
            // 演化综述执行器（ADR-049 序 5b）由 `tianyan-task-evolution` 消费；
            // 本插件的 5 个任务不需要它。
            evolution_executor: None,
        }
    }

    /// 迁移门禁：注册顺序 + 任务名 + 显示名 + 间隔常量与迁移前逐字一致
    /// （顺序即扫描/执行顺序；`TaskScheduler` 以注册顺序串行执行到期任务）。
    #[tokio::test]
    async fn test_tasks_order_names_and_intervals() {
        let services = make_services(TianyanConfig::default(), None).await;
        let registrations = tasks(&services);

        let observed: Vec<(String, &str, u64)> = registrations
            .iter()
            .map(|r| (r.name.clone(), r.display_name.as_str(), r.interval_secs))
            .collect();
        assert_eq!(
            observed,
            vec![
                ("summary_generation".to_string(), "摘要生成", 6 * 3600),
                ("garbage_collection".to_string(), "垃圾回收", 6 * 3600),
                ("usage_stats_flush".to_string(), "使用统计刷盘", 60),
            ],
            "默认配置下应注册 3 个无条件任务，顺序/显示名/间隔逐字保持"
        );
        // 处理器自报的名字必须与注册名一致（`register_task` 以注册名为 ID，
        // 两者分叉会让状态面板与日志对不上号）。
        for registration in &registrations {
            assert_eq!(
                registration.handler.name(),
                registration.name,
                "处理器 name() 必须等于注册名"
            );
        }
    }

    /// 条件注册：`snapshot_manager` 为 `Some` 时才注册 `snapshot_gc`（12h），
    /// 位置在 `usage_stats_flush` 之后、`reminder` 之前。
    #[tokio::test]
    async fn test_tasks_snapshot_gc_requires_manager() {
        let (_dir, manager) = make_snapshot_manager();
        let services = make_services(TianyanConfig::default(), Some(manager)).await;
        let names: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();

        assert_eq!(
            names,
            vec![
                "summary_generation",
                "garbage_collection",
                "usage_stats_flush",
                "snapshot_gc",
            ],
            "配置了工作区时必须注册 snapshot_gc（未启用提醒时到此为止）"
        );
        let interval = tasks(&services)
            .into_iter()
            .find(|r| r.name == "snapshot_gc")
            .map(|r| r.interval_secs);
        assert_eq!(interval, Some(12 * 3600), "快照 GC 间隔为 12 小时");
    }

    /// 启用条件：`reminder.enabled == false`（默认）时不注册提醒任务。
    #[tokio::test]
    async fn test_tasks_reminder_skipped_when_disabled() {
        let services = make_services(TianyanConfig::default(), None).await;
        assert!(
            tasks(&services).iter().all(|r| r.name != "reminder"),
            "提醒默认关闭，不得注册（迁移前 cfg.reminder.enabled 门控）"
        );
    }

    /// 间隔换算：`interval_secs` 取整分钟且至少 60s（迁移前
    /// `(interval_secs.max(60) / 60) as u64 * 60`）。
    #[tokio::test]
    async fn test_tasks_reminder_interval_rounds_to_minutes_with_floor() {
        for (configured, expected) in [(30u64, 60u64), (90, 60), (3600, 3600), (7200, 7200)] {
            let mut config = TianyanConfig::default();
            config.reminder.enabled = true;
            config.reminder.interval_secs = configured;
            let services = make_services(config, None).await;
            let interval = tasks(&services)
                .into_iter()
                .find(|r| r.name == "reminder")
                .map(|r| r.interval_secs);
            assert_eq!(
                interval,
                Some(expected),
                "配置 {}s 应换算为 {}s（取整分钟 + 至少 60s）",
                configured,
                expected
            );
        }
    }

    /// 注册名唯一：`TaskScheduler::register_task` 对重复 ID 直接报错
    /// （启动失败），因此清单里不得出现重名——含全部可选任务的最坏情况。
    #[tokio::test]
    async fn test_task_names_are_unique_with_all_optional_tasks() {
        let (_dir, manager) = make_snapshot_manager();
        let mut config = TianyanConfig::default();
        config.reminder.enabled = true;
        let services = make_services(config, Some(manager)).await;

        let names: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 5, "全启用时应有 5 个任务：{names:?}");
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "任务名不得重复：{names:?}");
        assert_eq!(
            names,
            vec![
                "summary_generation",
                "garbage_collection",
                "usage_stats_flush",
                "snapshot_gc",
                "reminder",
            ],
            "全启用时的完整注册顺序"
        );
    }

    /// 判别力：`tasks()` 不消费能力集合（`&TaskServices`），可重复调用——
    /// 装配入口在数据目录搬迁重启时会再次装配。
    #[tokio::test]
    async fn test_tasks_is_repeatable() {
        let services = make_services(TianyanConfig::default(), None).await;
        let first: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();
        let second: Vec<String> = tasks(&services).into_iter().map(|r| r.name).collect();
        assert_eq!(first, second);
    }
}
