//! 天演装配清单（ADR-049 期 4）：插件工厂的**单一实现**与**跨插件顺序纪律**。
//!
//! ## 为什么单独成包
//!
//! 期 1~3 把能力逐个搬进插件后，装配清单在 `app/src/main.rs`（277 行）与
//! `tauri/src/server.rs`（268 行）**各写了一遍**：11 个工厂函数逐字同形，靠注释
//! 「与 `app/src/main.rs::xxx_factory` **同形同源**」互相提醒。两份副本的漂移是
//! 静默的——插槽签名加一个参数时，先改的那份能编、后改的那份报错；而**顺序纪律**
//! （工具 fs→code→todo→web、任务 evolution 排第 2）在两份里分别成立，一旦某份被
//! 单独调整，症状是「桌面端聊天前缀缓存命中率突然下降」而不是编译失败。
//!
//! 因此清单本身也需要单一事实源。**依赖方向**（关键：不产生反向边）：
//!
//! ```text
//!   app（bin: tianyan-server）              tauri（桌面端）
//!         │                                     │
//!         └──────────────┬──────────────────────┘
//!                        ▼
//!              tianyan-assembly          ← 本 crate：全仓唯一 use 全部插件的地方
//!                        │
//!        ┌───────────────┼───────────────────┐
//!        ▼               ▼                   ▼
//!   15 个插件 crate   tianyan-server     （无反向边：server 不认识本 crate，
//!   （能力实现）      （类型擦除插槽）      也不认识任何插件——闸门 A 前提）
//! ```
//!
//! 本 crate 与 `service/config` 平级：两者都是**装配侧的服务层 crate**（不是能力插件）。
//! 它不放进 `plugins/`，因为插件的铁律是**插件之间零互赖**，而本 crate 恰恰要
//! `use` 全部插件——它在插件**之上**，不在插件**之间**。
//!
//! ## 两份清单：`full()` 与 `policies()`
//!
//! 装配清单有两个真实消费者，它们的**严格度不同**：
//!
//! ```text
//!   policies()  = 9 个「策略 / 基础设施」插槽        ← 集成测试（test_assembly）也用它
//!     manifests : task（builtin + evolution）/ tool_pipeline（tool-pipeline）
//!     storage   : storage            （Sqlite / LocalFile）
//!     policies  : approval（policy-approval）/ context（policy-context）/
//!                 task_reviewer、skill_review（policy-review）
//!     providers : provider_llm（provider-llm）/ provider_embed（provider-embed）
//!                        +
//!   full()      = policies() + 2 个「环境相关」插槽  ← app / tauri 用它
//!     manifests : plugin_factory     （动态工具清单；需真 ToolContext +
//!                                      PluginServices 才能执行）
//!     storage   : vector_factory     （LanceDB；集成测试走 NoopVectorStorage，
//!                                      避免每个测试建目录）
//! ```
//!
//! 上面两张表里的 `manifests` / `storage` / `policies` / `providers` 是
//! `AssemblyServices` 的**四个组**（ADR-049 期 4b 按「变化原因」分组）；四组的分组理由
//! 逐条写在 `tianyan_server::assembly` 的 crate 文档里，本 crate 只负责「哪一组该装
//! 什么」。`full() = policies() + 2` 的表述保持有效——只是这 2 个插槽分别追加到
//! `manifests`（`plugin_factory`）与 `storage`（`vector_factory`）两组。
//!
//! 为什么不是「一份 full() + 测试自己删两个插槽」：`AssemblyServices` 的字段只有
//! `with_*` 没有 `without_*`（那是**降级路径**的语义污染——「装配了又拿掉」与
//! 「本来就没装」在运行期不可区分）。因此用一个显式的**较窄清单**表达测试剖面，
//! 并把它写成 `full()` 的前缀，保证两份清单不会分叉：
//! `full() = policies() + 2`。
//!
//! 这也解释了 `server/tests/common/factory.rs::test_assembly()` 为什么能直接用
//! `policies()`：集成测试驱动 `create_app_with_assembly` 时**不需要**插件工具清单
//! （`create_app` 路径本就不注册动态工具）与真实向量存储（LanceDB 会为每个测试
//! 建目录、拉全链编译）。但**其余 9 个插槽必须与生产同形**——否则测的就不是生产
//! 形态（「不要用 `AssemblyServices::default()` 驱动集成测试」是同一条纪律）。
//!
//! ## 三条顺序纪律（只有本 crate 知道）
//!
//! ```text
//! ① 工具注册顺序 = 提示词前缀顺序（抖动 → 前缀缓存全白）
//!    fs(read_file, write_file, apply_edit, apply_patch, glob, list_dir)
//!      → code(symbol_outline, repo_map, grep)
//!      → todo(todo) / goal(goal)
//!      → web(web_search, web_fetch)
//!
//! ② 任务清单顺序 = 扫描顺序 = 同一轮到期任务的执行顺序
//!    summary_generation → evolution → garbage_collection → usage_stats_flush
//!      → [snapshot_gc] → [reminder]
//!          ▲
//!          └── splice(1..1)：**跨插件**位置，只有装配层知道
//!              （task-builtin 不知道自己后面还有 evolution，
//!                 task-evolution 不知道自己该排第二 —— 插件之间不 import，D2-2）
//!
//! ③ 工具管线监听器顺序（ADR-047，「顺序是语义」）
//!    观测（统计 / Trace / GEPA / 规则学习）→ 落盘（改写者）
//!          ▲
//!          └── 顺序在插件内 `ToolPipeline::into_listeners` 一行可见，
//!              本 crate 只做类型擦除包装（顺序不由这里决定）
//! ```
//!
//! ①② 由本 crate 的测试锁定（`test_tool_order_is_fs_code_todo_web` /
//! `test_merge_task_lists_*`）；③ 的顺序在插件内，由插件侧行为式测试锁定。

