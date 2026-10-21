//! policy-context 插件：上下文工程策略（注入 / 检索 / 压缩）的内核外实现。
//!
//! ## 为什么独立成 crate（ADR-049 期 3-T2，**期 3 的主体收益**）
//!
//! `core/src/context/` 里除了组装器（见下一节）之外的三块——上下文管线
//! （`pipeline.rs`）、压缩（`compression/`）、检索（`retrieval/`，含意图分类与
//! RRF 融合）——是**上下文策略**：注入哪些内容、按什么顺序、召回多少条、
//! 分数怎么偏置、什么时候压缩、摘要提示词长什么样、阈值取 0.6 还是 0.5。
//!
//! 这些东西的变化原因与内核机制的变化原因完全不重叠（ADR-049 D7：按「变化原因」
//! 切）。此前它们是 `tianyan-core` 的一部分，于是：
//!
//! - 改一句摘要提示词（`DEFAULT_SUMMARY_PROMPT`）→ 重编 `tianyan-core`（42.4K 生产行）
//!   + `tianyan-server`（13.4K）+ 全部下游 crate；
//! - 调一个压缩阈值（`compression_threshold`）或 `preserve_recent_messages` → 同上；
//! - 加一个意图分类关键词（`category_keywords`）→ 同上；
//! - 改 RRF 的 `memory_bias` / 新鲜度权重 → 同上。
//!
//! 搬进本 crate 后，这些改动只重编 **本 crate + 装配包**（`app` / `tauri`）；
//! 内核与通道层不动。这就是 ADR-049 承诺的「改策略不重编内核」，也是期 3 的主体收益
//! （T1 审批策略只搬了 725 行；T2 搬 1,729 行生产代码 + 1,755 行测试）。
//!
//! ## 为什么 `assembler.rs` 留 core（**不是遗漏**；D7 原文已修订）
//!
//! D7 原文写「`context`（3.4K）是干净的策略簇，可直接成为 `policy-context` 插件，
//! 不必动内核」——**期 3 普查推翻了这条前提**（ADR-049「期 3 普查结论 · ★ 推翻
//! 前提 1」）。`context/assembler.rs`（341 生产行）不是策略，是 K1 的**组装机制**：
//!
//! - 它是**纯静态函数**（`assemble` / `message_to_structured` / `normalize_tool_pairs` /
//!   `structured_to_messages`），零 I/O、零状态；
//! - `agent/loop.rs:13` 与 `agent/agent_core.rs:26` **直接 `use`** 它，生产调用点
//!   `agent_core.rs:655/762/764/1215` 与 `loop.rs:1452` 都在循环机制里；
//! - 它承载两条正确性约束：**ADR-043 工具对连续性规范化**（断裂的 `tool_calls` 链
//!   会让上游 400）与**前缀零漂移**（soul → project → rules → history 的顺序决定
//!   prompt 缓存命中率）。
//!
//! 把机制搬进插件，等于让内核反向依赖插件（破 D2-1/D2-3）。因此切法是
//! **assembler 留 kernel-impl、其余 1,729 行进本插件**，内核只认
//! `tianyan::context::ContextPort`（3 个方法，逐个有调用点证据）。
//!
//! ## 为什么不抽 `UsageReporter` trait
//!
//! `retrieval/retriever.rs` 持 `Option<Arc<UsageStats>>` 并直接写 SQLite 统计
//! （`record_doc_hit` / `record_search_query` / `record_doc_load`）。普查曾建议先给
//! kernel-api 一个 `UsageReporter` trait，否则「插件会依赖内核实现」（D2-1）。
//!
//! 本批**不做**，理由：
//!
//! 1. 今天 core 就是唯一的 kernel 包——**所有插件都直接依赖它**（`storage-sqlite` /
//!    `task-builtin` / `task-evolution` / `policy-approval` / `tools-*` 无一例外，
//!    仓库里根本还没有「kernel-api vs kernel-impl」这条缝可依赖）；
//! 2. `UsageStats` 只有一个实现，此刻抽 trait 没有第二实现能压测其形状——是 YAGNI：
//!    形状错了要等有第二个实现（或 T5 切包）时才会暴露，那时改 trait 的成本远高于现在；
//! 3. 正确时机是 **T5 把 core 切成 `kernel-api` / `kernel-impl`**：那时 `UsageStats`
//!    的归属（api 侧写统计契约 + impl 侧 SQLite 实现）与 `WorkingSetPort` 要一起定，
//!    一次做完才有真实的 seam 需求。
//!
//! 所以本 crate 直接 `use tianyan::observability::usage_stats::UsageStats`——与其他
//! 插件对 core 的依赖同一形态，不新增契约负担（决策与理由已记入 ADR-049 遗留）。
//!
//! ## 顺序契约（**留在内核；插件只产出压缩结果**）
//!
//! `Agent::maybe_compress_and_persist`（`agent_core.rs:807-818`）：
//! `compress_for_session` 返回的 `StructuredMessage` 带 `compression_marker`；
//! **只有在摘要落库成功之后**才允许 `ws.reshape_after_compression()`（ADR-035 §2 段化）。
//! 顺序颠倒会让失败路径也推进消费水位——把尚未注入的异步通知误标为已消费，且段起点
//! 前移到库中并不存在的压缩点。
//!
//! 这条契约**不在本 crate**：插件只回答「压缩结果是什么」，落库与收缩由内核按序执行；
//! 因此 `ContextPort` 的实现不知道 `WorkingSet` 的存在。
//!
//! ## 降级语义（未装配插槽时）
//!
//! 装配层未注入 `ContextFactory` 时，`AgentBuilder::build` 用 core 的
//! `NullContextPort`（内存直通：可注入上下文为空、不压缩、摘要提取恒 `None`），并
//! `warn!` 一条指路。为什么允许降级而不是 `Err`：`create_app`（构造对象图，47 处
//! lib 单测 + 嵌入式）不要求装配；`start_server*` 另有一道启动期硬守卫（与结构化存储
//! 同一手法），生产装配必须注入。**代价必须可见**（ADR-038）：装配期 `warn!` +
//! `build()` 里再 `warn!` 一次。
//!
//! ## 测试
//!
//! 65 个用例随实现整体迁入（pipeline 16 / compression 17 / retrieval 31），
//! 逐个同名同断言；另加 3 条装配入口门禁（[`pipeline`] 的窗口联动 / 缺省窗口 /
//! 端口冒烟）。core 侧**同源包含**同样的文件（`core/src/context/mod.rs` 的
//! `#[cfg(test)] #[path]`），因此 core 的 `--lib` 计数不变、判别力不减；本 crate
//! 自己再跑一遍（`cargo test -p tianyan-policy-context`）。
//!
//! **源码纪律**：被同源包含的这些文件只允许 `tianyan::…` 绝对路径（core 的类型）与
//! `crate::context::…`（本 crate 的同名模块树）；**不得**出现 `crate::` 的其他用法
//! ——core 的测试构建里 `crate::` 会解析到 core 而非本 crate。
//!
//! ## 公开面（13 个符号）
//!
//! 与迁移前 `tianyan::context::` 的导出集等价（`ContextAssembler` 除外，它留 core），
//! 另加装配入口 [`pipeline`]。反指标要求「每插件 crate 公开符号 ≤ 15」，本 crate 为 13（12 个实现符号 + 装配入口 pipeline）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// 上下文工程策略实现（管线 / 压缩 / 检索）。
///
/// 模块树形状刻意与 core 的 `context` 一致（见模块文档）：core 的单元测试用
/// `#[path]` 同源包含本目录下的文件，`crate::context::…` 的相对路径因此两边都成立。
pub mod context;

