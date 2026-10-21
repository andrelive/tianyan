//! 任务装配原语（ADR-049 期 2 序 5a）。
//!
//! 内置定时任务的**实现**已搬出 core（`plugins/task-builtin`），但「任务插件如何把
//! 自己的任务交给调度器」这件事必须有类型：装配层（`app` bin / `tauri`）把
//! [`TaskServices`]（能力集合）递给插件，插件返回 [`TaskRegistration`] 清单，server
//! 逐条 `scheduler.register_task(TaskDefinition::new(name, display_name, interval, handler))`。
//!
//! ## 为什么这两个类型住在 core（而不是 `server/src/assembly.rs`）
//!
//! 它们被**三方**同时引用：server（消费清单）、装配包（把插件函数包成类型擦除工厂）、
//! 任务插件（生产清单）。若定义在 server，`plugins/task-builtin` 就必须依赖
//! `tianyan-server` —— 那是 ADR-049 **D2-1「插件只依赖 kernel-api」**的第一条铁律，
//! 也与工具插件的先例不一致（`ToolContext` 同样住在 core：内核定义能力集合、插件消费）。
//! 因此契约面留在内核（D3 的 A 级「装配原语 + 任务契约」、D4 的 K4「任务注册表」），
//! server 侧只把它 re-export 成插槽契约（`tianyan_server::{TaskFactory, TaskRegistration,
//! TaskServices}`）。
//!
//! ## 与 [`TaskHandler`] / [`TaskContext`] 的分工
//!
//! - [`TaskHandler`]：任务的**执行**契约（`execute(&TaskContext)` + `name()`），机制留 core；
//! - [`TaskContext`]：任务**运行时**看到的世界（vfs / summary_engine / session_recall /
//!   config / task_state），由调度器在执行时提供；
//! - [`TaskServices`]：任务**装配时**需要的外部能力（统计、快照、会话、通知、模型、
//!   工作集）——`TaskContext` 不携带这些（它们不是「所有任务都需要的世界」，而是
//!   「装配期一次性注入的依赖」），所以任务实现把它们**存在自己的字段里**；
//! - [`TaskRegistration`]：插件交给调度器的**一条注册**（名字 / 显示名 / 间隔 / 处理器）。
//!
//! 这一层不做任何决策：注册顺序、启用条件、间隔常量全部由插件给出——server 只按
//! 顺序注册，因此「改任务策略/新增任务」只重编插件 + 装配包，不重编 server 库
//! （闸门 A 的前提）。

use std::sync::Arc;

use crate::agent::working_set::WorkingSetRegistry;
use crate::config::TianyanConfig;
use crate::model::ChatService;
use crate::notification::SharedNotificationSink;
use crate::observability::usage_stats::UsageStats;
use crate::scheduler::TaskHandler;
use crate::session::SessionManager;
use crate::snapshot::SnapshotManager;

/// 一条任务注册（任务的**元数据 + 处理器**）。
///
/// 字段语义与 [`crate::scheduler::TaskDefinition`] 的一一对应关系（由装配层照搬，
/// 迁移前后逐字一致）：
///
/// | 本结构 | `TaskDefinition` | 说明 |
/// |---|---|---|
/// | [`name`](Self::name) | `id` | 任务唯一 ID（也是 `name()` 的返回值） |
/// | [`display_name`](Self::display_name) | `name` | 面板/日志里的中文显示名 |
/// | [`interval_secs`](Self::interval_secs) | `interval_secs` | 间隔（秒），间隔制无 cron |
/// | [`handler`](Self::handler) | `handler` | 执行体 |
///
/// 刻意保留两个名字（ID 与显示名）：`TaskDefinition` 的字段命名是历史包袱（`id` 才是
/// 注册 ID），此处用不含歧义的名字，避免「注册了两次同名任务」这类误读。
pub struct TaskRegistration {
    /// 任务唯一 ID（= `TaskHandler::name()`，重复注册会被调度器拒绝）。
    pub name: String,
    /// 任务显示名（中文；状态接口与日志用）。
    pub display_name: String,
    /// 执行间隔（秒）。
    pub interval_secs: u64,
    /// 任务处理器。
    pub handler: Arc<dyn TaskHandler>,
}

impl TaskRegistration {
    /// 构造一条注册（参数顺序与 `TaskDefinition::new` 同形，便于逐行对照）。
    pub fn new(
        name: impl Into<String>,
        display_name: impl Into<String>,
        interval_secs: u64,
        handler: Arc<dyn TaskHandler>,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            interval_secs,
            handler,
        }
    }
}

/// 任务装配所需的外部能力集合（装配层从 `AppState` 组装，工厂调用前 **await 好**）。
///
/// 刻意用**公开字段**（与 [`crate::scheduler::TaskContext`] 同风格）：这是一个纯粹的
/// 数据包，没有不变量需要维护；插件只读，装配层只构造。
///
/// 字段为什么是这些（全部来自「迁移前那 5 段 `register_task` 现场取的东西」）：
///
/// - [`config`](Self::config)：注册期的启用条件与间隔（`reminder.enabled` /
///   `reminder.interval_secs` / `reminder.max_per_run` / `reminder.inject_to_session`、
///   `storage` 的 GC 阈值）。**整份克隆**：任务把它当只读快照，热重载后由下一次装配
///   刷新——与迁移前 `state.config().read().await.clone()` 同口径；
/// - [`usage_stats`](Self::usage_stats)：统计刷盘任务的落库对象（强依赖）；
/// - [`snapshot_manager`](Self::snapshot_manager)：`None` = 未配置工作区 → 插件**跳过**
///   快照 GC（迁移前 `if let Some(sm) = state.snapshot_manager()` 的同一语义）；
/// - [`session_manager`](Self::session_manager)：提醒任务选「最新会话」（只读路径）；
/// - [`notification_sink`](Self::notification_sink)：系统通知通道（桌面端/服务器端的
///   全局注册点，由装配层递入——core 不持有全局单例）；
/// - [`chat`](Self::chat) + [`chat_model`](Self::chat_model)：提醒任务的评估模型。
///   **复用共享 `ModelServices`**（装配层 `AppState::shared_model_services()`），
///   不重复建 HTTP client 池——迁移前就是这么写的，这里只是把同一份引用递进来；
/// - [`working_sets`](Self::working_sets)：提醒注入的唯一写入口（ADR-039 §4）。
///   `None` = 未装配 → 不注入（由 `persist_system_message` 单点告警）。
#[derive(Clone)]
pub struct TaskServices {
    /// 配置快照（注册期判启用条件与间隔；只读）。
    pub config: TianyanConfig,
    /// 使用统计追踪器（内存计数器 → SQLite）。
    pub usage_stats: Arc<UsageStats>,
    /// 工作区快照管理器（`None` = 未配置工作区，跳过快照 GC）。
    pub snapshot_manager: Option<Arc<SnapshotManager>>,
    /// 会话管理器（提醒任务读「最新会话」）。
    pub session_manager: Arc<dyn SessionManager>,
    /// 系统通知通道。
    pub notification_sink: SharedNotificationSink,
    /// 聊天模型服务（提醒任务评估用；共享实例，不重复建客户端池）。
    pub chat: Arc<dyn ChatService>,
    /// 聊天模型名称（提醒任务请求里用的 `model` 字段）。
    pub chat_model: String,
    /// 会话工作集注册表（提醒注入的唯一写入口；`None` = 不注入）。
    pub working_sets: Option<Arc<WorkingSetRegistry>>,
}
