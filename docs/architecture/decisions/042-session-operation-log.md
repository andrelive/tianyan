# ADR-042: 会话操作日志——undo/redo 对称 + CoW 写入集 + 变化集边界

**日期**: 2026-09-24
**状态**: 📝 草案（待评审）
**修订**: ADR-006（快照存储例外）· ADR-008（快照升级）· 替代 `snapshot/redo.rs` 的一次性语义
**影响范围**: 快照子系统（`core/src/snapshot/`）、工作集（`core/src/agent/working_set.rs`）、
会话 API（`server/src/api/sessions/`）、工具执行管线（`core/src/agent/tool_registry/pipeline.rs`、
`core/src/executor/`）、前端回退交互

---

## 背景（2026-09-24 侦查实证）

| 现状 | 证据（代码） | 后果 |
|------|-------------|------|
| redo 是**一次性数据** | `save_redo` / `load_redo` / `has_redo`（`core/src/snapshot/redo.rs`），消费即失效 | 只能撤销**一步**；孤儿 cache 需 GC 特例清理（`snapshot/mod.rs` 的 `test_gc_removes_orphaned_redo_cache` 即为此存在） |
| 回退 = 另一套机制 | 回退走 `capture`/`restore`（全树）+ `rewrite`（截断）；撤销回退走 `save_redo`/`load_redo` | 两条路径两份数据 → 语义不可能天然一致（本次不一致症状的来源之一） |
| 快照范围 = **会话工作目录整树** | `capture` 遍历 workdir（`mod.rs:156-196`）；`resolve_working_directory`（`agent_core.rs:494-507`） | 写到目录外（`%TEMP%`、其他盘、用户目录）**不被记录** |
| 无工作区 → **静默**跳过文件回退 | `delete_message`：`let Some(workdir) = ... else { return get_messages() }`（`sessions/services.rs:338-341`） | 用户以为文件也回退了 |
| 排除与上限 | `DEFAULT_EXCLUDED_DIRS = [.git, node_modules, target, snapshots]`（`mod.rs:47`）；`>MAX_FILE_BYTES` 跳过 | 这些范围外改动不记录（合理，但需显式告知） |
| 快照键 = 用户消息 ID | `capture(session_id, anchor_message_id)`，在用户消息落库后调用（`agent_core.rs:830-845`） | 锚点语义正确（"该输入处理前"）——应保留 |
| 回退慢 | `save_redo`/`capture` 每次**全量遍历 + 哈希**工作区 | "几秒"延迟的主要来源 |

---

## 决策

### 1. 会话操作日志（Session Op Log）：一个机制替代两套数据

```rust
struct OpLogEntry {
    op_id: String,
    anchor_message_id: String,          // 回退锚点（= 用户消息 ID，语义不变）
    message_delta: MessageDelta,        // 本操作对链的变更（截断点 / 追加消息）
    file_entries: Vec<FileEntry>,       // 本操作影响的文件
    ts: i64,
}

struct FileEntry {
    path: PathBuf,                      // **绝对路径**（不再相对工作目录）
    before: Option<ObjectHash>,         // None = 本操作新增的文件
    source: EntrySource,                // CoW | CdcBaseline | ReportedOnly
}
```

- **回退 = 反向应用**（截断链 + 按 `file_entries` 还原文件）；
- **撤销回退 = 正向应用**；
- 二者是同一条日志的两个方向 → **不可能不一致**；
- **删除** `save_redo` / `load_redo` / `has_redo` 及其一次性语义与孤儿 cache 的 GC 特例。

### 2. 文件条目的三种来源（附：各层的承诺强度）

| 写入方式 | 位置 | 记录来源 | 回退承诺 |
|---------|------|---------|---------|
| 结构化写工具（文件写/编辑/patch） | **任意路径** | 工具管线 pre-execute 拦截（CoW：写前把原内容存入既有内容寻址对象库） | ✅ 精确还原（含目录外） |
| shell / 外部程序 | 会话工作目录内 | 轮边界变化集探测（Windows USN journal；跨平台回退为 mtime+size 扫描对比） + 基线对象还原 | ✅ 还原（依赖已有基线对象） |
| shell / 外部程序 | **工作目录外** | 变化集探测（尽力） | ⚠️ **仅报告清单**（无基线，不回退） |
| 其他进程 / 系统行为 | 任意 | 不探测 | ⚠️ 不承诺（明示） |

