# ADR-030: 统一 Agent 循环框架——主 agent 与子代理同构

**日期**: 2026-09-06
**状态**: 已采纳
**影响范围**: Agent 循环（`core/src/agent/loop.rs`、`agent_core.rs`）、子代理委托
（`core/src/agent/tool_registry/agent_ops.rs`）、会话存储（`core/src/session/`）、
事件推送（`server/src/event_push.rs`）、前端任务面板（`gui-vite/src/components/chat/AgentTasksPanel.tsx`）

---

## 背景

主 AgentLoop（`loop.rs` 的 `run_turns`）与子智能体循环（`agent_ops.rs` 的
`run_delegation_loop`）是**两套循环实现**，但差异盘点后全部是配置，没有结构差异：

| 维度 | 主 agent | 子代理 | 性质 |
|------|---------|--------|------|
| 上下文组装 | 会话历史 + 检索 + 前缀 | 委托指令 + 角色 + sub_messages | 配置 |
| 工具集 | 全局注册表 | delegate_tools（角色过滤） | 配置 |
| 收尾条件 | 无工具调用 | submit_result（历史补丁，见下） | 统一为"无工具调用" |
| 状态管理 | 会话 running | 任务状态机 | 配置（完成回调） |
| 事件目标 | session_id | task_id（子代理 = 会话，task_id 即 session_id） | 配置（sender 注入） |
| 持久化 | add_structured_message（索引 FTS） | append_message_no_fts（不索引） | 配置（FTS 开关） |
| 审批 | wait_for_approval | 无（ADR-011） | 策略配置 |
| ask_user | 有 | 无（delegated_agent_cannot_ask） | 策略配置 |

**submit_result 的历史来源**：子代理曾因**工具未注入**只能输出文本（无工具调用）
被当最终结果而编造报告——"无工具调用即完成"判定本身没问题，是工具缺失导致信号
失真。工具注入正常后（delegate_tools），收尾条件可统一为"无工具调用"（与主 agent
一致，模型有工具要调用时会输出 tool_calls）。

**子智能体无流式**：`run_delegation_loop` 用非流式 `chat_completion`（整条消息），
前端 `AgentTasksPanel` 本地累积事件（`liveEvents`）+ `eventsToBlocks` 转换渲染——
与主会话两套数据接收/渲染。

## 问题

1. 两套循环实现：新场景（新角色/新任务形态）要复制循环逻辑。
2. 子智能体无流式：用户可见输出不是逐 token，前端两套渲染路径。
3. submit_result 是"唯一完成信号"的补丁语义，与主 agent 收尾条件不一致。

## 决策

### 1. 统一循环框架：AgentLoop 核心抽配置点

`run_turns` 的循环骨架（LLM 调用 → 工具执行 → 继续，直到收尾条件）保持不变，
配置点注入：

```
统一循环框架：
  loop {
      response = llm(context, tools)          // 流式或非流式
      if 无工具调用 break                      // 收尾条件统一
      results = execute_tools(response.tool_calls)
      context += results
  }
  完成回调（配置）：主 = 回答完成；子 = 任务完成 + 通知
```

配置点：
- **上下文组装器**：主 = `assemble_context`（会话历史 + 检索 + 前缀）；
  子 = 委托指令 + 角色 system_prompt + sub_messages
- **工具集**：主 = 全局注册表；子 = delegate_tools（角色过滤 + submit_result）
- **持久化策略**：FTS 开关（主索引 / 子不索引，ADR-026）
- **事件目标**：StreamEventSender 注入（子代理的 task_id 即 session_id）
- **完成回调**：主 = 回答完成（会话继续）；子 = 任务完成（状态机 + 通知 + 唤醒轮）
- **策略**：审批（主允许 / 子禁止，ADR-011）、ask_user（主允许 / 子禁止）

### 2. 子智能体 = AgentLoop 实例

`run_delegation_loop` 删除，子代理用 AgentLoop 实例（配置：委托上下文组装器、
delegate_tools、任务完成回调、不索引 FTS、无审批、无 ask_user）。角色解析
（`params.role` → system_prompt/model/max_turns）保留为委托上下文组装的一部分。

