# 天演文档地图

> **代码是唯一事实来源**：文档与代码冲突时以代码为准。写代码前先读 [`../AGENTS.md`](../AGENTS.md)。
> 这份文件只回答一件事：**我要做 X，该读哪一份。**

## 按目的找

| 我想… | 读这份 |
|---|---|
| 知道天演是什么、怎么装起来 | [`../README.md`](../README.md)（产品介绍 + 快速开始） |
| 改代码前弄清纪律与禁区 | [`../AGENTS.md`](../AGENTS.md) — **必读**：硬块约束 / 构建命令 / 工作区结构 / 代码规范 / 模块速览 / 常见陷阱 |
| 找某个模块在哪、职责是什么 | [`architecture/module-map.md`](architecture/module-map.md) — **唯一的模块索引**（crate / core 子模块 / server / GUI） |
| 查某个设计为什么这么定 | [`architecture/decisions/`](architecture/decisions/) — 52 份 ADR，索引见 [`decisions/README.md`](architecture/decisions/README.md)；被否的方向见 [`REJECTED.md`](architecture/decisions/REJECTED.md) |
| 理解一条链路的机制（不是决策） | 下面的「专题文档」 |
| 排查故障 / 体检数据 / 出包发布 | [`operations/`](operations/) · [`release/`](release/) |
| 看这个版本改了什么 | [`../CHANGELOG.md`](../CHANGELOG.md) — 权威变更记录（Keep a Changelog） |
| 翻历史（已完成的迁移 / 旧架构快照） | [`archive/`](archive/) — **不作为事实来源** |

## 专题文档（机制级）

| 文档 | 覆盖 |
|---|---|
| [`architecture/context-pipeline.md`](architecture/context-pipeline.md) | 上下文组装顺序（含 AGENTS.md 注入）+ 压缩机制 + 构成量化方法 + 故障模式 |
| [`architecture/event-protocol.md`](architecture/event-protocol.md) | 事件通道模型 + 四类事件字段表 + 订阅 / 快照恢复 + 可靠性分层 |
| [`architecture/model-provider-notes.md`](architecture/model-provider-notes.md) | Provider 矩阵 + reasoning 回传契约 + 思考强度 × 语言实测 + 前缀缓存 |
| [`architecture/task-runtime.md`](architecture/task-runtime.md) | 三类任务 + 并发排队 + TTL + 唤醒语义 + 取消 / 回退 |
| [`architecture/contracts.md`](architecture/contracts.md) | S / A / B 契约分级与跨层接口 |
| [`architecture/microkernel-baseline.md`](architecture/microkernel-baseline.md) | 微内核化基线（内核体量与本轮插件边界） |
| [`architecture/code-intelligence-routes.md`](architecture/code-intelligence-routes.md) | 代码智能路线（符号 / 结构地图 / 为何没上 LSP） |
| [`architecture/principles.md`](architecture/principles.md) | 设计原则（从 AGENTS.md 提取的约束参考） |
| [`architecture/refactoring-practices.md`](architecture/refactoring-practices.md) | 重构实践指南（审查 / 波次 / QA 纪律） |
| [`architecture/tool-catalog.md`](architecture/tool-catalog.md) | 工具目录 —— **自动生成**，改 schema 不改它（`cargo catalog-gen`） |

## 开发与运维

| 文档 | 用途 |
|---|---|
| [`development.md`](development.md) | 构建 / 测试命令、文档固化判据（`.scratch/` 不算固化）、重构纪律 |
| [`api-endpoints.md`](api-endpoints.md) | HTTP 端点速查 |
| [`operations/troubleshooting.md`](operations/troubleshooting.md) | 日志与三类高频故障 + DB 取证 + QA 纪律 |
| [`operations/data-health-check.md`](operations/data-health-check.md) | 表清单 + 巡检 SQL 集 + 解读处置 |
| [`operations/release-msi.md`](operations/release-msi.md) | 版本号落点 + 本地打包 + CI 发布链 + 已知坑 |
| [`operations/ci-pipeline.md`](operations/ci-pipeline.md) | CI 流水线与门禁 |
| [`operations/dependency-upgrade.md`](operations/dependency-upgrade.md) | 依赖升级流程 |
| [`release/known-issues.md`](release/known-issues.md) | 已知问题清单 |
| [`../gui-native/README.md`](../gui-native/README.md) | GPUI 原生客户端的实测数据与踩坑（**不是**产品文档） |

## 归档（历史快照，不作事实来源）

| 文档 | 说明 |
|---|---|
| [`archive/refactor-log.md`](archive/refactor-log.md) | 2026-09 的一次性重构迭代日志（Wave 1a/1b/2 …） |
| [`archive/module-descriptions.md`](archive/module-descriptions.md) | 2026-09 前的模块功能说明（含已退役的 Tauri 章节） |
| [`archive/module-relationships.md`](archive/module-relationships.md) | 2026-09 前的依赖矩阵与调用流程 |
| [`archive/system-architecture.md`](archive/system-architecture.md) | 2026-09 前的系统架构总览与数据流图 |
| [`archive/gui-native-migration-inventory.md`](archive/gui-native-migration-inventory.md) | gui-vite → GPUI 迁移盘点（迁移已完成，`tauri/` 已退役） |
| [`archive/release-notes.md`](archive/release-notes.md) | 历史发布说明（与 CHANGELOG 重叠，以 CHANGELOG 为准） |

## 维护约定

- **ADR 正文不改**：决策变了就在文末加「后续演进」，或新开一份并在旧文标注 superseded。
- **`tool-catalog.md` 不手改**：改工具 schema 后跑 `cargo catalog-gen`，`.\scripts\test.ps1 lint` 有 freshness 门禁。
- **历史文档进 `archive/` 而不是删除**：顶部注明「历史快照 + 被谁取代」，文件名统一 `kebab-case.md`。
- **模块文档只有一份**：`architecture/module-map.md`。新增模块先改它，别另起一份 module-xxx。
- **临时结论不算固化**：`.scratch/` 里的东西随会话丢失，判据见 [`development.md`](development.md)「知识固化判据」。
