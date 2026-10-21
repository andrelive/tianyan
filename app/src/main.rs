//! 天演装配器（服务器二进制入口）。
//!
//! ## 为什么单独成包（ADR-049 期 1）
//!
//! **装配清单住在 bin**。Cargo 的依赖粒度是 package，若清单写在
//! `tianyan-server` 的 bin 里，整个 server 包就依赖插件 crate —— 改一行工具会
//! 重编 server 库（13.4K 源码），闸门 A（≤3.5K）直接失效。
//!
//! 因此本包只做三件事：加载配置与日志、给出插件清单、启动 server。
//! server 库通过 `ToolPluginFactory`（类型擦除）接收清单，**不认识任何插件类型**。
//!
//! 期 2 序 3 起同一模式也用于**向量存储**：`lancedb`/`arrow`/`datafusion` 链搬进
//! `plugins/storage-vector` 后，core 不再承担它；本包把 `LanceDbVectorStore` 包成
//! `VectorStorageFactory` 注入启动入口（server 库同样不认识该实现）。
//!
//! 期 2 序 4 再扩一个插槽：VFS 的**结构化存储**（`SqliteBackend` /
//! `LocalFileBackend`）搬进 `plugins/storage-sqlite`，本包把两者包成
//! `StorageBackendFactory` 注入——`config.storage.backend` 选哪个 adapter 也在这里
//! 决定（server 库只递配置与共享连接）。
//!
//! 期 2 序 5a 扩第四插槽：**内置定时任务**（摘要 / GC / 统计刷盘 / 快照 GC / 提醒）
//! 搬进 `plugins/task-builtin`，本包把插件的 `tasks()` 包成 `TaskFactory` 注入
//! （server 库只认清单项类型，不认识任何具体任务）。
//!
//! 期 2 序 5b 该插槽收尾：**演化任务**搬进 `plugins/task-evolution`；两个插件的清单
//! 在 `task_factory()` 里**拼接**（`splice(1..1, …)`：演化任务紧跟 `summary_generation`）
//! ——跨插件顺序只有装配清单知道，插件之间不互相 `import`（D2-2）。
//!
//! 期 3-T1 扩第五插槽：**审批策略**（风险定级 / 自动规则 / 模式短路 / 人工审批挂起 /
//! 审计）搬进 `plugins/policy-approval`，本包把插件的 `gate()` 包成
//! `ApprovalGateFactory` 注入——server 库只认 `Arc<dyn ApprovalGate>`，不认识审批策略。
//!
//! 期 3-T2 扩第六插槽：**上下文策略**（注入管线 / 双层检索 RRF / 意图分类 / 分层压缩）
//! 搬进 `plugins/policy-context`，本包把插件的 `pipeline()` 包成 `ContextFactory` 注入
//! ——server 库只认 `Arc<dyn ContextPort>`，不认识上下文策略。这是期 3 的主体收益：
//! 改 prompt 模板 / 压缩阈值 / 检索参数不再重编内核与通道层。

use std::sync::Arc;

use tianyan::vfs::{StorageBackend, VectorStorage};
use tianyan_server::{
    start_server_with_assembly, ApprovalGateFactory, AssemblyServices, ContextFactory,
    ProviderEmbedFactory, ProviderLlmFactory, SkillReviewFactory, StorageBackendFactory,
    TaskFactory, TaskReviewerFactory, ToolPluginFactory, VectorStorageFactory,
};
use tianyan_storage_sqlite::{LocalFileBackend, SqliteBackend};
use tianyan_storage_vector::LanceDbVectorStore;

/// 插件清单（**全仓唯一 `use` 具体插件类型的地方**）。
///
/// 新增插件在此追加一行；插件内部顺序即注册顺序（追加到动态工具末尾，
/// 内置工具顺序不受影响 —— 提示词前缀稳定性）。
fn plugin_factory() -> ToolPluginFactory {
    Arc::new(|ctx, services| {
        // 顺序即注册顺序（追加到动态工具末尾）：fs → code → todo/goal → web。
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
        // `web_search` / `web_fetch`（ADR-049 期 2）：客户端由插件从
        // `services.web_config()` 就地构造（配置非法时降级为空清单，装配不
        // panic）。顺序固定在最后（web）。
        tools.extend(tianyan_tools_web::tools(&services.web_config()));
        tools
    })
}

