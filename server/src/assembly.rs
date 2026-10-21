//! 装配扩展点（ADR-049）：插件工具清单 + 向量存储实现 + 结构化存储实现 + 内置任务清单
//! + 审批门控实现 + 上下文策略实现。
//!
//! server 库**不认识任何具体插件 crate**：本模块只定义五根**类型擦除**的插槽
//! （以及没有插槽时的降级实现），由装配层（`app` bin / `tauri` 桌面端）在启动
//! 时注入。这样改一行插件实现只重编插件 + 装配包，server 库（13.4K 源码）不动
//! ——闸门 A 的前提。
//!
//! 六个插槽：
//! - [`ToolPluginFactory`]（`crate::ToolPluginFactory`）：工具清单，**可重复调用**
//!   （agent 会因配置热重载 / 模型切换反复重建，每次都要重新注入插件工具）。
//! - [`VectorStorageFactory`]：向量存储实现（`tianyan-storage-vector` 的
//!   LanceDB 后端）。**异步**：LanceDB 建连/建表是 async，且失败要能向上抛（启动
//!   期明确失败优于静默无向量）。
//! - [`StorageBackendFactory`]：VFS 结构化存储实现（`tianyan-storage-sqlite` 的
//!   `SqliteBackend` / `LocalFileBackend`）。**同步**：两个后端的构造都不做 IO
//!   （建表/建目录发生在 `StorageBackend::initialize`）。`config.storage.backend`
//!   的**选择语义**也在工厂里（装配层决定哪个 adapter）——server 库只递配置。
//! - [`TaskFactory`]：定时任务清单（`tianyan-task-builtin` 的 5 个任务 + `tianyan-task-evolution`
//!   的自演化综述，ADR-049 期 2 序 5a / 5b）。**同步**：调用前本库已把 [`TaskServices`]
//!   **await 好**（聊天模型服务是 `AppState` 里共享实例的克隆，工厂里无处可等），
//!   工厂只做「按当前配置构造处理器 + 判定启用条件」。
//! - [`ApprovalGateFactory`]：审批门控实现（`tianyan-policy-approval`，
//!   ADR-049 期 3-T1）。**同步**：`ApprovalWorkflow::new` 只存字段、通知器也只是存
//!   句柄；「`SecurityConfig` → 自动审批规则」的映射随之进插件（单一事实源）。
//! - [`ContextFactory`]：上下文策略实现（`tianyan-policy-context`，ADR-049 期 3-T2）。
//!   **同步**：管线/压缩器/检索器的构造都只存字段（无 IO）；「注入顺序 / 压缩阈值 /
//!   摘要提示词 / 检索参数 / 压缩窗口与模型规格联动」的映射随之进插件（单一事实源）。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use tianyan::config::StorageConfig;
use tianyan::executor::approval::ApprovalGate;
use tianyan::vfs::{
    ContextEntry, StorageBackend, VectorPoint, VectorSearchQuery, VectorSearchResult,
    VectorStorage, VectorType,
};

use crate::ToolPluginFactory;

// 任务清单的**契约面**（ADR-049 期 2 序 5a）：定义在 core 的
// `scheduler::task_assembly`，此处 re-export —— 插件、装配层、本库三方都要引用它，
// 因此不能住在 server（否则任务插件必须依赖 server 库，违反 D2-1「插件只依赖
// kernel-api」，也与 `ToolContext` 住在 core 的先例不一致）。
pub use tianyan::scheduler::{TaskRegistration, TaskServices};
// 上下文策略的**契约面**（ADR-049 期 3-T2）：与任务清单同一条纪律——定义在 core 的
// `context::{port,assembly}`，此处 re-export 成插槽契约。插件、装配层、本库三方
// 都要引用它，住在 server 会让上下文插件反过来依赖 server 库（破 D2-1）。
pub use tianyan::context::{ContextPort, ContextServices};

/// 向量存储工厂（**装配层注入**，期 2 序 3）。
///
/// `Arc<dyn Fn(&StorageConfig) -> Pin<Box<dyn Future<Output = Result<Arc<dyn
/// VectorStorage>>> + Send>>>`：参数是 `&StorageConfig`（借用的，实现内部须自行
/// 克隆需要的数据），返回 boxed future（trait 对象无法用 `async fn`）。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：启动入口可能被复用（数据目录搬迁后
/// 以新配置重启、多端口探测重试），工厂必须是可重复调用的。
pub type VectorStorageFactory = Arc<
    dyn Fn(
            &StorageConfig,
        ) -> Pin<
            Box<dyn std::future::Future<Output = tianyan::Result<Arc<dyn VectorStorage>>> + Send>,
        > + Send
        + Sync,
>;

/// 结构化存储后端工厂（**装配层注入**，期 2 序 4）。
///
/// `Arc<dyn Fn(&StorageConfig, Arc<Database>) -> Result<Arc<dyn StorageBackend>>>`：
/// - 第一个参数是 `&StorageConfig`（借用；`config.storage.backend` 的**选择语义**
///   在这里落地——哪个 adapter 由装配层决定，本库只递配置）；
/// - 第二个参数是**全系统共享的 SQLite 连接**（ADR-005 单连接：`SqliteBackend` 必须
///   把 SQL 打在同一根连接上，不能自开一个库）；
/// - **同步**：`SqliteBackend::new` / `LocalFileBackend::new` 都只存字段不碰 IO
///   （建表 / 建目录在 `StorageBackend::initialize` 里，由 VFS 初始化链调用），
///   因此不需要 [`VectorStorageFactory`] 那样的 boxed future。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：数据目录搬迁后以新配置重启会再次装配。
pub type StorageBackendFactory = Arc<
    dyn Fn(&StorageConfig, Arc<tianyan::db::Database>) -> tianyan::Result<Arc<dyn StorageBackend>>
        + Send
        + Sync,
>;

/// 定时任务工厂（**装配层注入**，期 2 序 5a / 5b）。
///
/// `Arc<dyn Fn(&TaskServices) -> Vec<TaskRegistration>>`：参数是**已 await 好**的
/// 能力集合（[`TaskServices`]：配置快照 + 统计/快照/会话/通知/模型/工作集/演化执行器），
/// 返回按注册顺序排好的任务清单。
///
/// **顺序是清单的属性**：多个任务插件各自的 `tasks()` 只保证自己的内部顺序，**跨插件
/// 的相对位置由装配层拼接决定**（插件之间不互相 import，D2-2）——例如演化任务用
/// `splice(1..1, …)` 紧跟 `summary_generation`。本库只按返回顺序注册。
///
/// **同步**（与 [`StorageBackendFactory`] 同理，与 [`VectorStorageFactory`] 相反）：
/// 工厂里没有需要等的事——聊天模型服务是 `AppState` 里 `ModelServices` 的克隆
/// （共享实例，**不重复建 HTTP client 池**），本库在调工厂前已经取好；工厂只做
/// 「按配置构造处理器 + 判定启用条件」。若把工厂做成 async，调用方就得在
/// `register_task` 的循环里 await 一个纯内存操作，收益为零。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：数据目录搬迁后以新配置重启会再次装配。
pub type TaskFactory = Arc<dyn Fn(&TaskServices) -> Vec<TaskRegistration> + Send + Sync>;