use std::sync::Arc;

use tianyan::context::{ContextPort, ContextServices};

pub use context::{
    estimate_tokens, CompressionConfig, CompressionResult, CompressionStatus, CompressionStrategy,
    ContextCompressor, ContextPipeline, DualLayerRetriever, Intent, IntentAnalyzer,
    RetrievalResult, TokenEstimator,
};

/// 构造上下文端口实例（**装配期唯一入口**）。
///
/// 把「上下文策略怎么配」的三件事收在本 crate 里（迁移前它们散在
/// `AgentBuilder::build` 与 `server/src/agent_builder.rs`）：
///
/// 1. **检索器**：`DualLayerRetriever::new(vfs).with_usage_stats(usage_stats)`——
///    文档命中 / 检索 / 加载写统计（迁移前在 `server/src/agent_builder.rs` 里建）；
/// 2. **压缩器**：`context_window` **联动聊天模型规格**（`chat_spec.context_length`；
///    无 spec 时保持默认 128_000）、`preserve_recent_messages = 6`、
///    `summary_model = chat_model`（迁移前在 `AgentBuilder::build` 里建）；
/// 3. **管线**：`default_top_k` / `learned_rules_top_k` 取自配置（同上）。
///
/// 三处装配（`app` / `tauri` / 集成测试的 `test_assembly()`）因此各写一行包装
/// （`|s| tianyan_policy_context::pipeline(s)`），不会各抄一份构造参数——那是
/// 「两份口径同时漂移」的经典来源（与 `tianyan_policy_approval::gate` 同一手法）。
///
/// **同步**（与 `ApprovalGateFactory` / `TaskFactory` 同理）：构造只存字段与句柄
/// （`ContextPipeline::new` / `ContextCompressor::new` / `DualLayerRetriever::new`
/// 都不做 IO），做成 async 只会让调用方多 await 一个纯内存操作。
pub fn pipeline(services: &ContextServices) -> Arc<dyn ContextPort> {
    build_pipeline(services)
}