### 3. 子智能体流式化

子代理用流式路径（`run_stream`）→ 逐 token 事件（thought/answer/tool_call/
observation，task_id 即 session_id）→ 前端完全复用主会话的 reducer 写 store
（`sessionMessages[task_id]`）。`AgentTasksPanel` 改读 store 渲染，删除
`liveEvents` / `eventsToBlocks` / `/tasks/stream` 连接。

### 4. submit_result 降级为可选结果落盘工具

- 收尾条件统一为"无工具调用"（与主 agent 一致）
- `submit_result` 保留为**可选工具**：子代理把最终结果写入任务存储，工具返回
  条目 ID；子代理最后输出告诉主 agent 结果 ID
- 主 agent 用现有 `task_status` 工具（带 task_id 返回任务快照含结果）查询
- 不调用 submit_result 也不算错：无工具调用即完成，结果 = 模型最后输出

### 5. 子智能体消息落库即广播

`SessionManager` trait 加 `add_structured_message_no_fts`（落库不索引 FTS）：
- `PersistentSessionManager`：`store.append_message_no_fts`
- `BroadcastingSessionManager`：inner 落库 + 广播 message 事件（走订阅过滤，
  ADR-029）——子智能体消息与主会话同构（快照恢复 / 断点对齐自动适用）

## 后果

- **一套循环逻辑**：新场景（新角色/任务形态）只需配置，不复制循环。
- **子智能体流式化**：前端数据接收/渲染完全统一（`eventsToBlocks` 删除）。
- **收尾条件统一**：submit_result 从"唯一完成信号"降级为可选结果落盘工具。
- **主 AgentLoop 重构风险**：核心路径抽配置点，分阶段实施 + 每阶段测试兜底。

## 实施顺序

1. **基础设施**：`SessionManager` trait 加 `add_structured_message_no_fts` +
   `BroadcastingSessionManager` 实现 + `ToolRegistry` 注入 session_manager
2. **AgentLoop 配置化**：`run_turns` 抽配置点（上下文组装器/工具集/持久化策略/
   完成回调/策略），主 agent 用默认配置（行为不变，测试兜底）
3. **子智能体并入**：`run_delegation_loop` 删除，改用 AgentLoop 实例（流式路径）；
   submit_result 降级；任务完成回调接入任务状态机
4. **前端**：`AgentTasksPanel` 改读 store（删 liveEvents/eventsToBlocks//tasks/stream）
5. **测试 + 验证**：loop_tests / agent_ops_tests 重写；端到端（委托任务流式展示、
   结果 ID 查询、任务状态机、唤醒轮）

## 修订（2026-10-05）

### 1. timeout_secs 兜底守卫恢复

ADR-030 实施时 `run_subagent_loop` 重写删除了 `tokio::time::timeout` 包裹，但
`DelegateToAgentParams.timeout_secs` 参数与工具描述仍承诺"显式设置作为兜底守卫"——
参数失效（传了完全无效）。审查发现后恢复：

- 超时优先级：显式参数 > 角色值 > 无（后台任务默认无超时，ADR-026）
- 超时后**置位取消标志**（AgentLoop 轮顶/工具执行边界响应，避免任务悬死），
  返回"委托执行超时"错误（watcher 标 fail + 通知主 agent）
- 新增测试 `test_delegate_timeout_aborts_hung_subagent`（mock 流挂起验证真实超时）

### 2. 子智能体消息广播已被 ADR-031 修订

本 ADR 第 5 节"子智能体消息落库即广播"（`BroadcastingSessionManager` 广播 message
事件）已被 [ADR-031](031-optimistic-render-user-message-id.md) 移除：消息不再广播，
子智能体消息经 chat_stream 事件（session_id=task_id）路由到任务面板活跃归约器，
快照（订阅端点）与断点对齐（fetch）兜底。`add_structured_message_no_fts` 决策
（不索引 FTS）保持不变。