/// 审批门控工厂（**装配层注入**，ADR-049 期 3-T1）。
///
/// `Arc<dyn Fn(&SecurityConfig, Arc<WorkingSetRegistry>, SharedNotificationSink)
///   -> Arc<dyn ApprovalGate>>`：
/// - 第一个参数是 `&SecurityConfig`（借用）：`approval_mode` / `blocked_commands` /
///   `allowed_commands` / `prompt_commands` → 自动审批规则 + 「总是询问」列表的映射
///   在**插件**里（`tianyan_policy_approval::gate`），本库只递配置——这样「配置项怎么
///   变成规则」只有一个事实源，三处装配不必各抄一份；
/// - 第二个参数是**会话工作集**（ADR-039：审批挂起通知落库的唯一写入口）——必须是
///   `Agent` 用的**同一个** `Arc`，否则审批提示会写进另一份状态（`AppState` 组合根
///   保证是同一实例）；
/// - 第三个参数是系统通知通道（挂起时发桌面通知；无人值守场景即时提醒）。
///
/// **同步**（与 [`StorageBackendFactory`] / [`TaskFactory`] 同理）：构造只存字段与
/// 句柄，没有任何 IO，做成 async 只会让调用方多 await 一个纯内存操作。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：配置热重载会重建 agent（`reload_agent`），
/// 审批门控随之重建——规则必须跟着**新配置**走，不能沿用旧实例。
pub type ApprovalGateFactory = Arc<
    dyn Fn(
            &tianyan::config::SecurityConfig,
            Arc<tianyan::agent::working_set::WorkingSetRegistry>,
            tianyan::notification::SharedNotificationSink,
        ) -> Arc<dyn ApprovalGate>
        + Send
        + Sync,
>;

/// 上下文策略工厂（**装配层注入**，ADR-049 期 3-T2）。
///
/// `Arc<dyn Fn(&ContextServices) -> Arc<dyn ContextPort>>`：参数是装配层填好的
/// **能力集合**（[`ContextServices`]：配置快照 + VFS + 统计 + 摘要模型客户端 +
/// 模型名 + 模型规格），返回值是内核唯一认识的上下文端口。
///
/// 为什么把「怎么配」放进工厂而不是留在本库：
/// - `context_window` 与模型规格的联动、`preserve_recent_messages = 6`、
///   `summary_model` 的取值、`default_top_k` / `learned_rules_top_k` 的读取，迁移前
///   散在 `AgentBuilder::build`（core）与 `server/src/agent_builder.rs`（本库）；
/// - 现在它们是**压缩/检索策略**（B 级，随插件自由演进），因此映射集中在
///   `tianyan_policy_context::pipeline`——三处装配各写一行包装，不会各抄一份
///   （与 [`ApprovalGateFactory`] 同一手法）。
///
/// **同步**（与 [`StorageBackendFactory`] / [`TaskFactory`] / [`ApprovalGateFactory`]
/// 同理）：`ContextPipeline::new` / `ContextCompressor::new` / `DualLayerRetriever::new`
/// 都只存字段，没有任何 IO，做成 async 只会让调用方多 await 一个纯内存操作。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：配置热重载会重建 agent
/// （`AppState::reload_agent`），压缩窗口必须跟着**新配置/新模型规格**走。
pub type ContextFactory = Arc<dyn Fn(&ContextServices) -> Arc<dyn ContextPort> + Send + Sync>;

/// 启装配所需的外部能力集合（`None` = 该扩展点未装配，走降级路径）。
///
/// 目前六个插槽：插件工具清单（[`ToolPluginFactory`]）、向量存储
/// （[`VectorStorageFactory`]）、结构化存储（[`StorageBackendFactory`]）、
/// 内置任务清单（[`TaskFactory`]）、审批门控（[`ApprovalGateFactory`]）、
/// 上下文策略（[`ContextFactory`]）。用
/// **一个结构**而不是给每个启动入口再加一个参数：入口变体的数量是组合爆炸
/// （listener × 关闭信号 × 插件 × 向量 × 存储 × 任务），打包后只需「默认装配」与
/// 「装配齐全」两类入口。
#[derive(Clone, Default)]
pub struct AssemblyServices {
    plugin_factory: Option<ToolPluginFactory>,
    vector_factory: Option<VectorStorageFactory>,
    storage_factory: Option<StorageBackendFactory>,
    task_factory: Option<TaskFactory>,
    approval_factory: Option<ApprovalGateFactory>,
    context_factory: Option<ContextFactory>,
}

impl AssemblyServices {
    /// 空装配（`create_app` / 嵌入式 / 测试路径：无插件工具、无持久化向量存储）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 装配插件工具清单工厂（`app` / `tauri` 用）。
    pub fn with_plugin_factory(mut self, factory: ToolPluginFactory) -> Self {
        self.plugin_factory = Some(factory);
        self
    }

    /// 装配向量存储工厂（`app` / `tauri` 用：LanceDB）。
    pub fn with_vector_factory(mut self, factory: VectorStorageFactory) -> Self {
        self.vector_factory = Some(factory);
        self
    }
    /// 装配结构化存储工厂（`app` / `tauri` 用：SQLite / 本地文件系统）。
    pub fn with_storage_factory(mut self, factory: StorageBackendFactory) -> Self {
        self.storage_factory = Some(factory);
        self
    }

    /// 装配定时任务工厂（`app` / `tauri` 用：`tianyan-task-builtin` + `tianyan-task-evolution`）。
    pub fn with_task_factory(mut self, factory: TaskFactory) -> Self {
        self.task_factory = Some(factory);
        self
    }

    /// 装配审批门控工厂（`app` / `tauri` 用：`tianyan-policy-approval`）。
    pub fn with_approval_factory(mut self, factory: ApprovalGateFactory) -> Self {
        self.approval_factory = Some(factory);
        self
    }

    /// 装配上下文策略工厂（`app` / `tauri` 用：`tianyan-policy-context`）。
    pub fn with_context_factory(mut self, factory: ContextFactory) -> Self {
        self.context_factory = Some(factory);
        self
    }

