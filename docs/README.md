# 天演文档地图

> **代码是唯一事实来源**：文档与代码冲突时以代码为准。写代码前先读 [`../AGENTS.md`](../AGENTS.md)。
> 这份文件只回答一件事：**我要做 X，该读哪一份。**

## 按目的找

| 我想… | 读这份 |
|---|---|
| 知道天演是什么、怎么装起来 | [`../README.md`](../README.md)（产品介绍 + 快速开始） |
| 改代码前弄清纪律与禁区 | [`../AGENTS.md`](../AGENTS.md) — **必读**：硬块约束 / 构建命令 / 工作区结构 / 代码规范 / 设计原则 / 模块速览 / 常见陷阱 |
| 找某个模块在哪、职责是什么 | [`architecture/module-map.md`](architecture/module-map.md) — **唯一的模块索引**（crate / core 子模块 / server / GUI） |
| 查某个设计为什么这么定 | [`architecture/decisions/`](architecture/decisions/) — 54 份 ADR，全量索引见 [`architecture/decisions/README.md`](architecture/decisions/README.md)；被否的方向见 [`architecture/decisions/REJECTED.md`](architecture/decisions/REJECTED.md) |
| 理解一条链路的机制（不是决策） | 下面的「专题文档」 |
| 改契约前先确认级别 | [`architecture/contracts.md`](architecture/contracts.md) — S / A / B 契约分级台账（分级已冻结） |
| 排查故障 / 体检数据 / 出包 / CI / 依赖升级 | [`operations/`](operations/) — 五份运维手册 |
| 知道用户会长期遇到的限制 | [`known-limits.md`](known-limits.md) — 结构性限制（不含版本号、不含临时状态） |
| 查 HTTP 端点 | [`api-endpoints.md`](api-endpoints.md) |
| 看这个版本改了什么 | [`../CHANGELOG.md`](../CHANGELOG.md) — 权威变更记录（Keep a Changelog） |
| 翻历史（已完成的迁移 / 旧架构快照） | [`archive/`](archive/) — **不作为事实来源** |
| 找外部资料整理（非天演机制） | [`reference/`](reference/) |

## 专题文档（机制级）

| 文档 | 覆盖 |
|---|---|
| [`architecture/context-pipeline.md`](architecture/context-pipeline.md) | 上下文组装顺序（含 AGENTS.md 注入）+ 压缩机制 + 构成量化方法 + 故障模式 |
| [`architecture/event-protocol.md`](architecture/event-protocol.md) | 事件通道模型 + 四类事件字段表 + 订阅 / 快照恢复 + 可靠性分层 |
| [`architecture/model-provider-notes.md`](architecture/model-provider-notes.md) | Provider 矩阵 + reasoning 回传契约 + 思考强度 × 语言实测 + 前缀缓存 |
| [`architecture/task-runtime.md`](architecture/task-runtime.md) | 三类任务 + 并发排队 + TTL + 唤醒语义 + 取消 / 回退 |
| [`architecture/contracts.md`](architecture/contracts.md) | S / A / B 契约分级与跨层接口（常驻台账） |
| [`architecture/module-map.md`](architecture/module-map.md) | 模块口径（crate / core 子模块 / server / GUI）——唯一的模块文档 |
| [`architecture/tool-catalog.md`](architecture/tool-catalog.md) | 工具目录 —— **自动生成**，改 schema 后跑 `cargo catalog-gen`，勿手改 |

## 开发与运维

| 文档 | 用途 |
|---|---|
| [`development.md`](development.md) | 构建与检查命令（主 workspace / `gui-native` / `gui-vite`）、知识固化判据（`.scratch/` 不算固化）、重构实践（审查 / 波次 / QA 纪律） |
| [`api-endpoints.md`](api-endpoints.md) | HTTP 端点速查 |
| [`operations/troubleshooting.md`](operations/troubleshooting.md) | 日志与三类高频故障 + DB 取证 + QA 纪律 |
| [`operations/data-health-check.md`](operations/data-health-check.md) | 表清单 + 巡检 SQL 集 + 解读处置 |
| [`operations/release-msi.md`](operations/release-msi.md) | 版本号落点 + 本地打包 + 静默安装 / 同版本升级实测 + CI 发布链 + 已知坑 + 自动更新编排 |
| [`operations/ci-pipeline.md`](operations/ci-pipeline.md) | CI 流水线与门禁 |
| [`operations/dependency-upgrade.md`](operations/dependency-upgrade.md) | 依赖升级流程 |
| [`known-limits.md`](known-limits.md) | 用户可见的长期限制与处置口径 |
| [`../gui-native/README.md`](../gui-native/README.md) | GPUI 原生客户端的实测数据与踩坑（**不是**产品文档） |

## 参考资料

| 文档 | 性质 |
|---|---|
| [`reference/harness-engineering-overview.md`](reference/harness-engineering-overview.md) | 外部资料整理（Harness 工程综述）——**非天演机制**，不描述本仓库实现 |

## 归档（历史快照，不作事实来源）

| 文档 | 说明 |
|---|---|
| [`archive/code-intelligence-routes.md`](archive/code-intelligence-routes.md) | 代码智能路线决策（grep / tree-sitter / LSP 移除 / code graph）——2026-09 决策、已实施，2026-11 归档 |
| [`archive/microkernel-baseline.md`](archive/microkernel-baseline.md) | ADR-049 期 0 基线报告（内核体量与全量重编基线）——期次已全部结项，2026-11 归档 |
| [`archive/gui-native-migration-inventory.md`](archive/gui-native-migration-inventory.md) | gui-vite → GPUI 迁移盘点（迁移已完成；仓库根的 tauri 壳已于 2026-10 删除） |
| [`archive/module-descriptions.md`](archive/module-descriptions.md) | 2026-09 前的模块功能说明（含已退役的 Tauri 章节） |
| [`archive/module-relationships.md`](archive/module-relationships.md) | 2026-09 前的依赖矩阵与调用流程 |
| [`archive/system-architecture.md`](archive/system-architecture.md) | 2026-09 前的系统架构总览与数据流图 |
| [`archive/refactor-log.md`](archive/refactor-log.md) | 2026-08~09 的一次性重构迭代日志（Wave 1a/1b/2 …） |
| [`archive/release-notes.md`](archive/release-notes.md) | 历史发布说明（与 CHANGELOG 重叠，以 CHANGELOG 为准） |

## 维护约定

- **ADR 正文不改**：决策变了就在文末加「后续演进」，或新开一份并在旧文标注 superseded。
- **`tool-catalog.md` 不手改**：改工具 schema 后跑 `cargo catalog-gen`，`.\scripts\test.ps1 lint` 有 freshness 门禁。
- **历史文档进 `archive/` 而不是删除**：顶部注明「历史快照 + 被谁取代」，文件名统一 `kebab-case.md`。
- **模块文档只有一份**：`architecture/module-map.md`。新增模块先改它，别另起一份 module-xxx。
- **临时结论不算固化**：仓库根 `.scratch/` 里的东西随会话丢失，判据见 [`development.md`](development.md)「知识固化判据」。
- **一个事实只有一个写者**：模块职责只在 `module-map.md`；构建与验证命令只在 `development.md`；契约分级只在 `contracts.md`；`AGENTS.md` 只留「不可违反的约束 + 去哪看」，其余用链接。
- **一次性报告结项即归档**：阶段报告、基线测量、路线决策记录在结项后进 `archive/`，顶部标「归档 + 日期 + 被谁取代」，不占 `architecture/` 主位。
