# ADR-040: 会话排他写事务——Actor-per-session + Unit of Work

**日期**: 2026-09-24
**状态**: 📝 草案（待评审）
**修订**: ADR-013（串行化）· ADR-035 §6（per-session 锁唯一持有者）· ADR-027（时序链截断语义）
**影响范围**: 工作集注册表（`core/src/agent/working_set.rs`）、Agent 协调器
（`core/src/agent/{coordinator,agent_core}.rs`）、会话 API（`server/src/api/sessions/`）、
快照子系统（`core/src/snapshot/`）

---

## 背景（2026-09-24 侦查实证：回退吞消息）

**症状**：用户"停止 → 回退 → 立刻再输入同样内容"时，新消息**偶发被吞**。

| 现状 | 证据（代码） | 后果 |
|------|-------------|------|
| 轮全程持 per-session 锁 | `turn_guard` = `working_sets.lock(session_id)`（`agent_core.rs:169-175`）；`process_message*` 入口即取锁 | 轮内落库受锁保护 |
| 回退的锁**只覆盖前半段** | `rollback_session`（`coordinator.rs:391-460`）持锁做完 ①②（等轮退出 + 取消锚点后任务）后返回；其 doc 自述"**③④ 不在本方法内**" | 锁释放后，回退仍未完成 |
| 截断是**无锁 read-modify-write** | `SessionService::delete_message`（`server/src/api/sessions/services.rs:302-358`）：`get_session`（读全链）→ `save_redo`（**全量遍历工作区**）→ `truncate` → `persist_messages`（整表重写），**全程无锁** | 窗口 = 读链到写链之间（含一次全树哈希，可达秒级） |
| 整表重写会**覆盖**窗口内新增 | `SessionStore::rewrite`：单事务内清 FTS + 清消息 + 逐条重写（`store.rs:283`） | 窗口内落库的新轮消息（含唤醒轮/通知）被**删除**，库为权威 → 不可自愈 |
| 缓存被同一动作替换 | `SessionWorkingSet::rewrite`：替换 `state.structured_messages` 并重置消费水位 | 正在跑的轮后续 `append` 追加到被截断的链上 → 链错乱 |
| 半成品可能 | 截断先落库、快照恢复在后且失败仅 `warn`（`services.rs:352-360`） | "消息回退了、文件没回退"静默发生 |

**机制缺口**：锁的**粒度**（轮）与写入的**事务边界**（回退事务）不匹配，且
"读—改—写"作为三个独立 API 暴露在锁外。

---

## 决策

### 1. 会话单写者：`with_session_exclusive`

在 `WorkingSetRegistry` 上提供唯一的排他事务入口（**复用现有 `locks` 表，不新增第二把锁**）：

```rust
pub async fn with_session_exclusive<T, F, Fut>(
    &self, session_id: &str, f: F,
) -> Result<T>
where F: FnOnce(Arc<SessionWorkingSet>) -> Fut, Fut: Future<Output = Result<T>>
```

语义：进锁 → `ensure`（锁内重读最新链，保证不是陈旧快照）→ `f(&ws)` → 出锁。
**所有 read-modify-write 型操作必须在此闭包内完成**：回退、重做、编辑、
删除消息、压缩、清空。

- Actor 视角：每会话 = 一个串行处理命令的 actor；per-session 锁 = mailbox 的串行化；
- UoW 视角：闭包 = 工作单元边界；提交 = 闭包内的落库 + 缓存同步 + 事件推送。

### 2. 回退内聚为 core 单一事务

core 提供 `rollback_to(session_id, message_id) -> RollbackOutcome`，**一把锁内**顺序：

1. 取消并等待活动轮退出（置 cancel 标志 → 等 turn 释放，复用现有语义）；
2. 取消时序锚点之后的任务（委托 cancel + 命令 kill），等终态；
3. 保存可逆数据（ADR-042 的操作日志条目）；
4. **先恢复工作区**（预校验对象存在 → 写入；快照已有"先校验后动手"语义）；
5. **后截断链**（经工作集 `rewrite`，含缓存同步）；
6. 推送事件。