use std::sync::Arc;

use tianyan::vfs::{StorageBackend, VectorStorage};
use tianyan_server::{
    ApprovalGateFactory, AssemblyServices, ContextFactory, ManifestAssemblies, PluginServices,
    PolicyAssemblies, ProviderAssemblies, ProviderEmbedFactory, ProviderLlmFactory,
    SkillReviewFactory, StorageAssemblies, StorageBackendFactory, TaskFactory, TaskRegistration,
    TaskReviewerFactory, ToolPipelineFactory, ToolPluginFactory, VectorStorageFactory,
};
use tianyan_storage_sqlite::{LocalFileBackend, SqliteBackend};
use tianyan_storage_vector::LanceDbVectorStore;

// ============================================================================
// 单个插槽的工厂（每个插件一个；类型擦除包装的唯一实现）
// ============================================================================

/// 插件工具清单工厂（`plugins/tools-{fs,code,todo,web}`）。
///
/// **顺序即注册顺序**（追加到内置工具末尾）：fs → code → todo/goal → web。
/// 工具清单进的是 system 提示词的工具目录，顺序抖动会让整个前缀缓存失效
/// （ADR-004 前缀零漂移），因此这条顺序由测试锁定，不靠注释维持。
///
/// 三个分支的签名各不相同（`tools(ctx)` / `tools(todo, goal)` / `tools(&web_config)`），
/// 这正是「清单无法数据化」的原因：插件各自决定自己需要什么，装配层负责满足并排序。
///
/// **为什么 `pub`**：除了 [`full`]，还有两个**真实**消费者需要同一份清单——
/// ① `app/examples/tool_catalog.rs`（工具目录的**权威生成器**：目录必须与运行时
/// 工具集逐字一致，否则插件工具既无文档覆盖、也不受 freshness 门禁保护）；
/// ② `app/tests/assembly.rs`（装配级回归：插件工具全部注册 / 展示意图 / 不撞内置 /
/// 工厂可重复调用）。改造前这两处各写了一遍清单（加上两个装配器与一个集成测试
/// 工厂，**同一份清单共 5 处副本**）——顺序纪律靠 5 份互相提醒，等于没有纪律。
pub fn plugin_factory() -> ToolPluginFactory {
    Arc::new(
        |ctx: &tianyan::agent::ToolContext, services: PluginServices| {
            let mut tools = tianyan_tools_fs::tools(ctx);
            tools.extend(tianyan_tools_code::tools(ctx));
            // `todo` / `goal` 还需要**数据存储**（会话绑定的待办 / 目标）：store 由
            // server 从 `AppState` 递过来（`PluginServices`）——工具必须与 REST API
            // 读写同一份实例，否则工具写的待办在 UI 里看不见。`tools()` 内部顺序
            // 固定 `todo` → `goal`（前缀缓存稳定性）。
            tools.extend(tianyan_tools_todo::tools(
                services.todo_store(),
                services.goal_store(),
            ));
            // `web_search` / `web_fetch`：客户端由插件从 `services.web_config()` 就地
            // 构造（配置非法时降级为空清单，装配不 panic）。
            tools.extend(tianyan_tools_web::tools(&services.web_config()));
            tools
        },
    )
}