    /// 插件工具清单工厂（`None` = 不注入插件工具）。
    pub(crate) fn plugin_factory(&self) -> Option<ToolPluginFactory> {
        self.plugin_factory.clone()
    }
    /// 审批门控工厂（`None` = 不装配审批门控）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**（`None` 继续往下传）：不装配审批门控，
    /// `ToolContext::ensure_approved` 走既有的「未装配直接放行」分支，
    /// `AgentBuilder::build` 打一条 `warn!` 指路。这比存储降级（`error!`，
    /// 「重启即丢数据」）轻一个量级：默认配置就是 `ApprovalMode::Autonomous`
    /// （黑名单外全放行，ADR-033），且命令黑名单在 `SecurityPolicy` 守卫层**另有**
    /// 一份拦截；而 `create_app`（构造对象图，47 处 lib 单测/嵌入式）不要求装配，
    /// 硬失败等于删入口。
    pub(crate) fn approval_factory(&self) -> Option<ApprovalGateFactory> {
        self.approval_factory.clone()
    }

    /// 上下文策略工厂（`None` = 不装配上下文策略）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**（`None` 继续往下传）：`AgentBuilder::build` 用 core 的
    /// `NullContextPort` 兜底——`load_injectable` 返回空可注入上下文（模型**没有**
    /// soul / 项目说明 / learned rules / memories 前缀）、`compress_for_session` 恒
    /// `None`（上下文永不压缩），并 `warn!` 一条指路。
    ///
    /// 与审批降级（T1）的**差异**：`start_server*` 入口对上下文策略设了**启动期硬前置**
    /// （[`Self::has_context_factory`]，见 `start_server_inner`）——审批缺失只是「少了
    /// 一层策略复核」，而上下文缺失是「整个 system 前缀没了 + 上下文只增不减」，服务虽然
    /// 起得来但行为已不成立。`create_app`（构造对象图，47 处 lib 单测/嵌入式）仍允许降级。
    pub(crate) fn context_factory(&self) -> Option<ContextFactory> {
        self.context_factory.clone()
    }

    /// 是否装配了上下文策略工厂（**启动服务的硬前置**，见 `start_server_inner`）。
    ///
    /// 与 [`Self::has_storage_factory`] 同一手法：`create_app`（测试/嵌入式构造对象图）
    /// 允许空直通兜底，`start_server*`（真跑服务）必须拒绝——两条路径严格度不同，
    /// 判定点因此必须能被调用方分别选用。
    pub(crate) fn has_context_factory(&self) -> bool {
        self.context_factory.is_some()
    }

    /// 是否装配了结构化存储工厂（**启动服务的硬前置**，见 `start_server_inner`）。
    ///
    /// 为什么单独开一个查询方法而不是让 `storage_backend()` 自己报错：
    /// `create_app`（测试/嵌入式构造对象图）需要允许内存兜底，而
    /// `start_server*`（真跑服务）必须拒绝——两条路径的严格度不同，判定点必须能
    /// 被调用方分别选用。守卫放在 `start_server_inner` 入口而不是工厂里，是为了让
    /// 失败发生在**启动前**（清晰报错）而不是运行期第一次写入时（数据已丢）。
    pub(crate) fn has_storage_factory(&self) -> bool {
        self.storage_factory.is_some()
    }

    /// 构造 VFS 结构化存储后端实例（**唯一的存储实现选择点**）。
    ///
    /// 未装配工厂时返回 [`EphemeralStorageBackend`]（**进程内存**：内容读写正常
    /// 但**不落盘**，重启即丢）。为什么是兜底而不是 `Err`——两个理由：
    ///
    /// 1. **旧入口不能坏**：`create_app` / `start_server*` 这组不带装配的入口是
    ///    公开 API，约 60 处测试与嵌入式调用靠它起一个能跑的实例（ADR-049 序 3
    ///    明确「旧签名全部保留且语义等价」）。存储是**启动硬依赖**，返回 `Err`
    ///    会让这批入口在**任何**构建里都起不来——那等于删掉它们，而不是收敛装配。
    /// 2. **实现拿不到**：`LocalFileBackend` / `SqliteBackend` 已在插件里
    ///    （`tianyan-storage-sqlite`），本库**不能** `use` 它；兜底只能在 server
    ///    内自带。写一份「本地文件/SQLite 的复制品」是两份实现同时漂移，比内存
    ///    兜底危险得多。
    ///
    /// **代价必须可见**（ADR-038 可见失败，比序 3 的向量降级更严）：装配期
    /// `error!` 一条、首次写入再 `error!` 一次（`atomic` 只打一次）。序 3 的向量
    /// 降级只是「没有语义检索」，这里降级是「**VFS 内容不落盘**」——生产误配的
    /// 症状会是「重启后数据全部消失」，所以用 `error` 级而不是 `warn`。
    pub(crate) fn storage_backend(
        &self,
        config: &StorageConfig,
        database: Arc<tianyan::db::Database>,
    ) -> tianyan::Result<Arc<dyn StorageBackend>> {
        match self.storage_factory.as_ref() {
            Some(factory) => factory(config, database),
            None => {
                error!(
                    backend = ?config.backend,
                    "未装配结构化存储工厂：VFS 使用**进程内存**存储（内容不落盘，重启即丢）——\
                     生产装配请用 create_app_with_assembly / start_server_with_assembly"
                );
                Ok(Arc::new(EphemeralStorageBackend::new()))
            }
        }
    }

    /// 构造向量存储实例（**唯一的向量实现选择点**）。
    ///
    /// 未装配工厂时返回 [`NoopVectorStorage`]：VFS 仍可工作（内容读写/摘要/工具
    /// 全部正常），只是**没有**语义检索。这条路径服务于测试与嵌入式调用
    /// （`create_app`）——生产装配（`app` / `tauri`）必须注入工厂，否则启动日志
    /// 会有明确告警。
    pub(crate) async fn vector_storage(
        &self,
        config: &StorageConfig,
    ) -> tianyan::Result<Arc<dyn VectorStorage>> {
        match self.vector_factory.as_ref() {
            Some(factory) => factory(config).await,
            None => {
                warn!(
                    "未装配向量存储工厂：VFS 使用空向量存储（无语义检索，向量写入被丢弃）——\
                     生产装配请用 create_app_with_assembly / start_server_with_assembly"
                );
                Ok(Arc::new(NoopVectorStorage::new(config)))
            }
        }
    }