/// 管线构造主体（保留具体类型：装配入口只做类型擦除，测试要能核对压缩窗口联动）。
fn build_pipeline(services: &ContextServices) -> Arc<ContextPipeline> {
    let retriever = Arc::new(
        DualLayerRetriever::new(services.vfs.clone())
            .with_usage_stats(services.usage_stats.clone()),
    );
    let compressor = Arc::new(tokio::sync::Mutex::new(ContextCompressor::new(
        services.chat.clone(),
        CompressionConfig {
            // 模型规格联动压缩窗口；无 spec 时保持默认 128_000（语义与
            // DEFAULT_CONTEXT_WINDOW 一致，不能依赖 ..Default::default() 覆盖）。
            context_window: services
                .chat_spec
                .map(|s| s.context_length)
                .unwrap_or(128_000),
            preserve_recent_messages: 6,
            summary_model: services.chat_model.clone(),
            ..CompressionConfig::default()
        },
    )));
    Arc::new(ContextPipeline::new(
        services.vfs.clone(),
        retriever,
        compressor,
        services.config.agent.default_top_k,
        services.config.agent.learned_rules_top_k,
    ))
}

#[cfg(test)]
mod pipeline_tests {
    //! 装配入口的判别力测试（**从 core 的 `builder.rs` 迁来**，ADR-049 期 3-T2）。
    //!
    //! 原先这两个断言住在 `core/src/agent/builder.rs`
    //! （`test_with_chat_model_spec_sets_loop_spec_and_window` /
    //! `test_no_chat_model_spec_defaults_window`），靠
    //! `agent.context_pipeline.compressor().get_status(0).context_window` 穿透具体类型
    //! 断言「chat_spec 与压缩器窗口联动」。管线搬走后 core 只剩 `Arc<dyn ContextPort>`，
    //! 而**联动逻辑本身住在本 crate**（[`build_pipeline`]），所以断言跟着逻辑走：
    //! 这里测「窗口联动」，core 侧的 builder 测试改测「接线」（spec 是否注入 AgentLoop、
    //! 装配的 port 是否就是注入的那个）。断言强度不降：窗口值仍是精确相等断言。

    use super::*;

    use tianyan::config::TianyanConfig;
    use tianyan::model::spec::ModelSpec;
    use tianyan::test_utils::{MockChatService, MockVfs};
    use tianyan::vfs::VirtualFileSystem;

    /// 构造能力集合（除 spec 外全部用最小替身）。
    fn services(chat_spec: Option<ModelSpec>) -> ContextServices {
        let vfs: Arc<dyn VirtualFileSystem> = Arc::new(MockVfs::new());
        let database = tianyan::db::Database::open_in_memory().expect("内存库");
        ContextServices {
            config: TianyanConfig::default(),
            vfs,
            usage_stats: tianyan::observability::usage_stats::UsageStats::new(database)
                .expect("统计"),
            chat: Arc::new(MockChatService::new()),
            chat_model: "test-model".to_string(),
            chat_spec,
        }
    }

    /// 有 spec → 压缩器 `context_window` 联动 `spec.context_length`（**逐字迁移的断言**）。
    ///
    /// 判别力：把 [`build_pipeline`] 的 `unwrap_or(128_000)` 写死、或漏掉
    /// `context_window` 字段而依赖 `..Default::default()`（= 128_000），本断言必红。
    #[tokio::test]
    async fn test_pipeline_links_compressor_window_to_chat_spec() {
        let spec = ModelSpec {
            context_length: 200_000,
            max_output_tokens: 16_000,
            max_input_tokens: 184_000,
        };
        let pipeline = build_pipeline(&services(Some(spec)));
        let status = pipeline.compressor().lock().await.get_status(0);
        assert_eq!(
            status.context_window, spec.context_length,
            "压缩器 context_window 应与 spec.context_length 联动"
        );
    }

    /// 无 spec → 压缩器窗口保持默认 128_000（**逐字迁移的断言**）。
    ///
    /// 判别力：`unwrap_or(128_000)` 若被改成别的缺省（或误用 `0`），本断言必红。
    #[tokio::test]
    async fn test_pipeline_defaults_compressor_window_without_spec() {
        let pipeline = build_pipeline(&services(None));
        let status = pipeline.compressor().lock().await.get_status(0);
        assert_eq!(status.context_window, 128_000);
    }

    /// 装配入口返回的端口真的连着这份管线（类型擦除之后仍可观测）：
    /// 空会话 + 0 token（远低于阈值）→ 不压缩；空 VFS → 注入上下文为空。
    #[tokio::test]
    async fn test_pipeline_port_smoke() {
        let port = pipeline(&services(None));
        assert!(
            port.compress_for_session(&[], "s1", 0, false, None)
                .await
                .is_none(),
            "低于阈值不该压缩"
        );
        let empty = port
            .load_injectable("q", None)
            .await
            .expect("空 VFS 不报错");
        assert!(empty.soul.is_empty());
    }
}