/// 向量存储工厂（LanceDB；`plugins/storage-vector`）。
///
/// 类型擦除的同款手法：server 库只认 [`VectorStorageFactory`]（`Arc<dyn Fn>`），
/// `LanceDbVectorStore` 只在本 crate 出现。工厂**可重复调用**（数据目录搬迁后以
/// 新配置重启会再次装配）；`&StorageConfig` 是借用，需先 `clone` 再 move 进 async 块。
fn vector_factory() -> VectorStorageFactory {
    Arc::new(|config: &tianyan::config::StorageConfig| {
        let config = config.clone();
        Box::pin(async move {
            let store = LanceDbVectorStore::new(&config).await?;
            Ok(Arc::new(store) as Arc<dyn VectorStorage>)
        })
    })
}

/// 结构化存储工厂（SQLite / 本地文件系统；`plugins/storage-sqlite`）。
///
/// **adapter 选择的唯一落地处**（ADR-049 D5）：`config.storage.backend` 决定用
/// `SqliteBackend`（默认，走共享连接——ADR-005 单连接）还是 `LocalFileBackend`。
/// server 库拿不到这个判断，因为它不认识两个具体类型。
///
/// 同步工厂（两个后端的 `new` 都只存字段，建表 / 建目录在 `initialize` 里）。
fn storage_factory() -> StorageBackendFactory {
    Arc::new(
        |config: &tianyan::config::StorageConfig, database: Arc<tianyan::db::Database>| {
            use tianyan::config::StorageBackendType;
            let backend: Arc<dyn StorageBackend> = match config.backend {
                StorageBackendType::Local => Arc::new(LocalFileBackend::new(config.clone())),
                StorageBackendType::Sqlite => Arc::new(SqliteBackend::new(database)),
            };
            Ok(backend)
        },
    )
}

/// 跨插件清单拼接：演化任务紧跟 `summary_generation`（清单第 2 位）。
///
/// **为什么这是装配层的职责**：注册顺序 = 扫描顺序 = 同一轮到期任务的执行顺序，
/// 而「演化任务排第 2 位」是**跨插件属性**——`task-builtin` 不知道演化任务存在，
/// 演化插件也不知道别人排在哪，只有装配清单知道「哪个插件该排在哪」。让插件互相
/// `import` 来拼接会破 D2-2（插件之间零互赖）。
///
/// 三个角色各守其位：`tianyan_task_builtin::tasks()` 保持既有 5 项的相对顺序
/// （不为位置妥协）、`tianyan_task_evolution::tasks()` 返回 0/1 项且不关心绝对位置、
/// 合并用 `splice(1..1, …)`（**在 index 1 之前插入**）。
///
/// 用 `splice` 而不是 `insert` 循环：让这条纪律在一行里可见，也让「演化插件未启用」
/// 时自然退化为「只有内置那几项」（`splice` 空区间 = 无操作）。位置纪律只在这里，
/// 由 `test_merge_task_lists_puts_evolution_second` 锁定。
fn merge_task_lists(base: &mut Vec<TaskRegistration>, evolution: Vec<TaskRegistration>) {
    base.splice(1..1, evolution);
}