    /// 构造定时任务清单（**唯一的任务实现选择点**）。
    ///
    /// 未装配工厂时返回**空清单**（不注册任何内置任务）+ `warn!` 一次指路。
    /// 为什么是「空清单」而不是 `Err`：
    ///
    /// 1. **旧入口不能坏**：`start_server*` 这组不带装配的公开入口是「保留且语义
    ///    等价」的 API 面（序 3 的纪律），`Err` 会让这批入口在任何构建里都起不来；
    /// 2. **任务不是启动硬依赖**：它们是周期后台维护（摘要 / GC / 统计刷盘 /
    ///    快照 GC / 提醒），缺了服务照常收发消息——只是维护动作不跑。对比序 4 的
    ///    存储降级（`error!`，「重启即丢数据」），这里的代价低一个量级，用 `warn!`；
    /// 3. **必须留痕**（ADR-038 可见失败）：静默不注册会让生产误配的唯一症状变成
    ///    「摘要永远不补、统计数据不落库」，看不出是降级还是数据缺失。
    ///
    /// 语义差异须知（迁移前后唯一的行为差）：迁移前 `start_server*`（不带装配）
    /// **会**注册这些任务（5 个内置 + 演化）；现在只有注入 [`TaskFactory`] 的装配
    /// （`app` / `tauri` / 集成测试的 `test_assembly()`）才注册。`create_app` 路径迁移前后都**不**注册
    /// 任务（任务注册在 `start_server_inner` 里），因此 47 处 `create_app` 单测的
    /// 可观测行为不变。
    pub(crate) fn tasks(&self, services: &TaskServices) -> Vec<TaskRegistration> {
        match self.task_factory.as_ref() {
            Some(factory) => factory(services),
            None => {
                warn!(
                    "未装配任务工厂：不注册任何定时任务（摘要生成 / 垃圾回收 / 统计刷盘 / \
                     快照 GC / 主动提醒 / 自演化综述）——生产装配请用 \
                     create_app_with_assembly / start_server_with_assembly"
                );
                Vec::new()
            }
        }
    }
}

/// 空向量存储（未装配 [`VectorStorageFactory`] 时的降级实现）。
///
/// 语义：**写入丢弃、检索恒空、行数恒 0**。存在的意义是让 VFS 的依赖
/// （`Arc<dyn VectorStorage>`）在测试/嵌入式路径上可满足，而不必把 LanceDB 这类
/// 重实现拖回 core 或引入 `Option` 分支污染整个 VFS。
///
/// 不持久化任何东西：`create_app`（无装配）跑出来的实例没有语义检索能力，这是
/// **有意的**——真装配由装配层注入（[`AssemblyServices::with_vector_factory`]）。
struct NoopVectorStorage {
    embedding_dim: usize,
    /// 首次向量写入时告警一次。
    ///
    /// `upsert_point` 丢弃数据是**静默**的：若不留下证据，生产误配的唯一表现会是
    /// 「语义检索永远为空」，看不出是降级还是数据缺失（ADR-038 可见失败）。启动期
    /// 的 warn 只能覆盖「进程起动那一刻在读日志的人」，本字段把证据补到**真正
    /// 丢数据的那一刻**，且只打一次（向量写入是热路径，不能刷屏）。
    upsert_warned: AtomicBool,
}

