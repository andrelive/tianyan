//! 上下文工程契约（ADR-049 期 3-T2）。
//!
//! 内核只认本 trait，不认识任何具体上下文实现——管线 / 压缩 / 检索的实现已搬到
//! `plugins/policy-context`（crate `tianyan-policy-context`）。三个方法**逐个都有
//! 生产调用点证据**，没有「以后可能要用」的方法（反指标：每期净增 pub trait ≤ 3，
//! 本批净增 **1 个**）。
//!
//! ## 方法 → 调用点（证据，均在 `core/src/agent/agent_core.rs`）
//!
//! | 方法 | 内核侧调用点 | 语义 |
//! |---|---|---|
//! | [`ContextPort::load_injectable`] | `:478` `Agent::ensure_injectable` | 加载可注入前缀（soul + 当前环境/工作目录 + AGENTS.md + learned rules + memories）。调用方按会话缓存，`soul` 非空即视为已加载 |
//! | [`ContextPort::compress_for_session`] | `:783` `Agent::maybe_compress_and_persist` | 判定+执行压缩，产出带 `compression_marker` 的摘要消息；`None` = 不需要压缩 / 空摘要丢弃 |
//! | [`ContextPort::extract_marker_summary`] | `:751` `Agent::maybe_compress_and_persist`（取「本会话已有摘要」） | 从链上最后一个压缩点剥出纯摘要文本，作为下一次增量摘要的输入 |
//!
//! 为什么第三个方法也在 trait 上、而不是留在 core 当纯函数：**摘要的包装格式是策略**
//! （`[对话摘要] … [摘要结束]` 由本 trait 的实现在 `compress_for_session` 里写出）。
//! 剥壳算法若留 core，改一句话术就会让「已有摘要」静默提取失败（增量摘要退化为
//! 从零摘要，不报错）。产生者与消费者因此必须同 crate——与 T1 的 `action_fingerprint`
//! 同理，只是方向相反（那次是提到 core 共享，这次是留在插件共享）。
//!
//! ## 顺序契约（**在 core 执行，不在实现里**）
//!
//! `Agent::maybe_compress_and_persist`（`agent_core.rs:807-818`）：`compress_for_session`
//! 返回的 [`StructuredMessage`] 带 `compression_marker`；**只有在摘要落库成功后**才允许
//! `ws.reshape_after_compression()`（ADR-035 §2 段化，压缩是段内唯一的收缩时机）。
//!
//! 顺序颠倒的后果是具体的：失败时也收缩会推进消费水位（`reshape_after_compression`
//! 内部 `mark_consumed`），把尚未注入的异步通知误标为已消费，且段起点前移到库中并不
//! 存在的压缩点。因此**实现方不得自己触发收缩**——它连 `WorkingSetRegistry` 都拿不到。
//!
//! ## 未装配时（[`NullContextPort`]）
//!
//! 装配层未注入 `ContextFactory` 时，`AgentBuilder::build` 用 [`NullContextPort`]
//! 兜底并 `warn!` 指路（降级语义见 `plugins/policy-context/src/lib.rs`）。
//!
//! ## 与实现的关系
//!
//! 实现是 `plugins/policy-context` 的 `ContextPipeline`（`impl ContextPort for
//! ContextPipeline`，逐方法委托给同名固有方法）；装配层经 `ContextFactory`
//! （`server/src/assembly.rs` 第六插槽）注入，插槽能力集合是 [`ContextServices`]。

use async_trait::async_trait;

use crate::common::error::Result;
use crate::common::types::{InjectableContext, Message, StructuredMessage};

/// 上下文工程端口：可注入上下文的加载 + 会话压缩 + 压缩点摘要提取。
///
/// 实现方必须保持的策略语义（这些是**行为契约**，迁移前后逐字一致）：
///
/// - 注入顺序：`soul` → 当前环境（`working_directory`，渲染成「## 当前环境」段）
///   → `project_instructions`（AGENTS.md）→ `rules_and_experiences` → `memories`
///   （组装顺序由 `ContextAssembler` 决定，本端口只填充内容）。环境段必须排在
///   AGENTS.md **之前**：后者正文里写的是相对路径，先立基准模型才用得上；
/// - 压缩触发：真实 usage（最近一次请求的 prompt 侧 token）> `context_window ×
///   compression_threshold`；`force = true`（用户手动压缩）跳过阈值判定，只受消息数
///   下限保护；
/// - 空摘要（模型空响应，含单次重试后仍空）**丢弃**：返回 `None`、不改写对话——
///   链上保持原压缩点，下一轮自然重试。
#[async_trait]
pub trait ContextPort: Send + Sync {
    /// 加载可注入上下文（soul + 工作目录 + 项目指令 + learned rules + memories）。
    ///
    /// 有 I/O 开销（VFS 读 + 向量检索），调用方应在会话级缓存结果——
    /// `Agent::ensure_injectable` 以「`soul` 非空」为已加载标记。
    ///
    /// - `query` — 用户查询（规则/记忆的语义检索输入；唤醒轮传空串）
    /// - `working_directory` — 会话绑定工作目录：既是 `AGENTS.md` 的探测根，也原样
    ///   记入 [`InjectableContext::working_directory`]（模型据此确定相对路径基准）。
    ///   `None` 时两者皆空（不探测、不注入环境段）
    async fn load_injectable(
        &self,
        query: &str,
        working_directory: Option<&std::path::Path>,
    ) -> Result<InjectableContext>;