**顺序反转（文件先、链后）是关键**：把可能失败的一步放在前，失败即整体中止，
"消息回退了、文件没回退"这种半成品在结构上不可能出现。

```rust
pub struct RollbackOutcome {
    pub truncated: usize,             // 被截断消息数
    pub restored_files: usize,        // 恢复/删除的文件数
    pub workdir: Option<String>,      // None = 未绑定工作区（文件未回退，前端须明示）
    pub redo_available: bool,         // 可撤销回退
    pub cancelled_tasks: usize,
}
```

server `delete_message` handler 退化为薄壳（解析 → 调用 → 映射），
删除跨层"契约分工"及其注释。

### 3. 保持有意取舍：不把流式轮塞进 mailbox

流（用户轮/唤醒轮）仍由 `turn_guard` 串行、由 ADR-036 结构化取消管理；
DestructiveOp 与流之间的互斥由 **ADR-041 的租约**承担（门禁式），
不重构 `AgentLoop`。避免牵动 ADR-030/032/036 三条已落地契约。

---

## 替代方案与否决理由

| 方案 | 否决理由 |
|------|---------|
| A. 只在 `delete_message` 前后各加一次锁（补丁） | 锁与事务边界仍靠调用点拼凑，下一条写路径原样复发 |
| B. 引入 Saga / 补偿编排器 | 单进程单存储，无跨服务事务；编排器是 AGENTS.md 禁止的新抽象 |
| C. 库层乐观并发（版本号 CAS 写库）替代锁 | 版本号对"整表重写 vs 追加"无意义（两者都对）；且不能替代锁，只能**补充**前端一致性（见 ADR-041） |
| D. 完全 Actor 化（流也入 mailbox 队列） | 牵动 ADR-030/032/036 三条契约，收益小风险大 |
| E. 把回退改为"异步任务 + 前端轮询" | 不解决竞态（窗口仍在），只把问题藏到异步里 |

---

## 后果

**正面**
- 回退与任何轮**不可能并发**：窗口在类型上写不出（`get` 只能拿到锁内视图）；
- 半成品回退不可能（顺序反转 + 整体中止）；
- 前端首次获得结构化结果（`workdir: None` 可明确提示"未绑定工作区，仅回退了消息"）；
- core/server 的用例职责恢复单一（SRP），删除裂缝注释。

**成本 / 负面**
- `rollback_session` 的职责从 core 扩展到"含数据截断与快照"——core 需持有
  `SnapshotManager` 与工作目录解析（`Agent` 已持有 snapshot_manager；
  `resolve_working_directory` 已在 `agent_core.rs:494`）；
- 事务闭包内 `await` 长操作（快照 IO、任务 cancel）会**延长持锁时间** →
  同会话新轮排队更久。这是**正确性优先**的有意取舍；缓解手段：事务前先完成
  取消/等待（步骤 1–2 的等待本身不需要持锁，可移出锁外先行），把锁只覆盖
  步骤 3–6。**实现时按此优化，但语义上仍视为同一事务**。

---

## 实施（波次 2）与验收判据

1. `WorkingSetRegistry::with_session_exclusive`（含单测：锁内视图为最新、并发排队）；
2. core `rollback_to` 内聚（迁移 `services.rs` 的 save_redo/truncate/restore 语义）；
3. 重做 / 编辑 / 删除消息 / 压缩改走同一入口；
4. 删除 server 侧 `persist_messages`/`persist_meta` 的直写 fallback（与 ADR-039 同步）。

**验收判据（判别力测试）**
- 并发注入：回退与新轮同时发起 → 断言**消息零丢失**、链 seq 连续、缓存与库一致
  （注入旧行为即变红）；
- 半成品防护：快照对象缺失时 → 断言链**未被截断**（全或无）；
- 锁内视图：在 `with_session_exclusive` 外并发 append → 断言事务看到的链包含该 append；
- 现有回退/重做 e2e 与单测全绿。