impl NoopVectorStorage {
    /// 维度取配置值（保持 `embedding_dim()` 与配置一致，避免下游按 0 维构造请求）。
    fn new(config: &StorageConfig) -> Self {
        Self {
            embedding_dim: config.vector.vector_dimension,
            upsert_warned: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl VectorStorage for NoopVectorStorage {
    fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    async fn initialize(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn upsert_point(&self, _point: &VectorPoint) -> tianyan::Result<()> {
        if !self.upsert_warned.swap(true, Ordering::Relaxed) {
            warn!(
                "空向量存储**丢弃**了向量写入（该实例无语义检索能力）：装配层未注入 \
                 VectorStorageFactory —— 生产装配请用 start_server_with_assembly / \
                 create_app_with_assembly（本告警只打一次）"
            );
        }
        Ok(())
    }

    async fn delete_point(&self, _id: &str) -> tianyan::Result<()> {
        Ok(())
    }

    async fn search(&self, _query: VectorSearchQuery) -> tianyan::Result<Vec<VectorSearchResult>> {
        Ok(Vec::new())
    }

    async fn get_point(&self, _id: &str) -> tianyan::Result<Option<VectorPoint>> {
        Ok(None)
    }

    async fn search_fused(
        &self,
        _query_vector: Vec<f32>,
        _vector_types: &[VectorType],
        _top_k: usize,
        _category_filter: Option<&str>,
        _min_score: Option<f32>,
    ) -> tianyan::Result<Vec<VectorSearchResult>> {
        Ok(Vec::new())
    }
}

/// 进程内存存储后端（未装配 [`StorageBackendFactory`] 时的降级实现）。
///
/// 语义**照 `SqliteBackend` 抄**：键是 URI，`is_directory` 是条目自有字段，删除与
/// 列举都用「URI 后接斜杠」的字符串前缀。两者唯一的差别是「不落盘」。刻意不实现成
/// 「本地文件后端」——那会与插件里的 `LocalFileBackend` 争同一批路径约定，且形态
/// 语义（条目是文件还是目录）与生产默认后端 `SqliteBackend` 不一致，正是 ADR-049
/// 反复强调的「Mock 形态掩盖缺陷」的坑。
///
/// 它服务于 `create_app` / `start_server*` 这组**不带装配**的入口（测试与嵌入式，
/// 见 [`AssemblyServices::storage_backend`]）。**没有任何持久化承诺**：进程退出即
/// 全部消失——这不是缺陷而是刻意的（写 `error!` 留证据，不静默丢数据）。
///
/// `pub(crate)` 只为**本库的启动守卫测试**：`lib.rs::tests` 要构造「已装配存储、未装配
/// 上下文策略」的装配来锁住第二道守卫，而它不需要一个真的落盘后端（守卫在 `create_app`
/// 之前就返回了）。不对外暴露（`pub(crate)`，`tianyan_server` 的公开面仍然只有工厂类型）。
pub(crate) struct EphemeralStorageBackend {
    /// 键 = `uri.to_string()`（与 `vfs_entries` 主键同口径）。
    entries: tokio::sync::RwLock<std::collections::HashMap<String, ContextEntry>>,
    /// 首次「本应落盘」的写入时告警一次（写入是热路径，不能刷屏）。
    write_warned: AtomicBool,
}

impl EphemeralStorageBackend {
    /// 构造空的进程内存后端（语义见类型文档）。
    pub(crate) fn new() -> Self {
        Self {
            entries: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            write_warned: AtomicBool::new(false),
        }
    }

    /// 首次写入时补一条证据：装配期的 `error!` 只覆盖「起进程那一刻在读日志的人」。
    fn warn_write_once(&self) {
        if !self.write_warned.swap(true, Ordering::Relaxed) {
            error!(
                "内存存储后端**未落盘**地写入了 VFS 条目（进程退出即丢）：装配层未注入 \
                 StorageBackendFactory —— 生产装配请用 start_server_with_assembly / \
                 create_app_with_assembly（本告警只打一次）"
            );
        }
    }

    /// 取某层级字段的可变引用（`ContentLevel` → `ContextEntry` 字段的单点映射）。
    fn level_mut(
        entry: &mut ContextEntry,
        level: tianyan::common::types::ContentLevel,
    ) -> &mut Option<String> {
        use tianyan::common::types::ContentLevel;
        match level {
            ContentLevel::Abstract => &mut entry.abstract_content,
            ContentLevel::Overview => &mut entry.overview_content,
            ContentLevel::Detail => &mut entry.detail_content,
        }
    }
}

#[async_trait]
impl StorageBackend for EphemeralStorageBackend {
    async fn initialize(&self) -> tianyan::Result<()> {
        Ok(())
    }

    async fn exists(&self, uri: &tianyan::common::types::TianyanUri) -> tianyan::Result<bool> {
        Ok(self.entries.read().await.contains_key(&uri.to_string()))
    }

    async fn read_entry(
        &self,
        uri: &tianyan::common::types::TianyanUri,
    ) -> tianyan::Result<ContextEntry> {
        self.entries
            .read()
            .await
            .get(&uri.to_string())
            .cloned()
            .ok_or_else(|| tianyan::TianyanError::not_found(uri.to_string()))
    }

    async fn write_entry(&self, entry: &ContextEntry) -> tianyan::Result<()> {
        self.warn_write_once();
        self.entries
            .write()
            .await
            .insert(entry.uri().to_string(), entry.clone());
        Ok(())
    }

    async fn delete_entry(&self, uri: &tianyan::common::types::TianyanUri) -> tianyan::Result<()> {
        self.warn_write_once();
        let key = uri.to_string();
        // 子条目前缀：URI 后接斜杠——与 SqliteBackend 的 substr/length 同口径
        // （只匹配直接/间接子条目，不误删同前缀兄弟 ab 与 abc）。
        let dir_prefix = format!("{}/", key);
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|k, _| k != &key && !k.starts_with(&dir_prefix));
        if entries.len() == before {
            return Err(tianyan::TianyanError::not_found(key));
        }
        Ok(())
    }

    async fn list_directory(
        &self,
        uri: &tianyan::common::types::TianyanUri,
    ) -> tianyan::Result<Vec<ContextEntry>> {
        let prefix = format!("{}/", uri);
        let entries = self.entries.read().await;
        Ok(entries
            .iter()
            .filter_map(|(key, entry)| {
                let rest = key.strip_prefix(&prefix)?;
                // 只列一级子条目：前缀之后不得再含 '/'
                if rest.contains('/') {
                    return None;
                }
                Some(entry.clone())
            })
            .collect())
    }

    async fn read_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
    ) -> tianyan::Result<String> {
        let entries = self.entries.read().await;
        let missing = || tianyan::TianyanError::not_found(format!("{uri} 无 {level:?} 层级内容"));
        let mut entry = entries.get(&uri.to_string()).cloned().ok_or_else(missing)?;
        Self::level_mut(&mut entry, level)
            .clone()
            .ok_or_else(missing)
    }

    async fn write_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
        content: &str,
    ) -> tianyan::Result<()> {
        self.warn_write_once();
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(uri.to_string())
            .or_insert_with(|| ContextEntry::new_file(uri.clone()));
        *Self::level_mut(entry, level) = Some(content.to_string());
        entry.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }

    async fn append_content(
        &self,
        uri: &tianyan::common::types::TianyanUri,
        level: tianyan::common::types::ContentLevel,
        content: &str,
    ) -> tianyan::Result<()> {
        self.warn_write_once();
        let mut entries = self.entries.write().await;
        let entry = entries
            .entry(uri.to_string())
            .or_insert_with(|| ContextEntry::new_file(uri.clone()));
        let slot = Self::level_mut(entry, level);
        match slot {
            Some(existing) => existing.push_str(content),
            None => *slot = Some(content.to_string()),
        }
        entry.metadata.updated_at = chrono::Utc::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未装配工厂时降级为空向量存储：VFS 不会因此构建失败，但检索恒空。
    #[tokio::test]
    async fn test_default_assembly_yields_noop_vector_storage() {
        let config = StorageConfig {
            vector: tianyan::config::VectorStorageConfig {
                collection_name: "noop_test".to_string(),
                vector_dimension: 8,
            },
            ..Default::default()
        };
        let storage = AssemblyServices::new()
            .vector_storage(&config)
            .await
            .unwrap();
        assert_eq!(storage.embedding_dim(), 8);
        assert_eq!(storage.count_rows().await.unwrap(), 0);
        assert!(storage.list_point_ids().await.unwrap().is_empty());
    }

    /// 装配了工厂时必须**只用工厂**（降级路径不得抢占）。
    #[tokio::test]
    async fn test_assembly_uses_injected_vector_factory() {
        let factory: VectorStorageFactory = Arc::new(|config| {
            let dim = config.vector.vector_dimension;
            Box::pin(async move {
                let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                    embedding_dim: dim + 1,
                    upsert_warned: AtomicBool::new(false),
                });
                Ok(storage)
            })
        });
        let config = StorageConfig {
            vector: tianyan::config::VectorStorageConfig {
                collection_name: "injected".to_string(),
                vector_dimension: 8,
            },
            ..Default::default()
        };
        let storage = AssemblyServices::new()
            .with_vector_factory(factory)
            .vector_storage(&config)
            .await
            .unwrap();
        assert_eq!(
            storage.embedding_dim(),
            9,
            "应使用注入的工厂（维度 +1 是工厂内的判别标记）"
        );
    }

