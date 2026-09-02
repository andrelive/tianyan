# ADR-026: 后台任务与子智能体统一面板

日期：2026-09-01
状态：已采纳

## 背景

委托任务（`delegate_to_agent`）与终端命令（`execute_command background`）在后端已是统一注册表（`BackgroundTaskManager`，TaskKind = Delegate/Command），但前端展示割裂：委托卡片在聊天流内（ToolCallCard），后台任务在底部停靠条（SessionTasksPanel），子智能体的工作过程完全不可见（`run_delegation_loop` 的 `sub_messages` 在 `spawn_background_delegate` 结束时被丢弃）。用户对"智能体在干什么"缺乏掌控感。

## 问题

1. **展示割裂**：委托与终端两套展示，子智能体过程不可见（无中间输出/工具调用/思考）。
2. **数据不统一**：同步委托（缺省）阻塞在流内，不进注册表——面板数据源不完整。
3. **数据无界**：`BackgroundTaskManager` 内存 HashMap 无逐出；子智能体消息若落库则无限增长。
4. **并发不可配**：委托并发 4（`try_acquire` 立即拒绝，不排队）、终端并发 8，均为硬编码。

## 决策

### 1. 委托只支持异步

`delegate_to_agent` 删除同步分支：所有委托一律走 `spawn_background_delegate`（注册 → 并发许可 → spawn → 立即返回 task_id）。同步预算、超时提升、`promoted_to_background` 逻辑删除。与 ADR-013 唤醒语义完全对齐（完成 → 通知注入 → 唤醒轮汇总）。

### 2. 子智能体 = 带父会话引用的会话

子智能体消息流直接走会话存储（ADR-018），不另起 transcript 机制：

- `SessionHeader` 加 `parent_session_id` / `kind`（"delegate"）/ `role` 字段（Option，向后兼容）；`session_meta` 加独立列 `parent_session_id`（查询/过滤/级联直接走 SQL）。
- `run_delegation_loop` 每轮消息 `SessionStore.append_message(bt_xxx, msg, index_fts=false)`。**"一次性干净上下文"语义不变**：上下文组装仍从内存 sub_messages 构建，持久化只是记录。
- 会话 id = 任务 id（bt_xxx），任务状态机（`background_tasks`）与消息流（`session_messages`）通过 task_id 关联。

### 3. 流式推送与渲染复用

- 新增聚合 SSE 端点 `GET /tasks/stream`：事件复用 `ChatStreamEvent` 结构，加 `task_id` 字段归集。
- 前端把 `MessageBubble` 的段渲染抽成共享组件，主对话流与子智能体流共用——视觉完全一致，工具结果完整不截断。

### 4. 右侧任务面板（会话级）

右侧竖排面板（实现为 `w-72`≈288px，可折叠成 40px 竖条），替代底部 `SessionTasksPanel`（ADR-022 的停靠条位置）：活跃任务在上、完成沉底，可展开看过程/结果，可取消。流内 ToolCallCard 与 System 通知消息不动（通知注入是主 LLM 汇总机制，ADR-013）。

### 5. 清理策略

- **A. 级联删除**：主会话删除时连带删除其所有子智能体会话（三表：messages/meta/FTS）。
- **B. 上限保护**：子智能体会话总数超 300 时，删"最不活跃主会话"（`session_meta.updated_at` 最旧）的子会话，循环至 ≤ 300；惰性触发（创建子会话时检查）。
- **C. FTS 不索引子会话**：`append_message` 加 `index_fts` 参数，子会话传 `false`——FTS 不膨胀，`session_recall` 天然搜不到子会话（无"人"提供的信息），查询逻辑零改动。
- **D. 注册表 SQL 权威 + TTL**：`BackgroundTaskManager` 内存只保留未终态任务（Running ≤ 并发 + Pending 排队），终态落 SQLite；逐出 = `DELETE WHERE completed_at < now - 3d`。`CommandManager` 同样（活跃任务保留内存——进程句柄）。
- **D 修订（实现时偏差）**：SQL 写失败**保持告警不阻塞状态机**（非初稿的"上抛"）——状态机是任务生命周期核心，写失败上抛会让 spawn 的异步任务丢失终态处理（complete/fail 直接失败）；桌面场景 SQLite 写失败极罕见，告警 + 终态流转保证任务不悬挂。

### 6. 并发配置（可配置）

| 配置项 | 默认值 | 语义 |
|---|---|---|
| `agent.max_background_concurrency` | 20 | 委托任务同时运行上限 |
| `agent.max_background_queue` | 40 | 委托任务排队上限（超出拒绝） |
| `agent.max_command_concurrency` | 16 | 终端命令同时运行上限 |

委托排队模型：双信号量（`run_sem` 运行许可 + `queue_sem` 排队槽位）——`queue_sem.try_acquire_owned()` 失败即拒绝（排队已满）；`run_sem.acquire_owned().await` 阻塞排队。排队中的任务 = Pending（"等待中"），面板可见。`spawn_background_delegate` 顺序：先拿 queue 槽位 → register（Pending）→ 等 run 许可 → mark_running → spawn。终端命令保持现有排队模型，上限可配置。

## 后果

- 子智能体过程对用户可见（面板展开 = 迷你对话流），掌控感提升。
- 存储/协议/渲染三层复用，无新机制（对比 transcript 方案少一整套）。
- 数据有界：内存（未终态任务）、SQLite（3 天 TTL）、FTS（子会话不索引）、子会话（级联 + 300 上限）。
- 主 agent 对简单短任务也变成"委托→等通知→汇总"两轮，需实测验证不空转。
- SQL 写失败语义从"仅告警"改为上抛，状态机操作可见性提升。
