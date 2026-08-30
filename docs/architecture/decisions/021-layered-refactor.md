# ADR-021: 分层重构与循环消除（基础类型层 / 存储层 / 领域层单向依赖）

**日期**: 2026-08-30
**状态**: 已采纳
**影响范围**: 全 core（`core/src/`）、定时任务链（`server/src/scheduled_tasks/`）、组合根（`server/src/lib.rs`）

---

## 背景

1. **模块级循环**：`config ↔ agent`（config/roles.rs 用 `agent::AgentRole`）、`scheduler ↔ agent`
   （scheduler 演化任务用 `agent::role_store`/`agent::roles`；agent 用 `scheduler::tasks::RuleRecorder`）、
   `observability → skills`（execution_log 用 `skills::learning::ExecutionHistory`）。
2. **定时任务链循环引用（运行时泄漏）**：agent → ToolRegistry → ScheduleTaskTool → manager → scheduler →
   handler → agent_lock → agent 构成功能环——强引用阻止 AppState drop 时组件释放，SQLite 文件锁无法解除
   （数据目录搬迁依赖关停后文件可移动）。
3. **agent 上帝模块**：依赖 14 个模块（工具实现 + 核心运行时混在一起）。

## 决策

1. **拆基础类型层（打破模块环）**：
   - `AgentRole`/`RoleSource`/`RoleStatus`/`RoleUsage`/`DelegationRecord` → `core/roles/`（纯类型，只依赖 common）；
   - `RoleStore` → `core/role_store.rs`（独立存储层，依赖 vfs + roles）；
   - `RuleRecorder` → `core/observability/`（自 scheduler 移出）；
   - `ExecutionHistory`/`ExecutionStep` → `core/observability/execution_history.rs`（独立类型层）。
   - config/scheduler 改引用 `crate::roles`/`crate::role_store`——**config/scheduler 不再依赖 agent**。
2. **定时任务链分层（打破运行时环）**：
   - manager 经 `TaskRegistrar` 接口注册（装配层注入 `SchedulerRegistrar`，不持有 scheduler/task_ctx）；
   - handler 经 `TaskResultSink` 接口回写（`ResultSinkBridge` 实持 Weak<manager>）；
   - `ScheduleTaskTool` 经 channel 解耦（mpsc + oneshot，装配层消费者持 Weak manager）——工具不依赖 manager 类型，
     异步边界切断 agent 功能环（结构上无环，非 Weak 弱化）。
3. **依赖方向单向向下**：基础类型层（common/roles）→ 存储层（db/role_store）→ 领域层（session/vfs/observability）→
   顶层（agent/scheduler）。**生产代码零模块环**（文件级 SCC 检测 = 0）。

## 结果

- **生产零环**：config/scheduler/observability 不再依赖 agent；db 只依赖 common；
- **无泄漏**：迁移验证 Agent drops=1、SqliteDb drops=13（全部释放），旧目录完全清空；
- **不依赖优雅关停特殊处理**：分层是根本解法（shutdown 的 agent 释放补丁已移除）；
- **agent 依赖审计**：memory 仅测试引用（生产无）；其余依赖均为工具实现/装配注入（ADR-003 组件工具化本质，合理）。

## 权衡

- **cfg(test) 测试工具环**（context↔test_utils、test_utils↔vfs）：`test_utils` 是 `#[cfg(test)]` 专用模块，
  环只存在于测试编译（测试 mock 的天然模式），发布编译无环——接受。
- **agent 上帝模块**：工具实现依赖领域模块是 ADR-003 的设计本质（非缺陷），无实质瘦身空间。
