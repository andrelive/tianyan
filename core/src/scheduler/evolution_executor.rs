//! 演化综述执行器契约（ADR-049 期 2 序 5b：从 `tasks/evolution_task.rs` 提到这里）。
//!
//! [`EvolutionReviewExecutor`] 是演化任务的**唯一外部能力**：任务负责采集材料
//! （水位线 / 注册表清单 / 近期会话增量）与记账提交（VFS 写入 + 演化报告），
//! 而「跑一轮主 agent 产出 diff 计划」这件事属于 **server 侧业务实现**
//! （`AgentEvolutionExecutor`：在专用演化会话里委托 `evolution_reviewer` 角色）。
//!
//! ## 为什么契约留 core、实现留 server
//!
//! 演化任务的**实现**已搬出 core（`plugins/task-evolution`）。若 trait 随实现一起
//! 搬进插件，server 的 `AgentEvolutionExecutor` 就必须依赖该插件——那是 ADR-049
//! **D2-1 的反向**（通道层不得依赖插件），也与 `ToolContext` / `TaskHandler` 的
//! 既有分工不一致：**内核定义能力集合、插件消费、装配层构造实现**。
//!
//! 因此这条契约按 D3 留在内核（A 级「任务契约」），实际接线走
//! [`crate::scheduler::TaskServices::evolution_executor`]：server 构造实现 → 放进
//! 能力集合 → 插件用它构造 `EvolutionTask`。插件只认识本 trait，不认识
//! `AgentEvolutionExecutor`，也不认识 `AgentCoordinator`。
//!
//! 提议/提交分离因此仍然**由架构强制**：执行器的输出只能是文本（diff 计划 JSON），
//! 注册表写入只能经任务自身的记账通道完成。

use async_trait::async_trait;

use crate::common::error::Result;

/// 演化综述执行器（server 提供实现：委托 evolution_reviewer 角色 / 唤醒主 agent）。
#[async_trait]
pub trait EvolutionReviewExecutor: Send + Sync {
    /// 执行一次演化综述。
    ///
    /// input 为任务采集的输入包（上次运行时间 + 注册表清单 + 指令）；
    /// 返回智能体产出的 diff 计划（JSON 文本）。
    ///
    /// 返回类型从 `std::result::Result<String, TianyanError>` 改成别名
    /// `Result<String>`（clippy `unused_qualifications`；`Result<T>` 就是
    /// `std::result::Result<T, TianyanError>` 的别名——**同一类型、行为零变化**，
    /// 且与 core 其余代码的写法一致）。
    async fn review(&self, input: &str) -> Result<String>;
}
