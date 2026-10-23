//! 装配扩展点（ADR-049 期 4b）：按**变化原因**分组的四组插槽——三份有序清单 +
//! 结构化/向量存储 + 策略实现 + 模型 provider 实现。
//!
//! 插槽覆盖面自期 3-T5 起不变：插件工具清单 + 向量存储实现 + 结构化存储实现 +
//! 内置任务清单 + 审批门控实现 + 上下文策略实现 + 评审策略实现 + **provider 实现
//! （聊天/视觉/嵌入）** + 工具管线。
//!
//! server 库**不认识任何具体插件 crate**：本模块只定义**类型擦除**的插槽
//! （以及没有插槽时的降级实现），由装配层（`app` bin / `tauri` 桌面端）在启动
//! 时注入。这样改一行插件实现只重编插件 + 装配包，server 库（13.4K 源码）不动
//! ——闸门 A 的前提。
//!
//! ## 四组（期 4b：按「什么时候一起改」分组）
//!
//! ```text
//!   AssemblyServices
//!   ├── manifests  三份「有序清单」：plugin_factory / task_factory / tool_pipeline_factory
//!   ├── storage    结构化存储 + 向量存储：storage_factory / vector_factory
//!   ├── policies   策略实现：approval / context / task_reviewer / skill_review
//!   └── providers  模型 provider：provider_llm / provider_embed
//! ```
//!
//! 分组理由（逐组一句）：
//!
//! - **`manifests`**：[`ToolPluginFactory`]（工具注册顺序）+ [`TaskFactory`]（任务扫描
//!   顺序）+ [`ToolPipelineFactory`]（监听器顺序）——**三条顺序纪律的载体**，都是
//!   「插件提供的有序清单」（D6：清单是有序数据，顺序即语义），新增/调整清单时一起动；
//! - **`storage`**：换存储后端 / 向量库时一起动（[`StorageBackendFactory`] +
//!   [`VectorStorageFactory`]，两者读同一份 `StorageConfig`）；
//! - **`policies`**：内核能力的**策略实现**（审批 / 上下文注入与压缩 / 两个评审端口），
//!   改策略时一起动；
//! - **`providers`**：换模型 provider 时一起动（聊天视觉与嵌入共用同一套客户端构造
//!   语义）。
//!
//! 分组同时是**目录模块**：`manifests.rs` / `storage.rs` / `policies.rs` /
//! `providers.rs`，没有插槽时的降级实现跟着所属组走（`NoopVectorStorage` /
//! `EphemeralStorageBackend` 在 `storage.rs`），插槽测试仍在 `tests.rs`。对外路径
//! 不变（`crate::assembly::X` 与 `tianyan_server::X`）。
//!
//! ## 插槽逐条语义（共十一根）
//!
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
//! - [`TaskReviewerFactory`] / [`SkillReviewFactory`]：评审策略实现
//!   （`tianyan-policy-review`，ADR-049 期 3-T3）。**同步**：两个实现都只存句柄与字段；
//!   「判定提示词 / `VERDICT:` 解析口径 / 评分标准 / 截断预算」随之进插件
//!   （任务自审走 [`TaskReviewerFactory`]，技能复审读端口走 [`SkillReviewFactory`]）。
//! - [`ProviderLlmFactory`] / [`ProviderEmbedFactory`]：provider 实现
//!   （`tianyan-provider-llm` / `tianyan-provider-embed`，ADR-049 期 2 序 6）。**同步**：
//!   客户端构造与三个服务的组装都只存字段（无 IO）；「按能力解析 provider/model」「每
//!   provider 的客户端池」「方言驱动的请求构造」随之进插件（单一事实源）。产出
//!   [`ProviderLlmServices`] / [`ProviderEmbedServices`]（**契约在 core**，因为插件与
//!   装配层三方共用），本库再用它们拼 [`crate::ModelServices`]。

use std::pin::Pin;
use std::sync::Arc;

use tianyan::config::StorageConfig;
use tianyan::executor::approval::ApprovalGate;
use tianyan::vfs::{StorageBackend, VectorStorage};

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

// provider 能力集合的**契约面**（ADR-049 期 2 序 6）：与任务清单 / 上下文策略同一条纪律
// ——定义在 core 的 `model::assembly`，此处 re-export 成插槽契约。插件（生产）、装配层
// （注入）与本库（拼容器）三方都要引用它，住在 server 会让 provider 插件反过来依赖
// server 库（破 D2-1）。
pub use tianyan::model::{ConnectionProbe, ProviderEmbedServices, ProviderLlmServices};