/// 定时任务工厂（`plugins/task-builtin` + `plugins/task-evolution`）。
///
/// 5 个内置任务（摘要生成 / 垃圾回收 / 统计刷盘 / 快照 GC / 主动提醒）与自演化综述
/// 任务的实现与**注册清单**（名字 / 显示名 / 间隔 / 启用条件）都在插件里；本 crate
/// 只做「拼接两个插件的清单 + 包成类型擦除的 [`TaskFactory`]」——server 库因此不认识
/// 任何具体任务。
///
/// **同步**工厂：能力集合（`TaskServices`）由 server 在调用前 `await` 好（聊天模型
/// 服务是 `AppState` 里共享 `ModelServices` 的克隆，不重复建 HTTP client 池），
/// 插件里只做「按配置构造处理器 + 判启用条件」。
fn task_factory() -> TaskFactory {
    Arc::new(|services: &tianyan_server::TaskServices| {
        let mut registrations = tianyan_task_builtin::tasks(services);
        merge_task_lists(&mut registrations, tianyan_task_evolution::tasks(services));
        registrations
    })
}

/// 审批门控工厂（`plugins/policy-approval`）。
///
/// 同款类型擦除：server 库只认 [`ApprovalGateFactory`]（`Arc<dyn Fn>`），审批工作流
/// 只在本 crate 出现。「`SecurityConfig`（模式 / 黑名单 / 白名单 / 总是询问）→ 自动
/// 审批规则」的映射住**插件**里（`tianyan_policy_approval::gate`），两个装配器因此
/// 只各写一行调用，不会各抄一份映射逻辑（那是两份口径同时漂移的经典来源）。
///
/// 同步工厂：`ApprovalWorkflow::new` 只存字段、通知器只存句柄，没有任何 IO。
fn approval_factory() -> ApprovalGateFactory {
    Arc::new(
        |security: &tianyan::config::SecurityConfig,
         working_sets: Arc<tianyan::agent::working_set::WorkingSetRegistry>,
         sink: tianyan::notification::SharedNotificationSink| {
            tianyan_policy_approval::gate(security, working_sets, sink)
        },
    )
}

/// 上下文策略工厂（`plugins/policy-context`）。
///
/// 注入顺序、RRF 检索参数、压缩阈值与摘要提示词、意图分类关键词——以及「压缩窗口
/// ↔ 模型规格联动」——全部住**插件**里（`tianyan_policy_context::pipeline`）。
///
/// 同步工厂：`ContextPipeline::new` / `ContextCompressor::new` / `DualLayerRetriever::new`
/// 都只存字段，没有任何 IO。
fn context_factory() -> ContextFactory {
    Arc::new(|services: &tianyan::context::ContextServices| {
        tianyan_policy_context::pipeline(services)
    })
}

/// 任务结果自审器工厂（`plugins/policy-review`）。
///
/// 判定提示词、`VERDICT:` 解析口径、故障兜底全在插件里
/// （`tianyan_policy_review::task_reviewer`），改一句提示词不必重编内核。
///
/// 同步工厂：`LlmTaskReviewer::new` 只存（chat 客户端, 模型名）两个字段。
fn task_reviewer_factory() -> TaskReviewerFactory {
    Arc::new(|chat: Arc<dyn tianyan::model::ChatService>, model: &str| {
        tianyan_policy_review::task_reviewer(chat, model)
    })
}

/// 技能使用复审工厂（`plugins/policy-review`）。
///
/// 评审记录落 VFS `skill/_reviews/<id>.jsonl`——路径与技能发现时的过滤条件
/// （core 的 `REVIEWS_PREFIX`）同源，因此面板读得到插件写下的内容。
///
/// 同步工厂：`SkillReviewer::new` 只存（chat, vfs, 模型名）三个字段。
fn skill_review_factory() -> SkillReviewFactory {
    Arc::new(
        |chat: Arc<dyn tianyan::model::ChatService>,
         model: &str,
         vfs: Arc<dyn tianyan::vfs::VirtualFileSystem>| {
            tianyan_policy_review::skill_reviewer(chat, model, vfs)
        },
    )
}

