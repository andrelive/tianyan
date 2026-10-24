//! 上下文工程模块。**契约与组装机制留 core，策略实现已出 core（ADR-049 期 3-T2）**。
//!
//! 本模块提供上下文工程能力，包括统一的上下文存储和检索系统。
//!
//! 按职责三分（ADR-049「期 3 普查结论 · ★ 推翻前提 1」修订了 D7 原文）：
//!
//! - [`assembler`]：**组装机制（K1）** —— `ContextAssembler` 是纯静态函数、零 I/O，
//!   承载 ADR-043「工具对连续性规范化」与「前缀零漂移」（soul → project → rules →
//!   history 的字节序列决定 prompt 缓存命中率）。`agent/loop.rs` 与
//!   `agent/agent_core.rs` 直接持有它，**留 core**；
//! - [`port`]：**契约** —— [`ContextPort`]（3 个方法，逐个有调用点证据）+
//!   未装配时的降级实现 [`NullContextPort`]；
//! - [`assembly`]：**装配原语** —— [`ContextServices`]（插件构造所需的
//!   能力集合），与 `scheduler::TaskServices` 同一形态。
//!
//! 管线（注入顺序 / 检索 / 压缩）、压缩器、双层检索器、意图分类、RRF 参数已迁入
//! `plugins/policy-context`（crate `tianyan-policy-context`）：改 prompt 模板 / 压缩阈值 /
//! 检索策略 / 意图分类关键词不再重编内核（ADR-049 D7：按「变化原因」切）。
//! 装配层经 `ContextFactory` 注入实例，未装配时 `AgentBuilder::build` 用
//! [`NullContextPort`] 兜底 + `warn!` 指路。
//!
//! 注：`tianyan::context::ContextAssembler` 的公开路径不变（`loop.rs` /
//! `agent_core.rs` 零改动）；原 `tianyan::context::{ContextPipeline, CompressionConfig,
//! DualLayerRetriever, …}` 的导出随实现搬到 `tianyan_policy_context::…`。

pub mod assembler;
pub mod assembly;
mod port;
/// 组装期按引用加载 tool 结果图片（ADR-050：`session_images/` → data URL）。
pub mod tool_images;

pub use assembler::ContextAssembler;
pub use assembly::ContextServices;
pub use port::{ContextPort, NullContextPort};
pub use tool_images::SessionImages;

// ── 测试期同源包含：策略实现的**唯一副本仍在插件里** ────────────────────────
//
// core 的单测（`cargo test -p tianyan-core --lib`）必须能构造**真实**管线与压缩器：
// `agent_core.rs` 的 12 个用例锁的是「压缩落库 → 刷新注入缓存 → 工作集段收缩」的
// 端到端判别力（压缩点内容必须来自压缩请求、空摘要必须丢弃且不改写对话、唤醒轮也
// 执行压缩检查、`reshape_after_compression` 只在落库成功后发生）。用替身会把这些
// 语义抹平成「永远不压缩」，等于放弃断言。
//
// 为什么不能靠 `[dev-dependencies] tianyan-policy-context`：单测要把 core 的类型
// （`InjectableContext` / `StructuredMessage` / `dyn ContextPort`）传进插件 API，
// 而 `cfg(test)` 的 core 与插件依赖的 core 是**两个编译单元**，类型与 trait 随之出现
// 两个互不兼容的实例（ADR-049 期 2 序 4 实测过 22 处 E0308/E0277）。
//
// 所以测试期用 `#[path]` 把插件源码**同源包含**进来（与 `vfs/backend/mod.rs` 的
// `storage-sqlite`、`executor/approval/mod.rs` 的 `policy-approval` 同一手法）：
// 一份源码、零复制、零漂移；正常构建（`build`/`check`/发布）完全不参与——core 的
// 生产依赖图里没有任何插件。
//
// 模块树**形状与插件一致**（`context::{pipeline, compression, retrieval}`）：被包含的
// 文件内部用 `crate::context::…` 引用兄弟模块，于是同一份源码在插件里解析到插件自身的
// 模块、在这里解析到这些同源包含的模块。由此产生两条硬纪律：
//   1. 被包含的文件只允许 `tianyan::…`（core 的类型）与 `crate::context::…`
//      （本模块树）两种路径前缀，不得出现其它 `crate::…`；
//   2. 它们的**测试模块也是同源跑一遍**（与 `storage-sqlite` 先例一致：core 的
//      `--lib` 计数不变，插件侧再独立跑一遍）。
#[cfg(test)]
#[path = "../../../plugins/policy-context/src/context/pipeline.rs"]
pub mod pipeline;

#[cfg(test)]
#[path = "../../../plugins/policy-context/src/context/compression/mod.rs"]
pub mod compression;

#[cfg(test)]
#[path = "../../../plugins/policy-context/src/context/retrieval/mod.rs"]
pub mod retrieval;

// 测试期便利导出：core 的单测按**原路径**（`crate::context::ContextPipeline`）构造真实
// 管线，与迁移前的写法逐字一致（迁移只改了「代码住哪儿」，不改测试怎么读它）。
#[cfg(test)]
pub use pipeline::ContextPipeline;