// 工具管线的**契约面**（ADR-049 期 3-T5）：`ToolPostExecuteListener` 与接线形状
// （`ToolPipelineWiring`）住在 core 的 `agent::tool_registry::pipeline`——管线是 K1 的
// 第 4 阶段（注册表按注册顺序调用监听器），契约搬走会让内核反向依赖插件。此处
// re-export 成插槽契约：插件（生产）、装配层（注入）与本库（转交 builder）三方共用。
pub use tianyan::agent::{ToolPipelineFactory, ToolPipelineWiring};

/// 聊天 / 视觉 provider 工厂（**装配层注入**，ADR-049 期 2 序 6）。
///
/// `Arc<dyn Fn(&ModelsConfig) -> Result<ProviderLlmServices>>`：参数是模型配置（借用；
/// 实现内部按需克隆），返回值是**能力集合**（聊天服务 + 视觉服务 + 两个模型名 + 每
/// provider 的聊天客户端池 + 连通性探针）。
///
/// **同步**（与 [`StorageBackendFactory`] / [`TaskFactory`] / [`ApprovalGateFactory`]
/// 同理，与 [`VectorStorageFactory`] 相反）：`AsyncOpenAIClient::from_provider` 只构造
/// HTTP 客户端（不发请求），没有任何需要等待的 IO。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：配置热重载会重建模型服务
/// （`AppState::reload_agent`），客户端池必须跟着**新配置**走。
pub type ProviderLlmFactory = Arc<
    dyn Fn(&tianyan::config::ModelsConfig) -> tianyan::Result<ProviderLlmServices> + Send + Sync,
>;

/// 嵌入 provider 工厂（**装配层注入**，ADR-049 期 2 序 6）。
///
/// `Arc<dyn Fn(&ModelsConfig) -> Result<ProviderEmbedServices>>`：返回值是能力集合
/// （嵌入服务 + 模型名 + **用量上报槽位**）。槽位由插件创建、本库持有并在拿到
/// `UsageLog` 后后置注入（嵌入不走 AgentLoop，其 token 用量必须单独入账）。
///
/// **同步**（同 [`ProviderLlmFactory`]）：构造只存字段与 HTTP 客户端。
pub type ProviderEmbedFactory = Arc<
    dyn Fn(&tianyan::config::ModelsConfig) -> tianyan::Result<ProviderEmbedServices> + Send + Sync,
>;

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

/// 任务结果自审器工厂（**装配层注入**，ADR-049 期 3-T3）。
///
/// `Arc<dyn Fn(Arc<dyn ChatService>, &str) -> Arc<dyn TaskReviewer>>`：
/// - 第一个参数是**共享的**聊天客户端（`ModelServices.chat` 的克隆，不重复建 HTTP
///   客户端池）；
/// - 第二个参数是评审模型名（`resolve_chat_model` 的结果，装配层解析）。
///
/// 为什么需要**两个**评审插槽（与任务书初稿的「只加一个 `skill_review_factory`」的偏差，
/// 理由是硬的）：`LlmTaskReviewer` 的实现今天由 **core** 的
/// `AgentBuilder::build` 构造（`background_self_review` 为真时内联 `new`）。实现搬进
/// 插件后，builder 若没有注入路径就只能把实现留在 core——那等于没搬。因此第二个插槽是
/// 这次搬移的**必要配套**，不是范围扩张；形状与 [`ApprovalGateFactory`] 同款。
///
/// **同步**（与其余插槽同理）：`LlmTaskReviewer::new` 只存两个字段，没有任何 IO。
///
/// 用 `Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`：配置热重载会重建 agent，评审模型可能已改。
pub type TaskReviewerFactory = Arc<
    dyn Fn(
            Arc<dyn tianyan::model::ChatService>,
            &str,
        ) -> Arc<dyn tianyan::executor::review::TaskReviewer>
        + Send
        + Sync,
>;

/// 技能使用复审工厂（**装配层注入**，ADR-049 期 3-T3）。
///
/// `Arc<dyn Fn(Arc<dyn ChatService>, &str, Arc<dyn VirtualFileSystem>)
///   -> Arc<dyn SkillReviewPort>>`：聊天客户端（共享克隆）+ 评审模型名 + VFS
///   （**与 `SkillManager` 同一个实例**——评审写下的记录与面板读的必须同一份内容）。
///
/// 只产出**读端口**（`latest_review`）：产生评审的完整逻辑（`review_conversation`）
/// 今天零生产调用者，属「未接线」而不是死代码，因此留在插件的具体类型上、不进契约
/// （见 `plugins/policy-review` 的 crate 文档与 ADR-049 期 3-T3 遗留）。
///
/// **同步**：`SkillReviewer::new` 只存三个字段。用 `Arc<dyn Fn>` 而非
/// `Box<dyn FnOnce>`：每次技能统计请求都要现造一个（旧实现 `create_skill_reviewer`
/// 就是逐次构造，语义等同）。
pub type SkillReviewFactory = Arc<
    dyn Fn(
            Arc<dyn tianyan::model::ChatService>,
            &str,
            Arc<dyn tianyan::vfs::VirtualFileSystem>,
        ) -> Arc<dyn tianyan::skills::SkillReviewPort>
        + Send
        + Sync,