/// 聊天 / 视觉 provider 工厂（`plugins/provider-llm`）。
///
/// 客户端构造 + 按能力解析 + 每 provider 的聊天路由池 + 连通性探针全在插件里
/// （`tianyan_provider_llm::services`）。
///
/// 同步工厂：客户端构造（UA / 方言 / 双预算超时 / 动态会话头 / 重试策略）不发请求。
fn provider_llm_factory() -> ProviderLlmFactory {
    Arc::new(|config: &tianyan::config::ModelsConfig| tianyan_provider_llm::services(config))
}

/// 嵌入 provider 工厂（`plugins/provider-embed`）。
///
/// 产出里带**用量上报槽位**：server 拿到 `UsageLog` 后后置注入（嵌入不走 AgentLoop，
/// 其 token 用量必须单独入账）。
fn provider_embed_factory() -> ProviderEmbedFactory {
    Arc::new(|config: &tianyan::config::ModelsConfig| tianyan_provider_embed::services(config))
}

/// 工具管线工厂（`plugins/tool-pipeline`）。
///
/// **★ 顺序是语义**（ADR-047）：工厂返回的顺序即注册顺序——观测在前（数据层须见
/// 原始结果）、落盘在后（它是改写者）。顺序在插件里一行可见
/// （`tianyan_tool_pipeline::ToolPipeline::into_listeners`），并由插件侧行为式测试
/// 锁定（跑完整条管线后看 GEPA 数据层记的是原文还是预览）；本 crate 不重排。
fn tool_pipeline_factory() -> ToolPipelineFactory {
    tianyan_tool_pipeline::pipeline_factory()
}

// ============================================================================
// 两份清单
// ============================================================================

/// **生产清单的前缀**：9 个「策略 / 基础设施」插槽。
///
/// 集成测试（`server/tests/common/factory.rs::test_assembly()`）直接用它——这些
/// 插槽的严格度与生产**完全一致**：存储后端按 `config.storage.backend` 分发到真实
/// `SqliteBackend` / `LocalFileBackend`、任务清单是真实插件项、审批 / 上下文 / 评审 /
/// provider / 管线全部注入真实实现。**不注入**任何一项都会落到降级路径（内存兜底
/// 后端 / 空直通上下文 / 未配置模型服务 / 一个监听器都不注册），集成测试从此测的
/// 就不是生产形态（ADR-049 序 4 起的一贯纪律）。
///
/// 未装入的两个插槽是 [`full`] 追加的**环境相关**能力：动态工具清单（`create_app`
/// 路径不注册）与真实向量存储（LanceDB 会为每个测试建目录）。这是**测试剖面**，
/// 不是「生产少装两块」。
///
/// 期 4b 起清单按**四组**显式构造（`manifests` / `storage` / `policies` /
/// `providers`）：分组理由见 `tianyan_server::assembly` 的 crate 文档，本函数只声明
/// 「哪一组装什么」——`manifests` 装 `task_factory` 与 `tool_pipeline_factory`（两份
/// 有序清单；第三份 `plugin_factory` 是环境相关的，只在 [`full`] 里追加）。
pub fn policies() -> AssemblyServices {
    AssemblyServices {
        manifests: ManifestAssemblies::default()
            .with_task_factory(task_factory())
            .with_tool_pipeline_factory(tool_pipeline_factory()),
        storage: StorageAssemblies::default().with_storage_factory(storage_factory()),
        policies: PolicyAssemblies::default()
            .with_approval_factory(approval_factory())
            .with_context_factory(context_factory())
            .with_task_reviewer_factory(task_reviewer_factory())
            .with_skill_review_factory(skill_review_factory()),
        providers: ProviderAssemblies::default()
            .with_provider_llm_factory(provider_llm_factory())
            .with_provider_embed_factory(provider_embed_factory()),
    }
}

