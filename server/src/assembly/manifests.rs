//! `manifests` 组（ADR-049 期 4b）：三份**插件提供的有序清单**的工厂。
//!
//! 组内三个插槽是同一条纪律的三个实例——**清单是有序数据，顺序即语义**（D6）：
//! 工具注册顺序 = 提示词工具目录顺序（抖动 → 前缀缓存全白）、任务注册顺序 = 调度器
//! 扫描顺序 = 同一轮到期任务的执行顺序、工具管线监听器顺序 = 观测在前 / 落盘在后。
//! 三者的「怎么排」都写在装配层（插件之间零互赖，D2-2），因此新增/调整任何一份清单
//! 时这三个插槽会一起动 —— 同组。

use tracing::warn;

use super::{TaskFactory, TaskRegistration, TaskServices, ToolPipelineFactory, ToolPluginFactory};

/// 三份「有序清单」的工厂（`None` = 该清单未装配，走降级路径）。
///
/// 每个字段的降级语义见对应访问器（`plugin_factory()` / `task_factory()` /
/// `tool_pipeline_factory()`）；未装配时**不会**由本库
/// 兜底造一份清单——那会让「未装配」与「装配了某个实现」在日志与行为上无法区分
/// （ADR-038 可见失败）。
#[derive(Clone, Default)]
pub struct ManifestAssemblies {
    plugin_factory: Option<ToolPluginFactory>,
    task_factory: Option<TaskFactory>,
    tool_pipeline_factory: Option<ToolPipelineFactory>,
}

impl ManifestAssemblies {
    /// 装配插件工具清单工厂（`app` / `tauri` 用）。
    pub fn with_plugin_factory(mut self, factory: ToolPluginFactory) -> Self {
        self.plugin_factory = Some(factory);
        self
    }

    /// 装配定时任务工厂（`app` / `tauri` 用：`tianyan-task-builtin` + `tianyan-task-evolution`）。
    pub fn with_task_factory(mut self, factory: TaskFactory) -> Self {
        self.task_factory = Some(factory);
        self
    }

    /// 装配工具管线工厂（`app` / `tauri` 用：`tianyan-tool-pipeline`）。
    ///
    /// 观测监听器（统计 / Trace / GEPA 执行记录 / 失败规则学习）与超预算输出落盘
    /// （ADR-047）的实现搬出 core 后由本插槽注入——`AgentBuilder::build` 按工厂返回的
    /// **有序**清单注册（观测在前、落盘在后，见
    /// [`ToolPipelineWiring`](crate::assembly::ToolPipelineWiring)）。
    pub fn with_tool_pipeline_factory(mut self, factory: ToolPipelineFactory) -> Self {
        self.tool_pipeline_factory = Some(factory);
        self
    }

    /// 插件工具清单工厂（`None` = 不注入插件工具）。
    pub(crate) fn plugin_factory(&self) -> Option<ToolPluginFactory> {
        self.plugin_factory.clone()
    }

    /// 定时任务工厂（`None` = 不注册任何内置任务，见 [`Self::tasks`] 的降级说明）。
    pub(crate) fn task_factory(&self) -> Option<TaskFactory> {
        self.task_factory.clone()
    }

    /// 工具管线工厂（`None` = 不装配任何 post-execute 监听器）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent` → `AgentBuilder`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**：`AgentBuilder::build` **一个监听器都不注册**——超预算
    /// 工具输出不再落盘（只受既有 `executor::truncate` 截断限制）、使用统计 / Trace 工具
    /// span / GEPA 执行记录 / 失败规则学习全停；`build` 打一条 `warn!` 指路。
    ///
    /// 为什么允许降级而不硬失败：① 缺的是**优化与观测**，不是能力——工具照常执行、
    /// 结果照常返回；② `create_app`（构造对象图，47 处 lib 单测 + 嵌入式）不要求装配，
    /// `Err` 等于删入口；③ 与 ADR-047「落盘失败保持原结果（best effort）」同一档语义。
    /// 可见性由装配期 `warn!` 兜底（ADR-038：静默降级的症状会是「统计面板再也不涨 +
    /// 大输出重新撑爆上下文」，看不出是降级还是没触发）。
    pub(crate) fn tool_pipeline_factory(&self) -> Option<ToolPipelineFactory> {
        self.tool_pipeline_factory.clone()
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
        match self.task_factory() {
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
