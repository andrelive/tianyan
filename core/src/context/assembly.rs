//! 上下文策略的装配原语（ADR-049 期 3-T2）。
//!
//! [`ContextServices`] 是**装配期能力集合**：由组合根（`server` 的
//! `AppState::new` / `reload_agent`）填好，交给装配层注入的
//! `ContextFactory`（`server/src/assembly.rs` 第六插槽），工厂据此构造
//! `Arc<dyn ContextPort>`。
//!
//! **为什么定义在 core 而不是 `server/src/assembly.rs`**：与 `TaskServices` 同一条
//! 理由（ADR-049 期 2 序 5a）——本类型被**三方**同时引用：server 消费、装配包
//! （`app` / `tauri`）生产工厂、插件（`tianyan-policy-context`）读取字段。若住在
//! server，插件就必须依赖 `tianyan-server`，破 D2-1「插件只依赖 kernel-api」，也与
//! `ToolContext` 住在 core 的先例不一致（内核定义能力集合、插件消费）。server 侧
//! re-export 成插槽契约（`tianyan_server::{ContextServices, ContextFactory}`），对
//! 装配层而言 API 面不变。
//!
//! **字段是能力而非策略**：这里只递「世界」（模型服务 / VFS / 统计 / 配置 / 规格），
//! 不含任何「怎么注入、怎么压缩、检索多少条」——那些是插件的 B 级自由。

use std::sync::Arc;

use crate::config::TianyanConfig;
use crate::model::spec::ModelSpec;
use crate::model::ChatService;
use crate::observability::usage_stats::UsageStats;
use crate::vfs::VirtualFileSystem;

/// 构造上下文端口所需的外部能力集合。
///
/// 每个字段都是「插件自己拿不到、只能由装配层递进来」的东西：
///
/// - [`TianyanConfig`] — 注册期配置快照（`agent.default_top_k` /
///   `agent.learned_rules_top_k` 是管线参数）。整体传 `TianyanConfig` 而不是两个
///   `usize`：与 `TaskServices.config` 同口径（配置一处演进，能力集合不必跟着改签名）；
/// - `vfs` — 语义检索与内容加载的唯一入口（`VirtualFileSystem` 契约，不是具体实现）；
/// - `usage_stats` — 检索写统计（文档命中 / 查询 / 加载），与全系统共享同一实例；
/// - `chat` — 摘要模型客户端（`ModelServices.chat` 的**同一份 Arc**，不重复建
///   HTTP 客户端池）；
/// - `chat_model` — 摘要请求的模型名（`CompressionConfig.summary_model`）；
/// - `chat_spec` — 当前聊天模型的上下文规格（`context_window` 联动它的
///   `context_length`；`None` = 未知规格，插件回退默认窗口）。
#[derive(Clone)]
pub struct ContextServices {
    /// 注册期配置快照（管线参数来源）。
    pub config: TianyanConfig,
    /// 虚拟文件系统（检索 + 内容加载）。
    pub vfs: Arc<dyn VirtualFileSystem>,
    /// 使用统计（检索命中/加载写库）。
    pub usage_stats: Arc<UsageStats>,
    /// 摘要模型客户端（与 AgentLoop 共享同一实例）。
    pub chat: Arc<dyn ChatService>,
    /// 摘要模型名。
    pub chat_model: String,
    /// 聊天模型上下文规格（压缩窗口联动源；`None` 时插件用默认窗口）。
    pub chat_spec: Option<ModelSpec>,
}
