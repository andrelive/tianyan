# ADR-039: 会话写侧收口强制化——写权限面收窄

**日期**: 2026-09-24
**状态**: 📝 草案（待评审）
**修订**: ADR-035 §3（"写侧收口"从**约定**升级为**编译期机制**）
**影响范围**: 会话存储与管理（`core/src/session/{store,manager}.rs`）、工作集
（`core/src/agent/working_set.rs`）、Agent 核心/循环/后台（`core/src/agent/`）、
审批与调度（`core/src/executor/approval/`、`core/src/scheduler/tasks/`）、
会话/对话/事件 API（`server/src/api/`、`server/src/event_push.rs`、
`server/src/scheduled_tasks/`、`server/src/evolution_executor.rs`）

---

## 背景（2026-09-24 侦查实证）

ADR-035 §3 声称"写侧收口：消息全量重写经工作集"，但收口**只靠调用点自觉**——
`SessionManager` trait 的公开面本身就是第二写路径的入口。

| 现状 | 证据（代码） | 后果 |
|------|-------------|------|
| trait 公开暴露破坏性写 | `SessionManager::{update_session, add_structured_message, add_structured_message_no_fts, rewrite_messages, delete_session}`（`core/src/session/manager.rs:43-131`） | 任何持有 `Arc<dyn SessionManager>` 的模块都能绕过工作集写库 |
| 公开面被**复制** | `BroadcastingSessionManager` 原样转发 4 个破坏性写（`server/src/event_push.rs:62/71/104/112`）；`PersistentSessionManager` 亦为公开实现 | 收口点每加一层装饰器就多一份入口 |
| 非测试调用点 **18 处** | core 5：`agent_core.rs:684`（header）、`background.rs:1115`（通知）、`loop.rs:1346`（消息）、`approval/workflow.rs:71`（审批挂起）、`reminder_task.rs:146`（提醒注入）；server 13：`chat/services.rs:348/360/368`、`sessions/services.rs:67/97/129/433`、`event_push.rs:62/71/104/112`、`evolution_executor.rs:85`、`scheduled_tasks/manager.rs:206/211` | 缓存与库的一致性靠每个调用点自觉；漏一处即分叉 |
| 兜底直写恒存 | `loop.rs:1346`、`sessions/services.rs:67/97` 的注释自述"回退路径（未装配工作集/会话未加载）：原 session_manager 直写" | "正常路径走工作集 + 异常路径直写"—第二路径在生产中一直可用 |
| 分叉已被自认 | `scheduled_tasks/manager.rs:211` 注释："库被重建（清空）→ 失效工作集缓存（同一 sid 在 TTL 内重复运行时，旁路清空会留下残旧缓存）"，随后手动 `working_sets.remove(&sid)` | **症状被当成个案手动兜**，而非结构性修复 |
| 命名混淆 | `SessionState::add_structured_message`（内存态，`core/src/agent/session_state.rs`）与 `SessionManager::add_structured_message`（落库）**同名** | 评审/排障时无法从调用点分辨"写内存"还是"写库"——本次侦查即因此多绕一轮 |

**结论**：只要"破坏性写"是 trait 上的公开方法，"唯一写入口"就无法从机制上成立；
且 `fallback 直写` 使第二路径成为常态而非例外。收口必须是**编译期**的。

---

## 决策

### 1. `SessionManager` trait 收窄为「读 + 创建」

保留：`create_session` / `get_session` / `session_exists` / `load_before` /
`list_sessions`（读侧，含分页）。
**移除**：`update_session` / `add_structured_message` / `add_structured_message_no_fts` /
`rewrite_messages` / `delete_session`。

### 2. 破坏性写的唯一入口 = 工作集（写入收口 ①–④）

| 收口 | 现有方法（`SessionWorkingSet`） | 覆盖原调用点 |
|------|------------------------------|-------------|
| ① 追加 | `append(&msg, index_fts)` | `add_structured_message*`（5 处） |
| ② 整表重写 | `rewrite(&messages)` | `rewrite_messages`（4 处） |
| ③ 头部更新 | `update_header(\|h\| ...)` | `update_session`（6 处） |
| ④ 会话删除 | `delete(&self)`（+ 注册表 `remove`） | `delete_session`（2 处） |

**不新增类型 / trait / Manager**（AGENTS.md 硬约束）：工作集注册表已是 per-session
锁的唯一持有者（ADR-035 §6），破坏性写挂它名下即为单写者。

### 3. 轻量写口分层（防止"改标题也要全链事务"的假简洁）

- 轻量：`update_header`（闭包局部字段写，不加载全链）——覆盖标题 / 工作目录 /
  时间戳等头部字段；
- 重量：`rewrite`（全链替换，须在排他事务内，见 ADR-040）；
- 会话级：`delete`。
分层是**必须**的：否则收窄会把简单操作变笨拙（把耦合从一个方向挪到另一个方向）。

### 4. 无 fallback：`ensure` 失败即上抛