/// **完整生产装配**（11 个插槽）：`app` bin 与 `tauri` 桌面端用它。
///
/// = [`policies`]（9 个） + 动态工具清单 + 向量存储。写成前缀关系是刻意的：
/// 两份清单不可能分叉（加一个策略插槽只需改 `policies()`，两个消费者同时得到）。
///
/// 装配顺序对语义无影响（`AssemblyServices` 是字段独立的 builder），但**注册**顺序
/// 有影响——见 crate 文档的三条顺序纪律。
///
/// 实现上**解构** [`policies`] 的四个组再显式重建（而不是 `..policies()`）：
/// `AssemblyServices` 的字段是四个组结构，部分移动的写法在「给某组追加插槽、其余组
/// 原样带走」时会踩到部分移动歧义，显式解构让「哪组多了什么」在代码里可见——期 4b
/// 追加的 2 个插槽分别落在 `manifests`（`plugin_factory`）与 `storage`
/// （`vector_factory`），两组的分组理由见 `tianyan_server::assembly` 的 crate 文档。
pub fn full() -> AssemblyServices {
    let AssemblyServices {
        manifests,
        storage,
        policies,
        providers,
    } = policies();
    AssemblyServices {
        manifests: manifests.with_plugin_factory(plugin_factory()),
        storage: storage.with_vector_factory(vector_factory()),
        policies,
        providers,
    }
}

#[cfg(test)]
mod tests {
    // 测试里 `expect` 是断言手段（失败即测试失败并带消息），与项目其余测试文件
    // （`app/tests/assembly.rs` / `server/src/assembly/tests.rs` 等）同口径。
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use tianyan::scheduler::{TaskContext, TaskHandler, TaskResult};

