//! tool-pipeline 插件：工具执行管线的**出口侧实现**（ADR-049 期 3-T5）。
//!
//! 「出口侧」指工具已经执行完、结果即将回到会话历史的那一段。本 crate 装三个东西：
//!
//! 1. **观测监听器** [`ToolObservabilityListener`]（`agent/tool_registry/observability.rs`
//!    迁入）：使用统计 / 结构化 Trace / GEPA 执行记录 / 失败规则学习。它是 post-execute
//!    管线的第一个消费者（A1：原 `execute_single` 尾部硬编码观测逻辑的迁移终点）。
//! 2. **输出落盘监听器** [`ToolOutputSpillListener`]（`spill.rs` + `spill_tests.rs` 迁入，
//!    ADR-047）：超预算（`max_lines` / `max_bytes` 任一超限）的工具结果完整落盘
//!    `{data_dir}/tool_output/`，会话里只保留预览 + 路径 + `read_file` 指路。
//! 3. **工具目录渲染** [`tool_catalog`]（`agent/tool_catalog.rs` 迁入）：把运行时注册的
//!    工具集渲染成 markdown 目录，并提供 `--check` freshness 门禁的比较口径。
//!
//! ## 为什么搬出 core（ADR-049 D7：按「变化原因」切）
//!
//! 这三块都是**工具面的出口策略 / 呈现**（B 级）：记什么统计、Trace span 里放多少参数
//! 摘要（200 字节）、GEPA 记录截断到多少字符、落盘阈值（2000 行 / 50 KiB）、落盘目录、
//! 哪些工具不该落盘（`SKIP_TOOLS` 白名单）、目录里怎么写表头、平台提示词怎么归一化。
//! 换它们的理由与换内核机制毫无关系——但在迁移前，调一句「落盘预览该提示模型怎么读」
//! 要重编 core（42.4K 生产行）+ server（13.4K）。搬走后只重编本 crate + 装配包。
//!
//! ## 为什么契约留 core（**不是**遗漏）
//!
//! | 留 core 的东西 | 位置 | 为什么不能搬 |
//! |---|---|---|
//! | `ToolPostExecuteListener` / `ToolPreExecuteListener` / `ToolGuard` / `PreDecision` | `tianyan::agent::tool_registry::pipeline` | 管线本身是 **K1**（循环骨架的第 4 阶段）：注册表按注册顺序调用监听器、`execute_single` 在守卫之后跑它。契约搬走 ⇒ 内核反向 `use tianyan_tool_pipeline::…`，破 D2-1/D2-3 |
//! | `ToolPipelineWiring` / `ToolPipelineFactory` | 同上 | 装配层（`app` / `tauri` / 集成测试）与内核共用的**接线形状**：装配层填、内核在 `AgentBuilder::build` 里调用工厂、插件据以构造监听器。三方共用 ⇒ 住 core |
//! | `ToolCall` / `TianyanError` / `UsageStats` / `TraceCollector` / `ExecutionLog` / `RuleRecorder` / `ExecutionHistory` | `tianyan::{model,common,observability}` | 内核与多个插件共用的契约与数据层；本 crate 只**用**它们 |
//! | `executor::truncate::truncate_spill_with` | `tianyan::executor` | 截断/落盘的**唯一实现**：`execute_command` 的后台日志落盘（`executor/command.rs`）也用同一份。复制进插件＝两份口径同时漂移 |
//! | `ToolOutputConfig` / `TRACE_PARAMS_MAX_BYTES` | `tianyan::config` / `tianyan::agent` | 前者是 `TianyanConfig` 的字段形状（配置结构留 core，见 T4）；后者是**观测数据体积预算**，由内核在注册表侧声明（本 crate 与内核的 Trace 契约共用同一上限） |
//!
//! ## ★ 顺序约束（ADR-047 原文的语义，一字不改）
//!
//! 「落盘为 post-execute 统一出口（单点），**注册在观测监听器之后——数据层须见原始结果**」。
//!
//! 观测监听器把结果写进 Trace / GEPA 数据层（`ExecutionHistory.result` 是
//! `format!("{v}")` 的原文），落盘监听器是**改写者**（把结果换成 `{spilled, preview,
//! spill_path, …}`）。若落盘排在观测之前，数据层看到的永远是「已落盘对象的预览」——
//! 统计与演化数据静默失真，且没有任何报错。
//!
//! 因此顺序在 [`ToolPipeline::into_listeners`] 里**一行可见**：
//!
//! ```text
//! vec![Arc::new(self.observability), Arc::new(self.spill)]
//! //     ^^^^^^^^^^^^^^^^^^^^^^^^^^^ 观测在前：数据层须见原始结果
//! //                                    ^^^^^^^^^^^^^^^^ 落盘在后：它是改写者
//! ```
//!
//! 判别力由 `spill_tests.rs::test_pipeline_order_observability_sees_original_result`
//! 锁定（行为式：跑完整条管线后看 GEPA 数据层里记的是原文还是预览）。
//!
//! ## 降级语义（未装配插槽时）
//!
//! 装配层不注入 `ToolPipelineFactory` 时：**一个管线监听器都不注册**（内核
//! `ToolRegistry::new` 不再自带任何监听器），于是
//!
//! - 大输出**不再落盘**——只受既有的 `executor::truncate` 截断限制（工具自身的输出上限）；
//! - 使用统计 / Trace 工具 span / GEPA 执行记录 / 失败规则学习**全部停止**；
//! - `AgentBuilder::build` 打一条 `warn!` 指路（ADR-038 可见失败）。
//!
//! **不 panic、不返回 Err**：`create_app`（构造对象图，47 处 lib 单测 + 嵌入式）与
//! 集成测试靠「不要求装配也能起实例」才能跑。这与 ADR-047「落盘失败保持原结果
//! （best effort）」是同一档语义：缺实现时少一层优化，而不是工具调用失败。
//!
//! ## 测试
//!
//! 迁移 14 条（观测 2：T0-9 内存不缓冲 + GEPA 落库；落盘 10：超限改写 / 预算内零成本 /
//! 白名单 / `execute_command` 免落盘 / 失败不改写 / 关闭旁路 / 未配置旁路 / 落盘失败
//! 保原结果 / 顺序锁 / 阈值钳制；目录 2：平台归一化 / 渲染排序），另有 1 条**新增**
//! 接线测试（`test_pipeline_wires_tool_output_budget`：`ToolPipelineWiring.tool_output`
//! 真的落到 spill 监听器上——迁移前该断言寄生在注册表字段上，注册表不再持有监听器后
//! 必须换到管线入口）。core 侧 `--lib` 因此只少 14 条，**零放弃**。
//!
//! **源码纪律**：本 crate 只允许 `tianyan::…` 绝对路径，不得出现 `crate::<core 模块>`
//! （本 crate 没有 core 单测的 `#[path]` 同源包含——出口侧实现不被任何 core 用例需要）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod observability;
pub mod spill;
pub mod tool_catalog;