>;

mod manifests;
mod policies;
mod providers;
mod storage;

pub use manifests::ManifestAssemblies;
pub use policies::PolicyAssemblies;
pub use providers::ProviderAssemblies;
pub use storage::StorageAssemblies;

// `EphemeralStorageBackend` 是**降级实现**，本体在 `storage.rs`；这里只为
// `lib.rs::tests` 的启动守卫测试把它抬到 `crate::assembly::` 可见（`pub(crate)`，
// `tianyan_server` 的公开面仍然只有工厂类型与四个组结构）。测试目标之外没有任何
// 使用者，因此 `cfg(test)` 门控（避免非测试构建里出现未使用的 re-export）。
#[cfg(test)]
pub(crate) use storage::EphemeralStorageBackend;

#[cfg(test)]
mod tests;

/// 装配所需的外部能力集合（`None` = 该扩展点未装配，走降级路径）。
///
/// 目前十一个插槽，按**变化原因**分四组（ADR-049 期 4b，分组理由见 crate 文档）：
/// 插件工具清单（[`ToolPluginFactory`]）+ 内置任务清单（[`TaskFactory`]）+ 工具管线
/// （[`ToolPipelineFactory`]）→ `manifests`；向量存储（[`VectorStorageFactory`]）+
/// 结构化存储（[`StorageBackendFactory`]）→ `storage`；审批门控
/// （[`ApprovalGateFactory`]）+ 上下文策略（[`ContextFactory`]）+ 任务自审
/// （[`TaskReviewerFactory`]）+ 技能复审（[`SkillReviewFactory`]）→ `policies`；
/// 聊天/视觉 provider（[`ProviderLlmFactory`]）+ 嵌入 provider
/// （[`ProviderEmbedFactory`]）→ `providers`。用**一个结构**而不是给每个启动入口再加
/// 一个参数：入口变体的数量是组合爆炸（listener × 关闭信号 × 插件 × 向量 × 存储 ×
/// 任务），打包后只需「默认装配」与「装配齐全」两类入口。
///
/// ★ **平铺表的治理建议**（ADR-049 期 3-T5 遗留 1，期 4 执行）：11 个平铺字段 + 11 个
/// `with_*` + 11 个 `pub(crate)` 访问器已经到「查一眼要滚三屏」的程度。期 4 收敛时应
/// 按**插件**分组（一个插件一份能力结构，如 `struct ToolPlugins { fs, code, todo, … }`）
/// 或改成**清单结构**（`Vec<AssemblyEntry>`，由插件 crate 各自提供 `entry()`，装配层只
/// 拼接），而不是继续平铺；`AppState::new` 的同名参数列表同理。
///
/// **期 4b 已执行**：11 个平铺字段按「变化原因」收敛成 4 个组结构（`manifests` /
/// `storage` / `policies` / `providers`，见 crate 开头的分组理由与四张组文档），
/// `AssemblyServices` 上的 11 个 `with_*` 转发已删除（不保留两套构造路径），
/// `AppState::new` 的插槽形参从 7 个收成 3 个。保留本段原文是为了留下判据与方向
/// （「按插件分组」与「清单结构」仍是可选的下一步）。
#[derive(Clone, Default)]
pub struct AssemblyServices {
    /// 三份有序清单的工厂（工具注册顺序 / 任务扫描顺序 / 监听器顺序，见 crate 文档
    /// 「分组理由」第 1 条）。
    pub manifests: ManifestAssemblies,
    /// 结构化存储 + 向量存储工厂。
    pub storage: StorageAssemblies,
    /// 策略实现（审批 / 上下文 / 两个评审端口）。
    pub policies: PolicyAssemblies,
    /// 模型 provider 实现（聊天视觉 + 嵌入）。
    pub providers: ProviderAssemblies,
}

impl AssemblyServices {
    /// 空装配（`create_app` / 嵌入式 / 测试路径：无插件工具、无持久化向量存储）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 三份有序清单所在组（`pub(crate)`：只有本库的组合根取用，装配层负责注入）。
    pub(crate) fn manifests(&self) -> &ManifestAssemblies {
        &self.manifests
    }

    /// 存储组（结构化 + 向量）。
    pub(crate) fn storage(&self) -> &StorageAssemblies {
        &self.storage
    }

    /// 策略组（审批 / 上下文 / 两个评审端口）。
    pub(crate) fn policies(&self) -> &PolicyAssemblies {
        &self.policies
    }

    /// provider 组（聊天视觉 + 嵌入）。
    pub(crate) fn providers(&self) -> &ProviderAssemblies {
        &self.providers
    }
}
