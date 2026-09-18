# ADR-013: 统一消息通知与唤醒原语 —— 消息入库 + 唤醒语义

**日期**: 2026-08-09
**状态**: ✅ 已采纳
**影响范围**: 后台任务通知（`agent/background.rs`）、AgentLoop 入口（`agent/`）、会话层（`session/`）、外部事件源（未来 T1）、前端 SSE

---

## 背景

后台任务完成通知的现状是"**静默注入**"：`SessionTaskNotifier` 把完成消息（带 join 信号 `remaining`）作为 System 消息持久化到父会话，主 LLM 下一轮才看到——**但不触发新轮次**，用户必须手动再发一条消息，主 agent 才会汇总结果。调研 oh-my-openagent（opencode 插件）实证了完整模式：

- 任务完成 → 注入 `<system-reminder>` 消息到父会话 transcript
- **`shouldReply = allComplete || isTaskFailure`**：全部完成（或失败）→ 触发父 agent 新一轮生成；否则消息静默入库（`noReply=true` + 内部标记），**运行时强制不触发生成**（不是模型自律沉默）
- 父会话活动时 wake 排队（prompt-async-gate），空回合重试

反例：Codex CLI 的子任务完成通知 `trigger_turn=false` 且无唤醒补偿（issue #15723/#32188）——父 agent"永远不醒"，必须手动继续。**唤醒语义（通知是否携带"需要回复"）是模式成败的关键。**

## 决策

### 1. 统一消息通知机制（所有通知源同一队列）

后台任务完成、审批挂起、外部事件（文件监听/webhook，T1）、主动提醒——统一为消息类型 + 单一队列。**每条消息带唤醒语义**：

| 消息类型 | 默认唤醒语义 | 说明 |
|---------|------------|------|
| 后台任务完成（部分） | 静默注入 | 只入库，不触发轮次 |
| 后台任务完成（全部/失败） | **唤醒** | `remaining == 0 \|\| failure` → 触发主 agent 新一轮 |
| 审批挂起 | 静默注入 + OS 通知 | 等的是用户，不唤醒 agent |
| 外部事件 | 按配置 | 事件源配置是否触发轮次 |
| 主动提醒（T1） | 唤醒 | relevant-now 推送 |

### 2. 唤醒原语（wake）

- **唤醒轮入口**：AgentLoop 增加"注入消息触发轮"入口（复用现有消息处理，无需用户消息）——注入消息后自动启动一轮生成
- **空输出合法**：AgentLoop 允许模型输出空文本作为合法结束（现在循环必须输出文本才结束，需放开；omo 中对应空回合，天演不做 requeue 状态机）
- **串行化**：wake 与用户消息互斥——单会话同一时刻只有一个活动轮；wake 到达时若轮活动，**排队等待当前轮结束再触发**；绝不丢弃（丢弃 = Codex 的"永远不醒"坑）
- **失败降级**：唤醒轮重试 2 次（API 错误/空输出）后放弃——消息已在 transcript，用户下一条消息自然触发（天演简化版，不做 omo 的 requeue/deferral 状态机）

### 3. 前置：任务状态持久化（任务实体化）

wake 正确性依赖 `remaining` 计数，而 `BackgroundTaskManager` 状态目前**纯内存**——进程重启后任务消失、计数归零，唤醒信号失真。前置改造：后台任务状态持久化到 SQLite（registered/running/completed/failed/cancelled + result/error + seq），重启可查询可恢复。这是"任务脱离调用栈成为独立实体"的一步，也是 wake 机制正确性的前提。

> **边界澄清（2026-09-18）**：本前置仅覆盖**委托任务**（`BackgroundTaskManager`）。
> **命令类任务（`CommandManager`）有意不持久化**（进程内保留、重启即清空，属预期行为）
> ——理由见 [ADR-026](026-background-tasks-unified-panel.md) §5「D 边界澄清」。

## 后果

### 正面
- 后台任务全部完成后主 agent **自动**汇总输出（不再等用户手动消息）——"对话时强大" → "任务完成后自主收尾"
- 统一消息类型 + 完整消息日志 = 可观测性基建（所有通知源可审计）
- 外部事件（T1）与主动提醒直接复用同一队列与唤醒语义——事件驱动触发、主动提醒的底层通路
- 规避 Codex 反例（永不醒）与 omo 复杂度（无需 prompt-async-gate/requeue 状态机——单会话单进程比 omo 简单）

### 负面 / 代价
- AgentLoop 增加"注入消息触发轮"入口与空输出结束语义（循环边界处理 + 测试）
- 唤醒轮产生一次自动 LLM 调用（全部完成时一次，可接受；每任务唤醒已否决）
- 前端需处理"自动出现的 assistant 回复"（SSE 已有流式能力，确认无额外 UX 打断）
- 任务持久化 = BackgroundTaskManager 后端改造（内存 → SQLite）

### 边界条件（违反即重新评估）
- 唤醒**绝不打断**活动轮（排队而非抢占）
- 静默注入仍须入库（transcript 即状态，等待语义靠消息积累）
- 审批挂起不唤醒 agent（用户决策通道，OS 通知即可）
- 唤醒失败重试上限 2 次，之后静默（不无限重试）

## 关键文件

- `core/src/agent/background.rs` — SessionTaskNotifier / build_notification_text（join 信号）/ BackgroundTaskManager（持久化改造点）
- `core/src/agent/agent_core.rs`（AgentCoordinator）— 注入消息触发轮入口
- `core/src/agent/loop.rs` — 空输出合法结束语义
- `server/src/api/chat/services.rs` — 通知器装配
- `core/src/session/` — 任务状态持久化存储（SQLite 或 VFS）

---

## 修订记录（0.3.5）

**唤醒轮与用户轮同构：移除空输出豁免**。原实现给唤醒轮加 `allow_empty_answer`
（`with_allow_empty_answer()`）——空响应在唤醒轮直接放过不重试，理由是"模型可能
有意无需回复"。实测发现该豁免是缺陷的根源：失败场景（cmd 退出码 1 + "必须汇报"
指令）下模型返回仅含碎片思考（如 "@if"）的空输出，框架直接放行并把这条垃圾消息
持久化到会话（用户重启后看到"思考过程 @if"）。

按统一循环框架原则（ADR-030：主 agent 与子代理同构），唤醒轮与用户轮同样同构：

- `AgentLoopConfig.allow_empty_answer` 字段与 `with_allow_empty_answer()` 方法移除
- 空响应（无正文无工具调用）统一重试一次（同轮内，turn 不增加）——`process_wake`
  不再调用 `with_allow_empty_answer()`，`run` / `run_stream` 共享同一空响应判定
- 重试仍空才作为合法结束（空输出 = completed，对齐 DSH 无工具调用收尾语义）
- 唤醒指令保持失败/完成区分：失败场景"必须向用户汇报，禁止输出空文本"；
  全部成功且无需输出仍允许空输出（重试给模型第二次机会后仍空）