use std::sync::Arc;

use tianyan::agent::{ToolPipelineFactory, ToolPipelineWiring, ToolPostExecuteListener};

pub use observability::ToolObservabilityListener;
pub use spill::ToolOutputSpillListener;

/// 出口侧管线（**有序**：观测在前、落盘在后）。
///
/// 保留两个**具体类型**而不是直接给 `Vec<Arc<dyn …>>`：装配期与测试需要看接线结果
/// （`spill.snapshot()` 的阈值/目录、`observability.usage_stats_ref()`），把类型擦除
/// 推迟到 [`ToolPipeline::into_listeners`] 这一步。
pub struct ToolPipeline {
    /// 观测监听器（统计 / Trace / GEPA / 规则学习）——**必须**排在落盘之前。
    pub observability: ToolObservabilityListener,
    /// 输出落盘监听器（ADR-047，改写者）——**必须**排在最后。
    pub spill: ToolOutputSpillListener,
}

impl ToolPipeline {
    /// 展开成注册顺序的监听器清单（**顺序即语义**，见 crate 文档与 ADR-047）。
    pub fn into_listeners(self) -> Vec<Arc<dyn ToolPostExecuteListener>> {
        // 观测在前（数据层须见原始结果），落盘在后（它是改写者）——这一行就是契约。
        vec![Arc::new(self.observability), Arc::new(self.spill)]
    }
}

