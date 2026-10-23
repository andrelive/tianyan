//! `policies` 组（ADR-049 期 4b）：内核能力的**策略实现**插槽。
//!
//! 三个插槽都是「内核留出端口、装配层决定实现」的同一手法（ADR-049 期 3-T1/T2/T3，
//! D5 装配器二分）：审批门控、上下文注入与压缩、任务结果自审。它们的共同点是——
//! **改「策略」时一起动**（换审批口径 / 调压缩窗口 / 换评审提示词都属于同一类改动，
//! 且能力集合都是装配期 `await` 好的句柄与配置快照），因此同组。
//! 没有插槽时的降级都由**消费侧**承担（`AgentBuilder` 的 `None` 分支），本组不兜底
//! 造实现（ADR-038 可见失败）。
//!
//! 曾经的第四根插槽（技能使用复审读端口，期 3-T3）已随旧范式退役（ADR-049 期 4c）：
//! 技能质量判断改由演化智能体依据「技能使用统计 + 用户原话材料 + 技能清单」完成。

use super::{ApprovalGateFactory, ContextFactory, TaskReviewerFactory};

/// 策略实现工厂（`None` = 该策略未装配，走消费侧的降级分支）。
#[derive(Clone, Default)]
pub struct PolicyAssemblies {
    approval_factory: Option<ApprovalGateFactory>,
    context_factory: Option<ContextFactory>,
    task_reviewer_factory: Option<TaskReviewerFactory>,
}

impl PolicyAssemblies {
    /// 装配审批门控工厂（`app` / `tauri` 用：`tianyan-policy-approval`）。
    pub fn with_approval_factory(mut self, factory: ApprovalGateFactory) -> Self {
        self.approval_factory = Some(factory);
        self
    }

    /// 装配上下文策略工厂（`app` / `tauri` 用：`tianyan-policy-context`）。
    pub fn with_context_factory(mut self, factory: ContextFactory) -> Self {
        self.context_factory = Some(factory);
        self
    }

    /// 装配任务结果自审器工厂（`app` / `tauri` 用：`tianyan-policy-review`）。
    pub fn with_task_reviewer_factory(mut self, factory: TaskReviewerFactory) -> Self {
        self.task_reviewer_factory = Some(factory);
        self
    }

    /// 审批门控工厂（`None` = 不装配审批门控）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**（`None` 继续往下传）：不装配审批门控，
    /// `ToolContext::ensure_approved` 走既有的「未装配直接放行」分支，
    /// `AgentBuilder::build` 打一条 `warn!` 指路。这比存储降级（`error!`，
    /// 「重启即丢数据」）轻一个量级：默认配置就是 `ApprovalMode::Autonomous`
    /// （黑名单外全放行，ADR-033），且命令黑名单在 `SecurityPolicy` 守卫层**另有**
    /// 一份拦截；而 `create_app`（构造对象图，47 处 lib 单测/嵌入式）不要求装配，
    /// 硬失败等于删入口。
    pub(crate) fn approval_factory(&self) -> Option<ApprovalGateFactory> {
        self.approval_factory.clone()
    }

    /// 上下文策略工厂（`None` = 不装配上下文策略）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**（`None` 继续往下传）：`AgentBuilder::build` 用 core 的
    /// `NullContextPort` 兜底——`load_injectable` 返回空可注入上下文（模型**没有**
    /// soul / 项目说明 / learned rules / memories 前缀）、`compress_for_session` 恒
    /// `None`（上下文永不压缩），并 `warn!` 一条指路。
    ///
    /// 与审批降级（T1）的**差异**：`start_server*` 入口对上下文策略设了**启动期硬前置**
    /// （[`Self::has_context_factory`]，见 `start_server_inner`）——审批缺失只是「少了
    /// 一层策略复核」，而上下文缺失是「整个 system 前缀没了 + 上下文只增不减」，服务虽然
    /// 起得来但行为已不成立。`create_app`（构造对象图，47 处 lib 单测/嵌入式）仍允许降级。
    pub(crate) fn context_factory(&self) -> Option<ContextFactory> {
        self.context_factory.clone()
    }

    /// 任务结果自审器工厂（`None` = 不装配自审器，任务通知直接投递 + 装配期告警）。
    ///
    /// 与其余插槽的 `pub(crate)` 访问器同处：工厂只由本库的组合根（`AppState::new` /
    /// `reload_agent`）取用——装配层负责**注入**，不负责调用。
    ///
    /// **未装配时的降级语义**：`AgentBuilder` 得到 `None`，`background_self_review`
    /// 为真时不装配自审器（任务完成通知不带 `[自审未通过]` 标记、直接投递），`build`
    /// 打一条 `warn!` 指路。降级代价与 T1 审批同级（少一层质量复核），远轻于存储降级；
    /// 且该开关默认关闭（opt-in，每任务多一次模型调用）。
    pub(crate) fn task_reviewer_factory(&self) -> Option<TaskReviewerFactory> {
        self.task_reviewer_factory.clone()
    }

    /// 是否装配了上下文策略工厂（**启动服务的硬前置**，见 `start_server_inner`）。
    ///
    /// 与 `StorageAssemblies::has_storage_factory` 同一手法：`create_app`（测试/嵌入式
    /// 构造对象图）允许空直通兜底，`start_server*`（真跑服务）必须拒绝——两条路径严格度
    /// 不同，判定点因此必须能被调用方分别选用。
    pub(crate) fn has_context_factory(&self) -> bool {
        self.context_factory.is_some()
    }
}