    /// 只用于顺序断言的占位处理器（名字由构造点给定）。
    struct StubTask(&'static str);

    #[async_trait::async_trait]
    impl TaskHandler for StubTask {
        async fn execute(&self, _ctx: &TaskContext) -> TaskResult {
            TaskResult::success(0)
        }

        fn name(&self) -> &str {
            self.0
        }
    }

    fn reg(name: &'static str) -> TaskRegistration {
        TaskRegistration::new(name, name, 60, Arc::new(StubTask(name)))
    }

    fn task_names(regs: &[TaskRegistration]) -> Vec<&str> {
        regs.iter().map(|r| r.name.as_str()).collect()
    }

    /// 顺序纪律 ②：演化任务**紧跟** `summary_generation`（清单第 2 位）。
    ///
    /// 判别力：把 `merge_task_lists` 的 `splice(1..1, …)` 改成 `splice(2..2, …)`
    /// 或改成 `extend`（排到末尾）都会红——这正是「跨插件位置只有装配层知道」
    /// 必须被锁住的原因（位置错了不会编译失败，只会让演化任务排到垃圾回收之后）。
    #[test]
    fn test_merge_task_lists_puts_evolution_second() {
        let mut base = vec![
            reg("summary_generation"),
            reg("garbage_collection"),
            reg("usage_stats_flush"),
        ];
        merge_task_lists(&mut base, vec![reg("evolution")]);

        assert_eq!(
            task_names(&base),
            vec![
                "summary_generation",
                "evolution",
                "garbage_collection",
                "usage_stats_flush",
            ],
            "演化任务必须排第 2 位（紧跟 summary_generation）"
        );
    }

    /// 演化插件未启用（返回空清单）时，合并必须是**恒等**操作。
    ///
    /// 判别力：若实现写成「先 `insert` 一个占位再替换」，空清单会留下痕迹
    /// （内置清单被截断或错位）。
    #[test]
    fn test_merge_task_lists_is_identity_when_evolution_absent() {
        let mut base = vec![reg("summary_generation"), reg("garbage_collection")];
        merge_task_lists(&mut base, Vec::new());

        assert_eq!(
            task_names(&base),
            vec!["summary_generation", "garbage_collection"],
            "演化任务未启用时内置清单顺序与内容都不得变化"
        );
    }

    /// 顺序纪律 ①：工具注册顺序 = `fs → code → todo/goal → web`。
    ///
    /// 工具清单会进 system 提示词的工具目录，顺序抖动会让整个前缀缓存失效
    /// （ADR-004 前缀零漂移），因此这条顺序必须有断言而不是靠注释。
    ///
    /// 端到端跑真实插件清单（不 mock）：`ToolContext::new` 与两个 store 的构造都
    /// 不 IO，`web` 客户端在默认配置下可构造——成本远低于它的判别力。
    #[test]
    fn test_tool_order_is_fs_code_todo_web() {
        let ctx = tianyan::agent::ToolContext::new(tianyan::executor::SecurityPolicy::default());
        let dir = std::env::temp_dir().join("tianyan-assembly-tool-order");
        let services = PluginServices::new(
            Arc::new(tianyan::todos::TodoStore::new(&dir)),
            Arc::new(tianyan::goals::GoalStore::new(&dir)),
            tianyan::config::WebConfig::default(),
        );

        let tools = plugin_factory()(&ctx, services);
        let names: Vec<String> = tools.iter().map(|t| t.tool_name()).collect();

        // 前 11 个是 fs(6) + code(3) + todo/goal(2)：无条件注册，逐位断言。
        // 顺序以**实测注册顺序**为准（不是源码里的函数排列顺序）：fs 域内部是
        // read/write/edit/patch → glob/list_dir（与 `ToolPresentation` 表的枚举顺序
        // 和迁移前 `MIGRATED_TOOL_PRESENTATIONS` 一致）。
        let expected_head = [
            "read_file",
            "write_file",
            "apply_edit",
            "apply_patch",
            "glob",
            "list_dir",
            "symbol_outline",
            "repo_map",
            "grep",
            "todo",
            "goal",
        ];
        assert!(
            names.len() >= expected_head.len(),
            "插件工具不得少于 {} 个（fs/code/todo 无条件注册），实际 {:?}",
            expected_head.len(),
            names
        );
        assert_eq!(
            names[..expected_head.len()]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected_head.to_vec(),
            "工具注册顺序必须是 fs → code → todo/goal（前缀缓存生命线）"
        );

        // web 两个工具在默认 Web 配置下可构造（插件在配置非法时才降级为空清单）；
        // 断言它们**在末尾且顺序固定**，而不是断言总数——降级是插件的合法行为，
        // 不该让本断言变成「配置校验测试」。
        if names.len() > expected_head.len() {
            assert_eq!(
                names[expected_head.len()..]
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                vec!["web_search", "web_fetch"],
                "web 工具必须注册在最后，且顺序固定 web_search → web_fetch"
            );
        }
    }

    /// 两份清单的前缀关系：`full()` 比 `policies()` 多且只多两个环境相关插槽。
    ///
    /// 判别力：若有人在 `full()` 里重复写一遍 9 个 `with_*`（而不是复用
    /// `policies()`），两份清单就会各自漂移——本测试无法直接看见字段（`private`），
    /// 因此用**行为**断言：两者都能构造成功，且 `policies()` 不含插件工具 / 向量
    /// 存储（`plugin_factory` 访问器是 `pub(crate)`，只能从 server 侧看；这里以
    /// 「构造不 panic + `full()` 可被重复调用」作为最小回归）。
    ///
    /// 更强的字段级断言属 server 的插槽测试（`assembly.rs` 里已有「未装配 → 空清单 /
    /// Noop 兜底」的判别力测试），本 crate 不重复造第二套。
    #[test]
    fn test_full_and_policies_are_constructible_and_repeatable() {
        let _ = policies();
        let _ = full();
        // 工厂可重复调用（数据目录搬迁 / agent 热重载会再次装配）
        let _ = full();
    }

    /// 装配回归（**迁自** `app/tests/assembly.rs`，ADR-049 期 4）：向量存储工厂必须
    /// 真的产出 **LanceDB**，而不是 server 侧「未装配」时的空降级实现。
    ///
    /// **为什么迁到本 crate**：它测的就是[`vector_factory`]这个实现。留在 `app` 包里
    /// 就必须把工厂再写一遍（改造前正是如此：`app/tests/assembly.rs` 里一份闭包副本
    /// 只为了证明「与 `app/src/main.rs::vector_factory` 同形」）——测试与被测对象
    /// 分居两包，必然靠副本维系。
    ///
    /// 判别力（逐字沿用迁移前的三条）：
    /// 1. 证明 `VectorStorageFactory` 的类型擦除链（闭包 → `Arc<dyn Fn>` → boxed
    ///    future）真的编译得过；
    /// 2. 证明 `{data_dir}/lancedb` 被真实创建——空实现不会在磁盘留痕；
    /// 3. 维度来自 `StorageConfig`（8），说明配置被正确传入而非默认值。
    #[tokio::test]
    async fn test_vector_factory_yields_real_lancedb_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = tianyan::config::StorageConfig {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        config.vector.collection_name = "assembly_test".to_string();
        config.vector.vector_dimension = 8;

        let store = vector_factory()(&config).await.expect("工厂应构造成功");
        assert_eq!(store.embedding_dim(), 8, "维度应来自注入的配置");
        assert_eq!(store.count_rows().await.expect("行数可读"), 0);
        assert!(
            dir.path().join("lancedb").is_dir(),
            "LanceDB 数据目录应被创建（空实现不留痕）"
        );
    }

    /// 装配回归（**迁自** `app/tests/assembly.rs`，ADR-049 期 4）：结构化存储工厂必须
    /// 按 `config.storage.backend` 真的产出插件里的实现，而不是 server 侧「未装配」时
    /// 的内存兜底。
    ///
    /// 判别力（逐字沿用迁移前的四条）：
    /// 1. 同步工厂的类型擦除链真的编译得过；
    /// 2. `Sqlite` 分支写入的条目能被**另开一条连接到同一数据库文件**的后端读回——
    ///    内存兜底不落盘，`exists` 必为 false；
    /// 3. `Local` 分支会真的在磁盘上建出数据目录；
    /// 4. 两个分支共用同一份 `StorageConfig`，说明 adapter 选择发生在装配层（server
    ///    库只递配置与共享连接，不认识这两个类型）。
    #[tokio::test]
    async fn test_storage_factory_yields_real_backend_per_config() {
        use tianyan::config::StorageBackendType;
        use tianyan::vfs::ContextEntry;

        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("tianyan.db");

        // ── Sqlite 分支：写入必须落到数据库文件 ──────────────────────────
        let sqlite_config = tianyan::config::StorageConfig {
            data_dir: dir.path().to_path_buf(),
            backend: StorageBackendType::Sqlite,
            sqlite_path: Some(db_path.clone()),
            ..Default::default()
        };
        let database = tianyan::db::Database::open(db_path.clone()).expect("打开数据库");
        database.init_schemas().await.expect("初始化 schema");
        let backend = storage_factory()(&sqlite_config, database).expect("Sqlite 分支应构造成功");
        backend.initialize().await.expect("建 vfs_entries 表");
        let uri = tianyan::TianyanUri::new(
            tianyan::ContextNamespace::Knowledge,
            vec!["assembly_probe.md".to_string()],
        );
        backend
            .write_entry(&ContextEntry::new_file(uri.clone()))
            .await
            .expect("写条目");
        drop(backend);

        // 另开一条到同一文件的连接读回（内存兜底不会有任何落盘痕迹）
        let reopened = tianyan::db::Database::open(db_path).expect("重开数据库");
        reopened.init_schemas().await.expect("重开初始化 schema");
        let reopened_backend = storage_factory()(&sqlite_config, reopened)
            .expect("Sqlite 分支应可重复构造（重启/搬迁）");
        assert!(
            reopened_backend.exists(&uri).await.expect("存在性查询"),
            "Sqlite 分支必须落盘（未装配时会退化成内存后端，此处读回为空）"
        );

        // ── Local 分支：必须真的在磁盘上建目录 ──────────────────────────
        let local_dir = dir.path().join("local-data");
        let local_config = tianyan::config::StorageConfig {
            data_dir: local_dir.clone(),
            backend: StorageBackendType::Local,
            sqlite_path: None,
            ..Default::default()
        };
        let database =
            tianyan::db::Database::open(dir.path().join("tianyan2.db")).expect("打开数据库");
        let local_backend =
            storage_factory()(&local_config, database).expect("Local 分支应构造成功");
        assert!(
            !local_dir.exists(),
            "前置：构造阶段不做 IO（建目录在 initialize 里）"
        );
        local_backend.initialize().await.expect("建数据目录");
        assert!(
            local_dir.is_dir(),
            "Local 分支应建出数据目录（内存兜底不会留痕）"
        );
    }
}