/// 按装配层给的接线构造出口侧管线（**唯一的构造入口**）。
///
/// 未提供的依赖 = 该职责关闭（`usage_stats` 未接 → 不记统计；`tool_output` 未接 →
/// 不落盘……），与迁移前「`with_*` 没调用就不生效」逐项等价。
pub fn pipeline(wiring: &ToolPipelineWiring) -> ToolPipeline {
    let observability = ToolObservabilityListener::default();
    if let Some(stats) = wiring.usage_stats.clone() {
        observability.set_usage_stats(stats);
    }
    if let Some(trace) = wiring.trace_collector.clone() {
        observability.set_trace_collector(trace);
    }
    if let Some(recorder) = wiring.rule_recorder.clone() {
        observability.set_rule_recorder(recorder);
    }
    if let Some(log) = wiring.execution_log.clone() {
        observability.set_execution_log(log);
    }
    // 落盘阈值/目录/开关：未接线时监听器保持 `Default`（`dir = None` → 完全旁路）。
    let spill = ToolOutputSpillListener::default();
    if let Some((dir, config)) = wiring.tool_output.clone() {
        spill.configure(dir, &config);
    }
    ToolPipeline {
        observability,
        spill,
    }
}

/// 插槽工厂（装配层注入 `AssemblyServices::with_tool_pipeline_factory` 的那个）。
///
/// 与 `plugin_factory()` / `gate()` / `context pipeline()` 同款类型擦除：server 库只认
/// [`ToolPipelineFactory`]（`Arc<dyn Fn>`），两个监听器的具体类型只在本 crate 出现。
///
/// **可重复调用**（`Arc<dyn Fn>` 而非 `Box<dyn FnOnce>`）：配置热重载会重建 agent
/// （`AppState::reload_agent`），落盘预算与观测接线必须跟着**新配置**走。
pub fn pipeline_factory() -> ToolPipelineFactory {
    Arc::new(|wiring: &ToolPipelineWiring| pipeline(wiring).into_listeners())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tianyan::config::ToolOutputConfig;

    /// 工厂可重复调用（配置热重载会重建 agent），且每次都给出**两个**监听器。
    #[test]
    fn test_pipeline_factory_is_repeatable_and_ordered() {
        let factory = pipeline_factory();
        let wiring = ToolPipelineWiring::default();
        let first = factory(&wiring);
        let second = factory(&wiring);
        assert_eq!(first.len(), 2, "退出侧管线恒为 [观测, 落盘] 两个监听器");
        assert_eq!(second.len(), 2, "工厂应可重复调用（不是 FnOnce）");
    }

    /// 阈值/目录接线：`wiring.tool_output` 必须真的落到 spill 监听器（阈值下限钳到 1）。
    #[test]
    fn test_pipeline_wires_tool_output_budget() {
        let wiring = ToolPipelineWiring {
            tool_output: Some((
                PathBuf::from("/tmp/tool_output"),
                ToolOutputConfig {
                    enabled: true,
                    max_lines: 7,
                    max_bytes: 42,
                },
            )),
            ..Default::default()
        };
        let wired = pipeline(&wiring);
        let (enabled, dir, max_lines, max_bytes) = wired.spill.snapshot();
        assert!(enabled);
        assert_eq!(dir, Some(PathBuf::from("/tmp/tool_output")));
        assert_eq!(max_lines, 7);
        assert_eq!(max_bytes, 42);
        assert_eq!(wired.into_listeners().len(), 2);

        // 未接线（旧测试路径的默认状态）→ 不落盘
        let unwired = pipeline(&ToolPipelineWiring::default());
        assert_eq!(unwired.spill.snapshot().1, None, "未接线 = 不落盘");
        assert_eq!(
            unwired.into_listeners().len(),
            2,
            "旁路也要在管线里（不改写）"
        );
    }
}