    /// 工厂必须可重复调用（启动入口会被数据目录搬迁后的重启复用）。
    #[tokio::test]
    async fn test_vector_factory_is_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: VectorStorageFactory = Arc::new(move |_config| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let storage: Arc<dyn VectorStorage> = Arc::new(NoopVectorStorage {
                    embedding_dim: 4,
                    upsert_warned: AtomicBool::new(false),
                });
                Ok(storage)
            })
        });
        let assembly = AssemblyServices::new().with_vector_factory(factory);
        let config = StorageConfig::default();
        let _ = assembly.vector_storage(&config).await.unwrap();
        let _ = assembly.vector_storage(&config).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );
    }

    /// 空向量存储必须在**首次写入**时留下丢弃证据。
    ///
    /// 判别力：若删掉 `upsert_point` 里的 `swap`，生产误配的唯一表现就是「检索恒空」，
    /// 日志无任何线索（ADR-038 可见失败）；本测试锁住「写入路径有痕迹」。
    #[tokio::test]
    async fn test_noop_vector_storage_warns_on_first_upsert_only() {
        let config = StorageConfig::default();
        let storage = NoopVectorStorage::new(&config);
        let point = VectorPoint {
            schema_version: 1,
            id: "p1".to_string(),
            abstract_vector: Some(vec![0.0; 4]),
            overview_vector: None,
            visual_vector: None,
            payload: tianyan::common::types::EntryMetadata::default(),
        };
        // 丢弃不等于失败：写入必须仍然 Ok，否则会把「降级」变成「报错」。
        for _ in 0..3 {
            storage.upsert_point(&point).await.expect("丢弃写入不报错");
        }
        assert!(
            storage.upsert_warned.load(Ordering::Relaxed),
            "首次写入后须置起告警标志（证据链）；随后不再重复打日志（热路径不刷屏）"
        );
        assert_eq!(storage.count_rows().await.expect("计数"), 0);
    }

    // ── 结构化存储插槽（ADR-049 期 2 序 4）──────────────────────────────

    /// 未装配工厂时给的是**可用但不落盘**的内存后端：VFS 因此仍能读写（不 panic、
    /// 不阻断启动），但必须留下「未落盘」的证据。
    ///
    /// 判别力：若把兜底改成 `Err`，本测试的 `storage_backend` 直接失败；若把
    /// `warn_write_once` 删了，`write_warned` 断言必红（生产误配将完全无痕）。
    #[tokio::test]
    async fn test_default_assembly_storage_is_ephemeral_but_functional() {
        let config = StorageConfig::default();
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        let storage = AssemblyServices::new()
            .storage_backend(&config, database)
            .expect("未装配时应给内存兜底而不是报错");

        let uri = tianyan::TianyanUri::new(
            tianyan::ContextNamespace::Knowledge,
            vec!["probe.md".to_string()],
        );
        storage
            .write_content(&uri, tianyan::ContentLevel::Detail, "临时内容")
            .await
            .expect("兜底后端应可写入");
        assert_eq!(
            storage
                .read_content(&uri, tianyan::ContentLevel::Detail)
                .await
                .expect("回读"),
            "临时内容"
        );
        assert!(storage.exists(&uri).await.expect("存在性"));

        // 首次写入必须留下「未落盘」证据（直接构造以便检查私有标志）
        let direct = EphemeralStorageBackend::new();
        assert!(!direct.write_warned.load(Ordering::Relaxed));
        direct
            .write_content(&uri, tianyan::ContentLevel::Detail, "x")
            .await
            .expect("写入");
        assert!(
            direct.write_warned.load(Ordering::Relaxed),
            "首次写入后须置起告警标志（不静默丢数据）"
        );
    }

    /// 装配了工厂时**只用工厂**（兜底不得抢占）。
    ///
    /// 判别力：工厂返回的后端里预置了一个条目，返回值若能读到它就证明走的是工厂。
    #[tokio::test]
    async fn test_assembly_uses_injected_storage_factory() {
        let marker = tianyan::TianyanUri::new(
            tianyan::ContextNamespace::Knowledge,
            vec!["injected.md".to_string()],
        );
        let injected = marker.clone();
        let factory: StorageBackendFactory = Arc::new(move |_config, _database| {
            let backend = EphemeralStorageBackend::new();
            // 同步工厂：先写进一个预置条目（无需 async 上下文）
            let entry = ContextEntry::new_file(injected.clone());
            backend
                .entries
                .try_write()
                .expect("未竞争")
                .insert(entry.uri().to_string(), entry);
            Ok(Arc::new(backend) as Arc<dyn StorageBackend>)
        });
        let config = StorageConfig::default();
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        let storage = AssemblyServices::new()
            .with_storage_factory(factory)
            .storage_backend(&config, database)
            .expect("装配后应成功");
        assert!(
            storage.exists(&marker).await.expect("存在性"),
            "应使用注入的工厂（预置条目是工厂内的判别标记）"
        );
    }

    /// 工厂必须可重复调用（数据目录搬迁后的重启会再次装配）。
    #[tokio::test]
    async fn test_storage_factory_is_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: StorageBackendFactory = Arc::new(move |_config, _database| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(EphemeralStorageBackend::new()) as Arc<dyn StorageBackend>)
        });
        let assembly = AssemblyServices::new().with_storage_factory(factory);
        let config = StorageConfig::default();
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        let _ = assembly.storage_backend(&config, database.clone()).unwrap();
        let _ = assembly.storage_backend(&config, database).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );
    }

    /// 内存兜底的条目语义必须与 `SqliteBackend` 同口径（否则兜底会引入只属于测试的
    /// 行为差异，重演「Mock 形态掩盖缺陷」）：删除按「URI 后接斜杠」前缀级联、
    /// 不误删同前缀兄弟；列举只给一级子条目。
    #[tokio::test]
    async fn test_ephemeral_storage_prefix_semantics_match_sqlite() {
        let backend = EphemeralStorageBackend::new();
        let ns = tianyan::ContextNamespace::Knowledge;
        let parent = tianyan::TianyanUri::new(ns, vec!["ab".to_string()]);
        let sibling = tianyan::TianyanUri::new(ns, vec!["abc".to_string()]);
        let child = tianyan::TianyanUri::new(ns, vec!["ab".to_string(), "x.md".to_string()]);
        let grandchild = tianyan::TianyanUri::new(
            ns,
            vec!["ab".to_string(), "deep".to_string(), "y.md".to_string()],
        );
        for uri in [&parent, &sibling, &child, &grandchild] {
            backend
                .write_entry(&ContextEntry::new_file(uri.clone()))
                .await
                .expect("写入");
        }

        // 列举只给一级子条目（孙条目被过滤）
        let listed = backend.list_directory(&parent).await.expect("列举");
        assert_eq!(listed.len(), 1, "只应列出一级子条目");

        backend.delete_entry(&parent).await.expect("删除");
        assert!(!backend.exists(&parent).await.expect("存在性"));
        assert!(
            !backend.exists(&child).await.expect("存在性"),
            "子条目应被级联删除"
        );
        assert!(
            !backend.exists(&grandchild).await.expect("存在性"),
            "孙条目应被级联删除"
        );
        assert!(
            backend.exists(&sibling).await.expect("存在性"),
            "同前缀兄弟条目（ab vs abc）不应被误删"
        );
    }

    // ── 内置任务插槽（ADR-049 期 2 序 5a）────────────────────────────────

    /// 只用于装配测试的聊天服务桩（本库的测试不发起任何真实请求）。
    struct StubChatService;

    #[async_trait]
    impl tianyan::model::ChatService for StubChatService {
        async fn chat_completion(
            &self,
            _request: tianyan::model::types::ChatCompletionRequest,
        ) -> tianyan::Result<tianyan::model::types::ChatCompletionResponse> {
            Err(tianyan::TianyanError::Custom(
                "测试桩：不发起聊天请求".to_string(),
            ))
        }

        async fn chat_completion_stream(
            &self,
            _request: tianyan::model::types::ChatCompletionRequest,
        ) -> tianyan::Result<
            tokio::sync::mpsc::Receiver<
                tianyan::Result<tianyan::model::types::ChatCompletionChunk>,
            >,
        > {
            Err(tianyan::TianyanError::Custom(
                "测试桩：不发起流式聊天请求".to_string(),
            ))
        }
    }

    /// 只用于装配测试的会话管理器桩（任务清单判定不读会话）。
    struct StubSessionManager;

    #[async_trait]
    impl tianyan::session::SessionManager for StubSessionManager {
        async fn create_session(
            &self,
            id: &str,
            _message: tianyan::Message,
        ) -> tianyan::Result<tianyan::session::Session> {
            Ok(tianyan::session::Session::new(id))
        }

        async fn get_session(
            &self,
            _id: &str,
        ) -> tianyan::Result<Option<tianyan::session::Session>> {
            Ok(None)
        }

        async fn list_sessions(&self) -> tianyan::Result<Vec<tianyan::session::Session>> {
            Ok(Vec::new())
        }
    }

    /// 只用于装配测试的任务处理器（自报名字由工厂给定）。
    struct StubTask(&'static str);

    #[async_trait]
    impl tianyan::scheduler::TaskHandler for StubTask {
        async fn execute(
            &self,
            _ctx: &tianyan::scheduler::TaskContext,
        ) -> tianyan::scheduler::TaskResult {
            tianyan::scheduler::TaskResult::success(0)
        }

        fn name(&self) -> &str {
            self.0
        }
    }

    /// 构造任务装配能力集合（`snapshot_manager` 固定 `None`：本库的插槽测试不关心它；
    /// `evolution_executor` 同理，清单判定只用到它是否为 `Some`）。
    async fn make_task_services() -> TaskServices {
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        database.init_schemas().await.expect("建表");
        let usage_stats =
            tianyan::observability::usage_stats::UsageStats::new(database).expect("统计");
        TaskServices {
            config: tianyan::config::TianyanConfig::default(),
            usage_stats,
            snapshot_manager: None,
            session_manager: Arc::new(StubSessionManager),
            notification_sink: Arc::new(tianyan::notification::NoopNotificationSink),
            chat: Arc::new(StubChatService),
            chat_model: "stub-model".to_string(),
            working_sets: None,
            evolution_executor: None,
        }
    }

    /// 未装配工厂时**不注册任何任务**（空清单），且不 panic、不报错。
    ///
    /// 判别力：若把降级改成 `Err`，`start_server*` 这组不带装配的公开入口在所有构建里
    /// 都起不来（序 3/4 的同类取舍）；若改成「兜底注册一份内置清单」，本库就得知道
    /// 具体任务（D2-3 破）。
    #[tokio::test]
    async fn test_default_assembly_yields_no_tasks() {
        let services = make_task_services().await;
        let registrations = AssemblyServices::new().tasks(&services);
        assert!(
            registrations.is_empty(),
            "未装配任务工厂时应返回空清单（不注册内置任务），实际 {} 条",
            registrations.len()
        );
    }

    /// 装配了工厂时**只用工厂**（空清单不得抢占）。
    #[tokio::test]
    async fn test_assembly_uses_injected_task_factory() {
        let factory: TaskFactory = Arc::new(|_services| {
            vec![TaskRegistration::new(
                "injected_task",
                "注入的测试任务",
                42,
                Arc::new(StubTask("injected_task")),
            )]
        });
        let services = make_task_services().await;
        let registrations = AssemblyServices::new()
            .with_task_factory(factory)
            .tasks(&services);

        assert_eq!(registrations.len(), 1, "应返回工厂给的清单");
        assert_eq!(registrations[0].name, "injected_task");
        assert_eq!(registrations[0].display_name, "注入的测试任务");
        assert_eq!(registrations[0].interval_secs, 42);
        assert_eq!(registrations[0].handler.name(), "injected_task");
    }

    /// 工厂必须可重复调用（数据目录搬迁后的重启会再次装配）。
    #[tokio::test]
    async fn test_task_factory_is_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: TaskFactory = Arc::new(move |_services| {
            counter.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        });
        let assembly = AssemblyServices::new().with_task_factory(factory);
        let services = make_task_services().await;
        let _ = assembly.tasks(&services);
        let _ = assembly.tasks(&services);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );
    }

    // ── 审批门控插槽（ADR-049 期 3-T1）──────────────────────────────────

    /// 只用于装配测试的审批门控桩（本库的插槽测试只关心「工厂是否被使用」）。
    struct StubApprovalGate {
        /// 判别标记：装配层注入的是哪个实例（与「未装配」区分）。
        marker: &'static str,
    }

    #[async_trait]
    impl ApprovalGate for StubApprovalGate {
        async fn authorize(
            &self,
            _session_id: &str,
            _action: &tianyan::executor::Action,
            _allow_human_wait: bool,
        ) -> tianyan::Result<tianyan::executor::approval::ApprovalResponse> {
            Ok(tianyan::executor::approval::ApprovalResponse {
                request_id: self.marker.to_string(),
                decision: tianyan::executor::approval::ApprovalDecision::Deny,
                reason: None,
                responded_at: chrono::Utc::now(),
                approved_by: "stub".to_string(),
                edited_command: None,
            })
        }

        async fn status(&self) -> tianyan::executor::approval::ApprovalStatusSnapshot {
            tianyan::executor::approval::ApprovalStatusSnapshot {
                config: tianyan::executor::approval::ApprovalWorkflowConfig::default(),
                pending_approvals: Vec::new(),
                pending_confirmations: Vec::new(),
                recent_records: Vec::new(),
                confirmed_action_count: 0,
            }
        }

        async fn respond_to_approval(
            &self,
            _request_id: &str,
            _decision: tianyan::executor::approval::ApprovalDecision,
            _reason: Option<String>,
            _approved_by: &str,
            _edited_command: Option<String>,
        ) -> tianyan::Result<()> {
            Ok(())
        }

        async fn record_user_confirmation_by_fingerprint(&self, _fingerprint: &str) {}
    }

    /// 未装配工厂时插槽为空：`AgentBuilder` 因此拿到 `None`，工具走「未装配直接放行」
    /// 分支 + 启动期告警。
    ///
    /// 判别力：若把 `AssemblyServices::default()` 改成「兜底造一个放行门控」，本断言
    /// 必红——而兜底门控会让「未装配」与「装配了全自主模式」在日志与状态查询上
    /// **无法区分**（ADR-038 可见失败的反面）。
    #[test]
    fn test_default_assembly_has_no_approval_factory() {
        assert!(
            AssemblyServices::new().approval_factory().is_none(),
            "未装配时插槽必须为空（降级由 AgentBuilder 的 None 分支承担）"
        );
    }

    /// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载会重建 agent，
    /// 审批规则必须跟着新配置走）。
    ///
    /// 判别力：桩返回 `approved_by = "stub"`（工厂内的判别标记）——若插槽没被使用
    /// （例如 `approval_factory()` 恒返回 `None`），`expect` 直接 panic。
    #[tokio::test]
    async fn test_approval_factory_is_injected_and_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: ApprovalGateFactory = Arc::new(
            move |_security: &tianyan::config::SecurityConfig,
                  _working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
                  _sink: tianyan::notification::SharedNotificationSink| {
                counter.fetch_add(1, Ordering::SeqCst);
                Arc::new(StubApprovalGate { marker: "injected" }) as Arc<dyn ApprovalGate>
            },
        );
        let assembly = AssemblyServices::new().with_approval_factory(factory);
        let security = tianyan::config::SecurityConfig::default();
        let working_sets = tianyan::agent::working_set::WorkingSetRegistry::new(None);
        let sink: tianyan::notification::SharedNotificationSink =
            Arc::new(tianyan::notification::NoopNotificationSink);

        let gate = assembly.approval_factory().expect("装配后必须有工厂")(
            &security,
            working_sets.clone(),
            sink.clone(),
        );
        let _ = assembly.approval_factory().expect("装配后必须有工厂")(
            &security,
            working_sets,
            sink,
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );

        let action = tianyan::executor::Action::ReadFile {
            path: "probe".to_string(),
        };
        let resp = gate
            .authorize("s1", &action, false)
            .await
            .expect("桩不该失败");
        assert_eq!(resp.approved_by, "stub", "应使用注入的实例");
    }

    // ── 上下文策略插槽（ADR-049 期 3-T2）────────────────────────────────

    /// 只用于装配测试的上下文端口桩（本库的插槽测试只关心「工厂是否被使用」）。
    struct StubContextPort {
        /// 判别标记：装配层注入的是哪个实例（与「未装配」区分）。
        marker: &'static str,
    }

    #[async_trait]
    impl ContextPort for StubContextPort {
        async fn load_injectable(
            &self,
            _query: &str,
            _working_directory: Option<&std::path::Path>,
        ) -> tianyan::Result<tianyan::common::types::InjectableContext> {
            Ok(tianyan::common::types::InjectableContext::new())
        }

        async fn compress_for_session(
            &self,
            _messages: &[tianyan::Message],
            _session_id: &str,
            _recent_input_tokens: usize,
            _force: bool,
            _existing_summary: Option<&str>,
        ) -> Option<tianyan::common::types::StructuredMessage> {
            None
        }

        fn extract_marker_summary(
            &self,
            _marker: &tianyan::common::types::StructuredMessage,
        ) -> Option<String> {
            Some(self.marker.to_string())
        }
    }

    /// 构造上下文装配能力集合（VFS / 模型服务 / 统计全为最小替身）。
    fn make_context_services() -> ContextServices {
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        ContextServices {
            config: tianyan::config::TianyanConfig::default(),
            vfs: Arc::new(tianyan::test_utils::MockVfs::new()),
            usage_stats: tianyan::observability::usage_stats::UsageStats::new(database)
                .expect("统计"),
            chat: Arc::new(StubChatService),
            chat_model: "stub-model".to_string(),
            chat_spec: None,
        }
    }

    /// 未装配工厂时插槽为空：`AgentBuilder` 因此拿到 `None`，落到 `NullContextPort`
    /// （空可注入上下文 + 永不压缩）+ 启动期告警；`start_server*` 另有硬守卫。
    ///
    /// 判别力：若把 `AssemblyServices::default()` 改成「兜底造一个空直通端口」，本断言
    /// 必红——那会让「未装配」与「装配了某个真实现」在日志上**无法区分**
    /// （ADR-038 可见失败的反面）。
    #[test]
    fn test_default_assembly_has_no_context_factory() {
        assert!(
            AssemblyServices::new().context_factory().is_none(),
            "未装配时插槽必须为空（降级由 AgentBuilder 的 None 分支承担）"
        );
        assert!(
            !AssemblyServices::new().has_context_factory(),
            "未装配时启动守卫必须能看见「缺件」"
        );
    }

    /// 装配了工厂时**只用工厂**，且工厂可重复调用（配置热重载 / 模型切换会重建 agent，
    /// 压缩窗口必须跟着新模型规格走）。
    ///
    /// 判别力：桩在 `extract_marker_summary` 里回 marker（工厂内的判别标记）——若插槽
    /// 没被使用（例如 `context_factory()` 恒返回 `None`），`expect` 直接 panic。
    #[tokio::test]
    async fn test_context_factory_is_injected_and_repeatable() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let factory: ContextFactory = Arc::new(move |_services: &ContextServices| {
            counter.fetch_add(1, Ordering::SeqCst);
            Arc::new(StubContextPort { marker: "injected" }) as Arc<dyn ContextPort>
        });
        let assembly = AssemblyServices::new().with_context_factory(factory);
        assert!(assembly.has_context_factory(), "装配后守卫必须放行");
        let services = make_context_services();

        let port = assembly.context_factory().expect("装配后必须有工厂")(&services);
        let _ = assembly.context_factory().expect("装配后必须有工厂")(&services);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "工厂应可重复调用（不是 FnOnce）"
        );

        // 端口身份探针：桩不看内容，任意消息即可。
        let probe = tianyan::context::ContextAssembler::message_to_structured(
            &tianyan::Message::user("probe"),
            "s1",
            None,
            None,
        );
        assert_eq!(
            port.extract_marker_summary(&probe).as_deref(),
            Some("injected"),
            "应使用注入的实例"
        );
    }
}