/// 向量存储工厂（LanceDB；`plugins/storage-vector`）。
///
/// 类型擦除的同款手法：server 库只认 [`VectorStorageFactory`]（`Arc<dyn Fn>`），
/// `LanceDbVectorStore` 只在本包出现。工厂**可重复调用**（数据目录搬迁后以新配置
/// 重启会再次装配）；`&StorageConfig` 是借用，需先 `clone` 再 move 进 async 块。
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
/// 同步工厂（两个后端的 `new` 都只存字段，建表/建目录在 `initialize` 里）。
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

/// 定时任务工厂（`plugins/task-builtin` 序 5a + `plugins/task-evolution` 序 5b）。
///
/// 5 个内置任务（摘要生成 / 垃圾回收 / 统计刷盘 / 快照 GC / 主动提醒）与自演化综述
/// 任务的实现与**注册清单**（名字 / 显示名 / 间隔 / 启用条件）都在插件里；本包只做
/// 「拼接两个插件的清单 + 包成类型擦除的 [`TaskFactory`]」——server 库因此不认识任何
/// 具体任务。
///
/// **★ 顺序拼接为什么在装配层**：注册顺序 = 扫描顺序 = 同一轮到期任务的执行顺序，
/// 而「演化任务排第 2 位（紧跟 `summary_generation`）」是**跨插件属性**：
/// `task-builtin` 不知道演化任务存在，演化插件也不知道别人排在哪，只有装配清单知道
/// 「哪个插件该排在哪」。让插件互相 import 来拼接会破 D2-2（插件之间零互赖）。
/// 因此三个角色各守其位：`tianyan_task_builtin::tasks()` 保持既有 5 项的相对顺序
/// （不为位置妥协）、`tianyan_task_evolution::tasks()` 返回 0/1 项且不关心绝对位置、
/// 合并用 `splice(1..1, …)`（**在 index 1 之前插入** = 紧跟 `summary_generation`）。
/// 用 splice 而不是 `insert` 循环，是为了让这条纪律在一行里可见，也让「演化插件未
/// 启用」时自然退化为「只有 5 项」。
///
/// **同步**工厂：能力集合（`TaskServices`）由 server 在调用前 await 好（聊天模型
/// 服务是 `AppState` 里共享 `ModelServices` 的克隆，不重复建 HTTP client 池），
/// 插件里只做「按配置构造处理器 + 判启用条件」。
fn task_factory() -> TaskFactory {
    Arc::new(|services: &tianyan_server::TaskServices| {
        let mut registrations = tianyan_task_builtin::tasks(services);
        // 顺序纪律：evolution 紧跟 summary_generation（清单第 2 位，与迁移前一致）
        registrations.splice(1..1, tianyan_task_evolution::tasks(services));
        registrations
    })
}

/// 审批门控工厂（`plugins/policy-approval`，ADR-049 期 3-T1）。
///
/// 同款类型擦除：server 库只认 [`ApprovalGateFactory`]（`Arc<dyn Fn>`），审批工作流
/// 只在本包出现。「`SecurityConfig`（模式 / 黑名单 / 白名单 / 总是询问）→ 自动审批规则」
/// 的映射住**插件**里（`tianyan_policy_approval::gate`），三处装配因此只各写一行包装，
/// 不会各抄一份映射逻辑（那是两份口径同时漂移的经典来源）。
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

/// 上下文策略工厂（`plugins/policy-context`，ADR-049 期 3-T2）。
///
/// 同款类型擦除：server 库只认 [`ContextFactory`]（`Arc<dyn Fn>`），管线 / 压缩器 /
/// 检索器只在本包出现。注入顺序、RRF 检索参数、压缩阈值与摘要提示词、意图分类关键词
/// ——以及「压缩窗口 ↔ 模型规格联动」——全部住**插件**里（`tianyan_policy_context::pipeline`），
/// 三处装配因此只各写一行包装，不会各抄一份构造参数。
///
/// 同步工厂：`ContextPipeline::new` / `ContextCompressor::new` / `DualLayerRetriever::new`
/// 都只存字段，没有任何 IO。
fn context_factory() -> ContextFactory {
    Arc::new(|services: &tianyan::context::ContextServices| {
        tianyan_policy_context::pipeline(services)
    })
}

/// 任务结果自审器工厂（`plugins/policy-review`，ADR-049 期 3-T3）。
///
/// 同款类型擦除：server 库只认 [`TaskReviewerFactory`]（`Arc<dyn Fn>`），`LlmTaskReviewer`
/// 只在本包出现。判定提示词、`VERDICT:` 解析口径、故障兜底全在插件里
/// （`tianyan_policy_review::task_reviewer`），改一句提示词不必重编内核。
///
/// 同步工厂：`LlmTaskReviewer::new` 只存（chat 客户端, 模型名）两个字段。
fn task_reviewer_factory() -> TaskReviewerFactory {
    Arc::new(|chat: Arc<dyn tianyan::model::ChatService>, model: &str| {
        tianyan_policy_review::task_reviewer(chat, model)
    })
}

