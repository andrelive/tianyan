# 天演重构实践指南

> 2026-08-13 架构深化（死代码清理 / 去重 / 错误语义化 / 组合根收敛）后沉淀。
> 目标：后续任何 agent 在此仓库做架构审查与重构时，**不重复犯已犯过的错**。
> 配套规则：AGENTS.md「协作纪律」节 + ADR-014（错误语义化）。

---

## 1. 审查前置：先读决策记录，再提方向

开始任何架构审查前，**先读**：

1. [`decisions/REJECTED.md`](decisions/REJECTED.md) — 19 项明确否决 + **重新评估触发条件**。凡被否决的方向，除非触发条件满足，不要重新建议。
2. 相关 ADR（尤其 ADR-001 VFS / ADR-007 依赖环 / ADR-013 唤醒 / ADR-014 错误语义化）。
3. `docs/architecture/principles.md` — 项目设计原则（seam 原则、不叠加抽象、共享基础设施归属被依赖方）。

**教训来源**：本次审查初期提出的 `pipeline.run` 删除方向，实际是**有意的延后/保留**（文档注释已声明）。先读 REJECTED 和模块文档可避免浪费一轮探索。（`eval/` 模块例外：2026-08 已实际删除——回答质量离线评测与本地自演化定位不匹配，判断职责并入演化智能体。）

> **文档漂移是常态**：`docs/` 声称 LocalFileBackend "已删除"，实际它是生产默认后端（ADR-005 部分落地）。**一切以代码为准**（AGENTS.md 开头规则），发现漂移顺手在文档加注或修正。

## 2. 波次执行法（本次验证有效的流程）

```
探索（并行 explore agents + 自己读文档，不重叠）
  → plan agent 产出决策完备的任务图（波次 + 并行组 + 依赖 + 验证命令）
  → 每波并行执行 → 波末全量 gate（fmt + clippy -D warnings + core/server 测试）
  → 最终：全套件 + 独立审查（Oracle）+ 手工 QA
```

规则：

- **每波 gate 全绿才进下一波**——波内文件域不相交是并行前提。
- **同一波内不做相互依赖的变更**（例：executor 错误发射与 server 消费必须一次原子落地，否则中间态把 409 降级成 500）。
- **重名符号按符号全库 `rg`，不要按文件删**。本次 `search_by_visual` 同时存在于 `DualLayerRetriever` 与 `VfsSearch` trait——只删了前者，trait 上的副本漏网直到 Oracle 复审才发现。
- 删除死代码的验证协议：`rg "<symbol>" --type rust`（含测试）→ 仅剩定义 → 编译通过。编译器是删除操作的最终验证器。
- 保留有意的死代码（`LocalFileBackend`）时，在提交说明/文档注释中写明"有意的"。（`ClipboardWriteTool` 不再属于此类：已接线进组合根 `server/src/state.rs` 的 `dynamic_tools`，`AppState::new` 与 `reload_agent` 双点装配，与 MCP 工具同列注入。）

## 3. 错误分类：只用语义谓词

见 ADR-014。一句话规则：**发射端用构造器，消费端用谓词，禁止字符串匹配**。

- 新增错误类别 → 改 `core/src/common/error.rs` 一处（KIND 常量 + 构造器 + 谓词 + 碰撞矩阵测试）。
- server 层映射 HTTP 状态码 → 用 `is_not_found` / `is_conflict` / `is_invalid_input` / `is_permission` / `is_timeout`。
- 曾犯的错：`contains("快照")` 把解析失败也判 404；`contains("块")` 把任意含"块"字错误判 409——**子串匹配过宽会静默改变 HTTP 语义**。

## 4. 组合根收敛原则

- **同进程共享组件只构造一次**：`PersistentSessionManager`、`TraceCollector` 等由 `AppState::new`（组合根）创建，注入 `AgentBuilder`。热重载（`reload_agent`）复用同一实例。
- 验证：`rg "PersistentSessionManager::new" server/src` 计数 ≤ 1。
- 曾犯的错：API 层与 Agent 各建一个 SessionManager 实例 → chat 服务必须"建会话后清空重写"（create-then-wipe），重启后双写混乱。

## 5. 已知问题清单（pre-existing，勿重复排查）

| 问题 | 位置 | 说明 |
|------|------|------|
| `--host/--port` 无效 | `server/main.rs` | 独立启动总是默认 `127.0.0.1:3000`，QA 直接测 3000 |
| bench 参数 bug | `scripts/test.ps1:37` | `-- --verbose` 被 bench harness 拒绝；直接跑 `cargo bench -p tianyan-server` |
| `rusqlite::Error` 泄漏 | `core/src/vfs/backend/sqlite_db.rs:22,45,69` | 公共 API 泄漏外部错误类型（ADR-014 记录为债务，3 个调用点，未修） |
| 文档漂移 | `docs/module-descriptions.md` 等 | 声称 LocalFileBackend 已删除，实际为生产默认（ADR-005 部分落地） |

**已修复（2026-08-13 第二轮架构深化，勿重复排查）**：

| 已修复问题 | 位置 | 修复 |
|------|------|------|
| `move_entry` 向量丢失（移动后条目永久不可向量检索） | `core/src/vfs/vfs_impl.rs` | 克隆源向量点 + 强制目标 `to_point_id()`；含子条目拒绝（`conflict`），traits.rs 文档对齐 |
| 工具错误分类丢失（`tool: 执行失败` 包装 flatten 掉 not_found/conflict） | `core/src/agent/tool_registry/*` | `wrap_tool_error` 助手（mod.rs），22 处替换 |
| 会话错误非语义构造（`会话已存在/未找到` → 500） | `core/src/session/manager.rs` | `conflict()`/`not_found()` 构造器 |
| `add_message` 丢图（to_structured_message 只写 Text） | `core/src/session/manager.rs` | `StructuredMessage::from_message` 全保真 + 构造器收敛 4 站点 + 删 `add_message` trait |
| `ClipboardWriteTool` 未接线（孤儿工具） | `server/src/state.rs` | `new` + `reload_agent` 双点注入 dynamic_tools |
| server 死回退分支（6 个零生产者前缀）+ 死变体 + 13 处 log-and-flatten | `server/src/api/shared/error.rs` + handlers | 谓词统一 + `inspect_err`/`?`；保留 `配置错误：` |

> 新增发现的问题请追加到此表（含 file:line），供后续 agent 跳过重复排查。

## 6. 手工 QA 纪律

- **副作用可回滚是前提**：`PUT /api/v1/config` 会持久化写入 `tianyan.toml`——QA 后必须恢复（git 不追踪该文件，手工改回）。
- 起服务器：`cargo run -p tianyan-server`（默认 3000），测完 `Stop-Process` + 确认端口释放。
- 端点矩阵至少覆盖：health / chat 端到端 / 错误分类（404/400/409/403/504 各一）/ 会话持久化（重启后仍在）/ 热更新 reload。
- QA 临时文件（temp 目录、测试会话）**记入 TODO 并在交付前清理**。

---

**文档版本**: 2026-08-13
**来源**: 2026-08-13 架构深化复盘（W1 死代码清理 / W2 去重 / W3 错误语义化 / W4 组合根收敛 / W6 QA）
