# ADR-011: 子任务授权边界 —— 子 agent 无交互审批

**日期**: 2026-08-09
**状态**: ✅ 已采纳
**影响范围**: 审批工作流（`executor/approval/`）、工具执行链（`agent/tool_registry/`）、AgentLoop（`agent/loop.rs`）

---

## 背景

`delegate_to_agent` 产生子 agent（前台并行 / 后台 fire-and-forget）后，子 agent 内的危险操作（write_file / execute_command / apply_edit / apply_patch / run_tests / verify_build）会触发审批工作流。审查发现两条缺陷：

1. **后台子 agent 的审批请求无人响应**：wait_for_approval 模式下 `request_approval` 挂起等待人工响应，但后台任务通知只在完成时注入会话——用户不知道有审批请求，任务静默挂起至超时。
2. **审批 session_id 硬编码**：工具层调用 `request_approval("tool-execution", &action)` 传字面量而非真实会话——挂起通知注入到不存在的会话（静默失败），审批请求归属错乱。

## 决策

**原则：子 agent 是主 agent 意图的执行器，任务下发即授权边界。交互只发生在主 agent 与用户之间，子 agent 零交互。**

| 场景 | 行为 |
|------|------|
| 主 agent 已确认过的操作（指纹命中） | 子 agent **直接执行**（`confirmed_actions` 指纹共享，共用同一 ApprovalWorkflow） |
| 子 agent 遇到未授权的新危险操作 | **立即拒绝、不挂起**（`request_approval_no_wait`）——即使全局 `wait_for_approval=true`；拒绝原因携带"子任务操作需要主任务授权"标记 → 子 agent 上报 → 主 agent 在主对话向用户确认 → 指纹记录 → 重新委托 |
| 主循环（前台） | 维持现有交互（wait_for_approval 面板 / 询问用户降级链路） |

**实现**：
- `ApprovalWorkflow::request_approval_internal(session_id, action, allow_human_wait)` + 公开入口 `request_approval`（允许等待）/ `request_approval_no_wait`（子任务）
- `execute_parallel` / `execute_single` 增加 `subagent: bool` 参数（主循环 false；`run_delegation_loop` 内 true，嵌套委托恒 true），5 类审批工具按 subagent 分流
- 审批调用传真实 session_id（调用链参数化成果——上一轮 session_id 参数化接续）

**配套（方案 B，主循环 wait_for_approval 通道）**：`ApprovalPendingNotifier` / `SessionApprovalNotifier`——挂起时把审批请求作为 System 消息注入所属会话（前台获得面板指引；后台子 agent 的审批请求可见），用户到审批面板批准/拒绝后经 oneshot 通道恢复。

## 后果

### 正面
- 子 agent 危险操作要么执行（已授权）、要么拒绝上报（未授权）——无挂死、无静默
- 审批决策集中在主 agent 与用户的对话中，符合"任务下发 = 授权边界"的认知模型
- 审批挂起通知修复 session 归属（此前全部错乱）

### 负面 / 代价
- 未授权操作的子任务会失败一次（任务结果携带"需要授权"），由主 agent 重试——多一轮主对话往返
- `subagent` 标志贯穿工具执行链（签名扩展，测试调用点全量更新）

### 边界条件（违反即重新评估）
- 子 agent 上下文**永不**进入 wait_for_human_approval 挂起——若未来引入"子任务挂起等面板"需求，须重新评估
- 指纹共享依赖子 agent 与主循环共用同一 ApprovalWorkflow 实例（ToolRegistry 装配层保证）

## 关键文件

- `core/src/executor/approval/workflow.rs` — request_approval_no_wait / ApprovalPendingNotifier / SessionApprovalNotifier
- `core/src/agent/tool_registry/mod.rs` — execute_parallel / execute_single subagent 参数
- `core/src/agent/tool_registry/{agent_ops,file_ops,code_ops}.rs` — 审批工具分流
- `core/src/agent/loop.rs` — 主循环 subagent=false
