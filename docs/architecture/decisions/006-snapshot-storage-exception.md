# ADR-006: 工作区快照独立存储 —— VFS 例外

**日期**: 2026-08  
**状态**: ✅ 已采纳  
**影响范围**: 存储 — `snapshot` 模块（`core/src/snapshot/`）

---

## 背景

`SnapshotManager`（`core/src/snapshot/mod.rs`）为支持会话回退/撤销回退，在 Agent 处理每条消息之前捕获用户工作区文件的状态：

- 内容寻址对象库 `objects/{sha256前2位}/{sha256}.bin`（同内容只存一份）
- 快照树 `trees/{index}.json` + mtime 缓存 `trees/{index}.cache.json`
- 重做增量 `redo/`

存储位于 `{data_dir}/snapshots/`，全部使用裸 `tokio::fs`，与 VFS 零交互。按 ADR-001 禁止模式清单（"独立存储后端"、"在 VFS 之外引入新的存储抽象"）字面适用，这构成违规；`AGENTS.md` 硬块约束同样未给快照留出例外。

## 决策

**工作区快照保留为独立文件存储，不迁入 VFS，作为 ADR-001 禁止模式的显式例外。**

理由：

1. **内容语义不同**：快照存储的是用户工作区文件的镜像（外部世界），而 VFS 的设计意图是管理天演自身上下文（记忆、知识、技能、规则）。工作区文件本身从来不在 VFS 中——它们由 `execute_command` / `write_file` 工具直接操作。
2. **结构不匹配**：内容寻址对象库 + 树索引 + redo 增量是专门为"版本恢复"设计的结构，与 VFS 的 L0/L1/L2 层级模型无对应关系；强行塞入会破坏摘要语义（快照对象不是可摘要的上下文）。
3. **访问模式不同**：快照按消息边界批量读写大对象，与 VFS 渐进式披露（L0 摘要发现 → L2 按需加载）的访问模式无关；快照不需要向量检索。
4. **迁移无收益**：迁入 VFS 不带来查询、备份或检索收益，反而引入 L2 大内容写入的成本与摘要开销。

## 后果

### 正面
- 保留现有成熟实现，零迁移风险
- 架构审查不再将 `snapshot` 视为待修复债务（有文档裁决）

### 负面 / 代价
- 快照数据不享受 VFS 的统一原子写入与备份语义（VFS write 自带容错不覆盖快照）
- 系统存在第二套持久化抽象，需在代码评审中维持边界

### 边界条件（违反即重新评估）
- 若未来需要"按内容检索快照"或"快照参与统一备份"，应重新评估迁入 `tianyan://snapshot/` 命名空间
- `snapshots/` 目录仅存储工作区快照数据，**不得扩展为通用存储**（禁止写入记忆/知识/技能等上下文内容）
- 快照模块不得访问 VFS 内容，VFS 模块不得依赖 snapshot

## 关键文件

- `core/src/snapshot/mod.rs` — `SnapshotManager`
- `server/src/state.rs:124-132` — 装配点（root = `{data_dir}/snapshots`）
- `core/src/agent/agent_core.rs` — 消息处理前捕获
- `server/src/api/sessions/services.rs` — 回退/重做 API 驱动