**诚实边界（必须在文档与 UI 中明示）**：无沙箱的本地 agent **无法**保证任意路径的
shell 副作用可还原。本 ADR 的承诺是"**可还原的精确还原，不可还原的显式告知**"——
即**消除静默**。彻底的任意路径保证需要沙箱（Windows 受限令牌 / AppContainer /
Job Object 或文件系统级日志如 VSS/ETW），列为**独立安全域议题**（关联 ADR-033），
不在本 ADR 范围。

### 3. 存储位置：复用既有例外区，不新增存储抽象

`{data_dir}/snapshots/{session_id}/oplog/`（ADR-006 例外区内），对象库复用
`snapshots/objects/`（内容寻址，跨会话去重）。**不新增 trait / Manager / 第二套存储**。

### 4. 性能：从"每轮全树捕获"改为"写前按需捕获 + 基线复用"

- 结构化写工具路径：**只在写前捕获被写文件**（O(写入数)，不是 O(树大小)）；
- shell 探测需要基线：保留轮开始捕获，但依赖已有的 `latest.cache.json`
  （mtime+size 复用）使其廉价——**不再重新哈希未变文件**；
- 净效果：回退与轮启动的"几秒"消失（原 `save_redo` 的全树遍历是主因）。

### 5. 无工作区场景语义变更（需产品确认）

原先"未绑定工作区 → 静默不回退文件"。新方案下：

- 若模型使用**结构化写工具**，即使未绑定工作区，文件条目仍可记录并还原
  （路径为绝对路径）；
- 若仅用 shell 且无工作区，则同"目录外"：仅报告。

因此"未绑定工作区"不再是文件回退的**前置条件**，而是"shell 类副作用能否还原"
的边界。前端提示文案需相应调整。

---

## 替代方案与否决理由

| 方案 | 否决理由 |
|------|---------|
| A. 保留 Memento 整树快照（现状） | 范围盲区 + 每轮全树遍历（延迟）+ 与 redo 两套数据不一致 |
| B. 引入 VSS / 文件系统级快照 | 平台耦合、权限要求高、与"任意路径可回退"目标不匹配 |
| C. 直接上沙箱（受限令牌 / AppContainer） | 彻底的方案，但属安全域独立议题（影响工具执行全局）；本 ADR 先消除静默，不阻塞 |
| D. 只记录工具写、不管 shell | 留盲区且**静默**——正是当前被用户质疑的问题 |
| E. 把 op log 落到 VFS | 违反 ADR-006 例外定位与"运行期产物"分类；快照对象库已有归属 |

---

## 后果

**正面**
- 回退与撤销回退**共享一份数据**，一致性由结构保证（不再"两步两套"）；
- 从"锚点单步回退"自然升级为**多级撤销/重做**（日志可回放多个条目）；
- 删除 redo 一次性语义 + 其 GC 特例（概念净减少）；
- 范围盲区从"静默"变为"可还原 / 已报告"两态；
- 性能：轮启动与回退不再全树遍历。

**成本 / 负面**
- 工具管线需接入 CoW 捕获（挂点已有：`tool_registry/pipeline.rs` 的 pre/post 监听器）；
- 变化集探测是平台相关实现（Windows USN 优先，其他平台降级为扫描），
  需明确降级语义与测试策略；
- op log 存储量增加（对象库去重缓解）；需 TTL/GC（复用快照 GC 通道）；
- **语义变更**：无工作区也能回退结构化写入的文件（需产品确认并更新前端文案）。

---

## 实施（波次 5）与验收判据

1. OpLog 数据模型 + 存储（复用快照例外区）+ GC 接入；
2. 回退 / 撤销回退改为日志的反向 / 正向应用（与 ADR-040 的排他事务合并为同一事务）；
3. 工具管线 CoW 捕获（结构化写工具）；
4. 变化集探测（USN 优先 + 扫描降级）；
5. 删除 `snapshot/redo.rs` 及 `save_redo`/`load_redo`/`has_redo` 与相关 GC 特例。

**验收判据（判别力测试）**
- 目录外文件（结构化写工具写入）→ 回退后被精确还原；
- 工作目录内 shell 副作用 → 回退后被还原；工作目录外 shell 副作用 →
  **回退结果显式列出"未还原项"**（断言返回结构与 UI 提示）；
- 连续回退 N 步 + 连续撤销 N 步 → 断言消息链与文件系统均回到对应状态；
- 性能：回退与轮启动不再出现全树遍历（可断言 `capture` 调用次数 / 遍历文件数）；
- 回退失败（对象缺失）→ 断言链未被截断（与 ADR-040 的"全或无"一致）；
- 旧行为注入（redo 一次性语义）→ 对应测试变红。