    /// 判定并执行会话压缩，返回带 `compression_marker` 的摘要消息。
    ///
    /// 返回 `None` = 不需要压缩 / 模型空响应丢弃（两种情况调用方都不落库、不收缩段）。
    /// **返回 `Some` 之后必须先落库成功再 `reshape_after_compression`**——顺序由
    /// 调用方（核心）保证，见模块文档。
    ///
    /// - `messages` — 本会话自最后一个压缩点起的对话（已按 ADR-043 规范化工具对）
    /// - `session_id` — 本会话 id（摘要请求的动态头求值输入，ADR-046）
    /// - `recent_input_tokens` — 真实上下文占用（最近一次请求的 prompt 侧 token）
    /// - `force` — 用户手动压缩：跳过窗口阈值
    /// - `existing_summary` — 本会话已有摘要（增量合并；`None` = 全量摘要）
    async fn compress_for_session(
        &self,
        messages: &[Message],
        session_id: &str,
        recent_input_tokens: usize,
        force: bool,
        existing_summary: Option<&str>,
    ) -> Option<StructuredMessage>;

    /// 从压缩点消息提取**纯摘要文本**（剥去展示包装；无有效内容返回 `None`）。
    ///
    /// 与 [`Self::compress_for_session`] 写出的包装格式是一对：两者同源，改格式不会
    /// 让提取静默失效。
    fn extract_marker_summary(&self, marker: &StructuredMessage) -> Option<String>;
}

/// 未装配上下文策略时的内存直通实现（**降级路径，不是功能实现**）。
///
/// 语义：`load_injectable` 返回空可注入上下文（无 soul / 无工作目录（因而不注入
/// 环境段）/ 无 rules / 无 memories）、
/// `compress_for_session` 恒 `None`（不压缩）、`extract_marker_summary` 恒 `None`。
///
/// 为什么是「空直通」而不是 `Err`：`create_app`（构造对象图，47 处 lib 单测 +
/// 嵌入式）不要求装配，`Err` 等于删入口。为什么**不**「原样返回最后一条消息」当压缩
/// 结果：`Some(…)` 会被 `Agent::maybe_compress_and_persist` 当作**真的发生了压缩**
/// ——落一个 `compression_marker` 消息进会话链、再收缩工作集段，等于用最后一条用户
/// 消息顶替摘要（历史被静默丢弃）。降级只能「什么都不做」。
///
/// 代价必须可见（ADR-038）：`AgentBuilder::build` 在拿到本实现时 `warn!` 一次
/// （症状是「模型没有 system 前缀 + 上下文永不压缩」，看不出是降级还是没触发）；
/// `start_server*` 另有启动期硬守卫，生产装配必须注入 `ContextFactory`。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullContextPort;

impl NullContextPort {
    /// 构造降级实现（无状态，零成本）。
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ContextPort for NullContextPort {
    async fn load_injectable(
        &self,
        _query: &str,
        _working_directory: Option<&std::path::Path>,
    ) -> Result<InjectableContext> {
        Ok(InjectableContext::new())
    }

    async fn compress_for_session(
        &self,
        _messages: &[Message],
        _session_id: &str,
        _recent_input_tokens: usize,
        _force: bool,
        _existing_summary: Option<&str>,
    ) -> Option<StructuredMessage> {
        None
    }

    fn extract_marker_summary(&self, _marker: &StructuredMessage) -> Option<String> {
        // 本实现从不产生压缩点；即便链上存在旧压缩点（曾装配过真实实现），
        // 降级期的压缩也不会发生，返回 `None` 只是「没有可续用的摘要」。
        None
    }
}