/// 技能使用复审工厂（`plugins/policy-review`，ADR-049 期 3-T3）。
///
/// 同款类型擦除：server 库只认 [`SkillReviewFactory`]（`Arc<dyn Fn>`），`SkillReviewer`
/// 只在本包出现。评审记录落 VFS `skill/_reviews/<id>.jsonl`——路径与技能发现时的
/// 过滤条件（core 的 `REVIEWS_PREFIX`）同源，因此面板读得到插件写下的内容。
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

/// 聊天 / 视觉 provider 工厂（`plugins/provider-llm`，ADR-049 期 2 序 6）。
///
/// 同款类型擦除：server 库只认 [`ProviderLlmFactory`]（`Arc<dyn Fn>`），`AsyncOpenAIClient`
/// 与流式解析只在本包（间接经插件）出现。客户端构造 + 按能力解析 + 每 provider 的聊天
/// 路由池 + 连通性探针全在插件里（`tianyan_provider_llm::services`），三处装配各写一行。
///
/// 同步工厂：客户端构造（UA / 方言 / 双预算超时 / 动态会话头 / 重试策略）不发任何请求。
fn provider_llm_factory() -> ProviderLlmFactory {
    Arc::new(|config: &tianyan::config::ModelsConfig| tianyan_provider_llm::services(config))
}

/// 嵌入 provider 工厂（`plugins/provider-embed`，ADR-049 期 2 序 6）。
///
/// 产出里带**用量上报槽位**：server 拿到 `UsageLog` 后后置注入（嵌入不走 AgentLoop，
/// 其 token 用量必须单独入账）。
fn provider_embed_factory() -> ProviderEmbedFactory {
    Arc::new(|config: &tianyan::config::ModelsConfig| tianyan_provider_embed::services(config))
}

/// 完整装配（插件工具清单 + 向量存储 + 结构化存储 + 内置任务 + 审批门控 + 上下文策略
/// + 任务自审 + 技能复审 + 聊天/视觉 provider + 嵌入 provider）。
fn assembly() -> AssemblyServices {
    AssemblyServices::new()
        .with_plugin_factory(plugin_factory())
        .with_vector_factory(vector_factory())
        .with_storage_factory(storage_factory())
        .with_task_factory(task_factory())
        .with_approval_factory(approval_factory())
        .with_context_factory(context_factory())
        .with_task_reviewer_factory(task_reviewer_factory())
        .with_skill_review_factory(skill_review_factory())
        .with_provider_llm_factory(provider_llm_factory())
        .with_provider_embed_factory(provider_embed_factory())
}

#[tokio::main]
async fn main() -> tianyan::common::error::Result<()> {
    // 日志初始化只做一次（tracing 全局 subscriber 不可重复设置；
    // 搬迁重启循环复用同一日志）
    let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
        eprintln!("配置加载失败（使用默认配置）: {}", e);
        tianyan::config::TianyanConfig::default()
    });
    tianyan::common::logging::init_logging(&config.logging)?;
    // panic hook（最后防线）：panic 现场写入 <日志目录>/panic.log
    // （与桌面端同一目录：core::common::panic_hook::default_log_dir）。
    tianyan::common::panic_hook::install(tianyan::common::panic_hook::default_log_dir());

    // 主循环：数据目录搬迁后以新配置重启（搬迁由 start_server 关停后的
    // handle_pending_migration 执行；无搬迁时单次运行即退出）
    loop {
        let config = tianyan::config::TianyanConfig::load().unwrap_or_else(|e| {
            eprintln!("配置加载失败（使用默认配置）: {}", e);
            tianyan::config::TianyanConfig::default()
        });

        // 端口来源单点：from_env 读 TIANYAN_PORT（未设 → 默认 127.0.0.1:3000）。
        // e2e 借此用备用端口启动，避免与运行中的桌面应用抢占 3000。
        let server_config = tianyan_server::ServerConfig::from_env();
        match start_server_with_assembly(server_config, config.clone(), assembly()).await {
            Ok(()) => {
                // 服务器正常关停：检查是否有待处理的数据目录搬迁
                if tianyan_server::migration::handle_pending_migration(&config)
                    .await
                    .is_some()
                {
                    eprintln!("数据目录搬迁完成，以新配置重启");
                    continue;
                }
                break;
            }
            Err(e) => {
                eprintln!("服务器错误：{}", e);
                break;
            }
        }
    }
    Ok(())
}