删除所有"工作集不可用 → 直写"分支（`loop.rs:1346`、`sessions/services.rs:67/97`
及同类）。`ensure` 失败（未装配 store / 读库失败）→ 返回 `TianyanError`，
调用方在消息中携带模块前缀。**测试桩场景统一注入内存 `SessionStore`**
（`crate::db::Database::open_in_memory` + `SessionStore::new`，工作集测试已有辅助）。

### 5. 编译期强制 + 命名去歧义

- `SessionStore` 的破坏性写（`rewrite` / `delete` / `update_header`）降为
  `pub(in crate::agent::working_set)`（工作集内部专用）——server 是独立 crate，
  **编译不过**即不可绕；
- `SessionState::add_structured_message` → 改名 `push_message`（内存态），
  与落库 API 在名字上彻底分离。

---

## 替代方案与否决理由

| 方案 | 否决理由 |
|------|---------|
| A. 只对 server 收窄（core 保留 trait 面） | core 内部 5 处仍是第二路径，且其中 4 处是"兜底直写" |
| B. 新增 `SessionWriter` trait / Manager 承载写 | 违反 AGENTS.md"不新增抽象层"；工作集本身即写入口，叠加抽象是退化 |
| C. 保留 fallback + 运行期审计告警 | "约定 + 事后检测"弱于编译期；本次事故正是这类强度不足的直接后果 |
| D. 只在文档/注释里强调"必须经工作集" | 已证明无效（ADR-035 §3 就是这么写的） |

---

## 后果

**正面**
- 破坏性写在 server crate 内**不可达**（编译期保证），"第二条写路径"结构上不存在；
- 缓存与库的分叉在结构上不可能 → 可删除 `scheduled_tasks/manager.rs:211` 的手动
  `working_sets.remove()` 兜底、`loop.rs:1346` 的"回退直写"分支及其注释；
- 调用点从 18 处收敛为 1 类入口，评审时只需看一处。

**成本 / 负面**
- 18 处调用点改造 + trait 收窄（编译器全程引导，机械但量大）；
- 测试桩必须显式装配内存 store（否则"无 fallback"会让相关测试红）——
  这是**有意**的：测试应覆盖真实写路径；
- core 内部调用方需持有 `WorkingSetRegistry`（`Agent` 已持有；`ReminderTask`、
  `SessionApprovalNotifier`、后台通知等需注入）——依赖注入面扩大，
  但换来"写必须经入口"。

---

## 实施（波次 1）与验收判据

1. 核对 `update_header` 对 6 处 `update_session` 场景的字段映射（注意 `agent_core.rs:684`
   的"读 header → 闭包改 → 写回"语义要改为"闭包局部写"，避免覆盖并发写入）；
2. 改造 core 5 处 → 3. 改造 server 13 处 → 4. 收窄 trait + 降可见性 →
5. 测试桩迁移（内存 store）→ 6. 删除 fallback 分支与手动兜底。

**验收判据（可核查）**
- `cargo check --workspace` 通过，且 server crate 中不存在对
  `update_session` / `rewrite_messages` / `delete_session` / `add_structured_message*`
  的引用（可作为 CI 门禁断言）；
- `.\scripts\test.ps1 lint` + 定向单测全绿；
- `scheduled_tasks` 清空路径不再需要 `working_sets.remove()`；
- `SessionState` 不再有 `add_structured_message` 同名单（改名完成）。

---

## 实施记录（2026-09-24 波次 1 落地，commit `e87e60c`）

| 项 | 结果 |
|----|------|
| trait 收窄 | `SessionManager` 移除 5 个破坏性写，只留读 + 创建（`create_session` / `get_session` / `session_exists` / `load_before` / `list_sessions`） |
| 调用点迁移 | core 5 处 + server 13 处全部改走工作集；**`BroadcastingSessionManager` 装饰器整体删除**（写方法移除后退化为空壳，ADR-028 的「落库即推送」改挂工作集 `append` 的边界消息回调） |
| 系统消息落库 | 收敛为单点 `persist_system_message` |
| 兜底删除 | `scheduled_tasks` 的手动 `working_sets.remove()`、各 fallback 直写分支全部删除（未装配 = 装配缺陷 → 显式报错） |
| 测试迁移 | 测试桩改为「真实 store + 工作集」（有意：测试必须覆盖真实写路径） |
| 规模 | 净删除 722 行（+426 / −1148） |

**验收**：`cargo check --workspace` 零警告；core 1415 passed / server 171 passed；
`.\scripts\test.ps1 lint` 全绿。

**收尾项（2026-09-24 完成）**：
- `SessionStore` 的破坏性写方法已降 `pub(crate)`（`append_message` / `append_message_no_fts`
  / `rewrite` / `update_header` / `delete`）——写路径唯一入口 = 工作集，成为**编译期**
  约束（server 测试夹具相应迁移到 `ws.append`）；
- `SessionState::add_structured_message` → `push_message`（`Session` 的同名方法一并改名），
  「内存态 vs 落库」的命名混淆消除——落库方法已不在任何 trait 上。
