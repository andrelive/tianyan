# 架构决策记录（ADR）索引

> 共 52 份。**正文不改**：决策变了在文末加「后续演进」，或新开一份并在旧文标注 superseded。
> 被否决的方向（避免重复讨论）→ [REJECTED.md](REJECTED.md)

## 怎么用

- 改某个模块之前，先看相关 ADR：`rg -l "模块名" docs/architecture/decisions/`
- 想知道「为什么不这么做」，先翻 [REJECTED.md](REJECTED.md)
- 机制级说明（现在怎么运作，不是为什么）→ [`../README.md`](../README.md) 各专题文档

| # | 决策 | 文件 |
|---|---|---|
| 001 | VFS 双层摘要索引 —— 基础机制 | [001-vfs-dual-layer-index.md](001-vfs-dual-layer-index.md) |
| 002 | StructuredMessage —— 核心数据结构 | [002-structured-message.md](002-structured-message.md) |
| 003 | 组件工具化 | [003-component-toolization.md](003-component-toolization.md) |
| 004 | 上下文组装前缀匹配原则 | [004-prefix-match-context-assembly.md](004-prefix-match-context-assembly.md) |
| 005 | SQLite 作为主存储后端 | [005-sqlite-backend.md](005-sqlite-backend.md) |
| 006 | 工作区快照独立存储 —— VFS 例外 | [006-snapshot-storage-exception.md](006-snapshot-storage-exception.md) |
| 007 | Core 依赖环消除与共享基础设施归属 | [007-core-dependency-cycle-removal.md](007-core-dependency-cycle-removal.md) |
| 008 | 快照升级 —— gzip 压缩、GC 与 diff 集成 | [008-snapshot-upgrade.md](008-snapshot-upgrade.md) |
| 009 | 语义化编辑双原语 —— 内容匹配 + unified diff 信封 | [009-semantic-editing.md](009-semantic-editing.md) |
| 010 | 对话多模态链路 —— 图片输入 + MCP 截图落盘 | [010-multimodal-message-chain.md](010-multimodal-message-chain.md) |
| 011 | 子任务授权边界 —— 子 agent 无交互审批 | [011-subagent-approval-boundary.md](011-subagent-approval-boundary.md) |
| 012 | 注入上下文快照持久化 —— 前缀零漂移 + 会话边界/压缩点技能刷新 | [012-injectable-snapshot-persistence.md](012-injectable-snapshot-persistence.md) |
| 013 | 统一消息通知与唤醒原语 —— 消息入库 + 唤醒语义 | [013-unified-message-notification-wake.md](013-unified-message-notification-wake.md) |
| 014 | 错误语义化 —— 构造器/谓词模式（4 变体约束下的结构化分类） | [014-error-classification.md](014-error-classification.md) |
| 015 | 会话工作区绑定 —— 工作区是会话的父级分组 | [015-session-workspace-binding.md](015-session-workspace-binding.md) |
| 016 | 角色化子智能体的自演化分工 | [016-role-specialization-evolution.md](016-role-specialization-evolution.md) |
| 017 | 统一自演化架构（摘要独立 + 每日演化智能体任务 + GEPA 数据层 + FTS5 会话回忆） | [017-unified-self-evolution.md](017-unified-self-evolution.md) |
| 018 | 会话权威存储迁移至 SQLite（VFS 会话例外） | [018-session-authoritative-sqlite.md](018-session-authoritative-sqlite.md) |
| 019 | 服务端权威消息时间线（ChatMessage.segments 统一渲染） | [019-server-authoritative-message-timeline.md](019-server-authoritative-message-timeline.md) |
| 020 | 统一写入门面 Database（单连接 + 业务域 Repository） | [020-database-facade.md](020-database-facade.md) |
| 021 | 分层重构与循环消除（基础类型层 / 存储层 / 领域层单向依赖） | [021-layered-refactor.md](021-layered-refactor.md) |
| 022 | 会话绑定的任务面板与任务语义分层 | [022-session-bound-task-ux.md](022-session-bound-task-ux.md) |
| 023 | 配置目录与数据目录分离 | [023-config-data-dir-separation.md](023-config-data-dir-separation.md) |
| 024 | 调度模型从 cron 改为间隔 + 补跑 | [024-interval-scheduler.md](024-interval-scheduler.md) |
| 025 | 移除检索轨迹功能 | [025-remove-retrieval-traces.md](025-remove-retrieval-traces.md) |
| 026 | 后台任务与子智能体统一面板 | [026-background-tasks-unified-panel.md](026-background-tasks-unified-panel.md) |
| 027 | 会话时序链模型——存储层完整链，组装层压缩点视图 | [027-session-timeline-chain.md](027-session-timeline-chain.md) |
| 028 | 会话消息统一事件推送与任务实时同步 | [028-unified-event-push.md](028-unified-event-push.md) |
| 029 | 事件订阅与快照恢复——单流按需订阅 + resident | [029-event-subscription-snapshot.md](029-event-subscription-snapshot.md) |
| 030 | 统一 Agent 循环框架——主 agent 与子代理同构 | [030-unified-agent-loop.md](030-unified-agent-loop.md) |
| 031 | 乐观渲染 + user_message_id 确认 + 统一流式 | [031-optimistic-render-user-message-id.md](031-optimistic-render-user-message-id.md) |
| 032 | 流式事件转发统一（三处接线收敛单点） | [032-unified-stream-forward.md](032-unified-stream-forward.md) |
| 033 | 安全策略收敛——默认自主（ApprovalMode 三态） | [033-security-policy-convergence.md](033-security-policy-convergence.md) |
| 034 | 记忆生命周期治理（巩固通道 + 消费面分离 + 压缩契约） | [034-memory-lifecycle-governance.md](034-memory-lifecycle-governance.md) |
| 035 | 会话工作集——物化上下文缓存与读写收口 | [035-session-working-set.md](035-session-working-set.md) |
| 036 | 结构性取消——等待盲区收口（drop 即取消） | [036-structural-cancellation.md](036-structural-cancellation.md) |
| 037 | 命令执行底层——shell provider（单一事实源） | [037-shell-provider.md](037-shell-provider.md) |
| 038 | provider wire 方言单点（方言即数据） | [038-provider-wire-dialect.md](038-provider-wire-dialect.md) |
| 039 | 会话写侧收口强制化——写权限面收窄 | [039-session-write-side-enforcement.md](039-session-write-side-enforcement.md) |
| 040 | 会话排他写事务——Actor-per-session + Unit of Work | [040-session-exclusive-write-transaction.md](040-session-exclusive-write-transaction.md) |
| 041 | 控制面命令化与租约——Command + Idempotency + Lease | [041-control-plane-lease-idempotency.md](041-control-plane-lease-idempotency.md) |
| 042 | 会话操作日志——undo/redo 对称 + CoW 写入集 + 变化集边界 | [042-session-operation-log.md](042-session-operation-log.md) |
| 043 | 组装层工具对连续性规范化 | [043-tool-pair-continuity.md](043-tool-pair-continuity.md) |
| 044 | 流式双预算——首 token 预算与块间空闲分离 | [044-stream-dual-budget.md](044-stream-dual-budget.md) |
| 045 | 纯通知轮状态——移除看门狗，快照携带权威状态 | [045-notification-driven-turn-state.md](045-notification-driven-turn-state.md) |
| 046 | Provider 预置表与请求修饰单点（头差异数据化） | [046-provider-preset-and-request-modifiers.md](046-provider-preset-and-request-modifiers.md) |
| 047 | 工具输出统一 spill（超预算落盘 + 预览回会话） | [047-tool-output-spill.md](047-tool-output-spill.md) |
| 048 | 会话内 prune（老工具输出降级） | [048-session-internal-prune.md](048-session-internal-prune.md) |
| 049 | 插件化边界与装配模型（微内核化） | [049-microkernel-plugin-boundaries.md](049-microkernel-plugin-boundaries.md) |
| 050 | 工具结果携带图片（`view_image` 直通） | [050-tool-result-images.md](050-tool-result-images.md) |
| 051 | 桌面端单 server 复用（`desktop/` 启动编排段） | [051-desktop-single-server-reuse.md](051-desktop-single-server-reuse.md) |
| 052 | GUI 自动化端口（`GET /state` + `POST /act`） | [052-gui-automation-port.md](052-gui-automation-port.md) |
