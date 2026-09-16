# ADR-035: 会话工作集——物化上下文缓存与读写收口

**日期**: 2026-09-16
**状态**: ✅ 已采纳（待分三阶段实施）
**影响范围**: 会话内存态（`core/src/agent/session_state.rs`）、会话工作集（新增
`core/src/session/working_set.rs`）、Agent 协调器（`core/src/agent/agent_core.rs`、
`coordinator.rs`）、Agent 循环与通知投递（`core/src/agent/loop.rs`）、会话存储
（`core/src/session/{store,manager}.rs`）、会话 API（`server/src/api/sessions/`）、
事件推送（`server/src/event_push.rs`、`server/src/api/tasks/handlers.rs`）、
前端历史/事件（`gui-vite/src/hooks/use-session-history.ts`、
`hooks/use-unified-events.ts`、`lib/store/chat-slice.ts`）

---

## 术语

- **会话工作集（`SessionWorkingSet`）**：单个会话的**物化上下文缓存**，以
  `session_id` 为键，对外提供统一的读/写方法，内部完成上下文拼装、落库调用与
  事件派发。**非第二权威**——SQLite 会话表仍是唯一权威（ADR-018）。
- **与 ADR-015 的「工作区」无关**：ADR-015 / `SessionHeader.working_directory`
  里的 workspace 指**工作目录**；本 ADR 的 working set 指**会话上下文的工作集**。
  两者术语同名不同物，实现命名一律用 `WorkingSet`，不复用 `Workspace`。

---

## 背景（2026-09-16 侦查实证）

「会话状态」目前是**每轮临时物化**的：四个入口各自全量重建，轮与轮之间不共享、
不增量、不统一收口。

| 现状 | 证据（代码） | 后果 |
|------|-------------|------|
| **每轮入口全量重建** | `process_message` / `process_message_stream` / `process_wake` / `compress_session` / `rollback_session` 各自调 `load_and_build_state(session_id)`（`agent_core.rs:479`），内部 `store.load` 是 `SELECT content_parts ... ORDER BY seq` **全量读 + 逐条反序列化** | 长会话（数千条）每个轮入口都付一次全量成本 |
| **轮内不重读历史** | 上下文只在轮启动时 `assemble_context` 组装一次；turn 之间 `ctx.messages.push` 仅本地追加（`loop.rs` 投递注释亦确认） | 活动轮期间落库的通知模型看不到 → 只能靠唤醒补，且唤醒轮**重读全量历史** |
| **唤醒轮风暴的机制根因** | 每个 `process_wake` 都 `load_and_build_state` + `prepare_wake_context`（读**整段历史**，含已汇报过的通知）；`turn_guard` 按会话排队，主轮期间 7 个 wake 依次执行、每轮重读同一批通知 | 同一批通知被反复汇报（实测脚手架会话 9 轮唤醒，模型自称"已在前两轮逐条汇报"） |
| **投递水位是内存态** | `AgentLoop.notice_delivered: Arc<Mutex<HashMap<String, i64>>>`，初值 `NOTICE_WATERMARK_INIT` | **T1-24**：重启后水位归零，把历史通知全当未投递**重投**（0.4.7 安装后实测） |
| **写入路径分散** | 轮内 `persist_message`、通知异步任务 `add_structured_message`（只写库）、`rewrite_messages`（删/回退）、`update_session_header`（前缀/title）、`delete_session` 各自直连 `SessionManager` | 内存态与库的同步靠**每个调用点自觉**；漏一处即分叉 |
| **消息 seq 非一等公民** | `SessionState.structured_messages: Vec<StructuredMessage>`（**不带 seq**）；`ChatMessage` 亦无 seq（`server/src/api/sessions/types.rs:87`） | 前端乐观渲染只能按**到达顺序** append → 顺序倒置（**U10** 症状三） |
| **前端订阅快照全量推** | `build_snapshot_payload(&request.session_id, session.messages, cursor)`（`api/tasks/handlers.rs:82`）用**完整链** | 长会话打开即推全量；ADR-027 只约束了「存储层不截断」，未约束推送体积 |
| **空闲卸载已有钩子但空实现** | `SessionState.last_activity: Instant` + `cleanup() {}` | 失效机制无落点；内存里的会话态无法回收 |
| **前缀快照已存库并按压缩点刷新** | `SessionHeader.injectable_snapshot` + `ensure_injectable` / `persist_injectable_snapshot` / `invalidate_injectable_snapshot`（ADR-012 / ADR-030） | 机制正确，但读写散在三个私有方法，未经统一入口；压缩后**无条件**重写快照 |

**结论**：「每轮全量重建 + 写入点各自同步 + 推送全量」三者叠加，是唤醒轮风暴、
通知重放、顺序倒置、长会话成本这些**表面症状各异的缺陷的共同机制缺口**——
缺一个会话级的物化缓存与读写收口层。

---

## 决策

### 0. 定位与非目标（先划边界）

**是**：

- 会话物化上下文**缓存**：`session_id → 物化段（前缀 + 压缩点后消息 + 通知水位 + seq）`；
- 会话读写的**统一入口**：所有会话消息/头部的写入、以及在 core 侧读取上下文，
  都经工作集，由它内部完成「落库 → 更新内存段 → 派发内部事件」；
- `SessionState` 生命周期的提升：从「一轮」（每轮重建）升为「一会话」（跨轮复用 + 增量）。

**不是**：

- **不是第二权威**：库仍唯一权威。任何不一致以库为准；工作集崩溃/失效只是缓存丢失，
  从库重建即可（重建点 = 最近压缩点，见 §7）。
- **不是新存储抽象**：不新增存储后端、不新增持久化文件、不新增锁。写入仍走
  `SessionStore`（会话权威存储，ADR-018 的 VFS 例外保持原样）。
- **不改 ADR-027 契约**：存储层永远返回完整链、组装层从压缩点开始——工作集
  **不制造第二个截断点**，它只物化「压缩点 → 现在」这一段（该段正是组装所需），
  压缩点前的历史只经库按需读取（回退/导出/前端上滚）。
- **不冻结工具表**：工具定义每轮现取（ADR-030 后续演进的取舍结论不变）。

> **与 AGENTS.md 硬块约束的关系（「已有链路不叠加抽象」）**：工作集是**收敛**而非
> 叠加——它把当前 4 处 `load_and_build_state`、5 类写入调用点、3 个前缀私有方法、
> 1 处订阅快照组装**归并到同一处**，不引入新 trait / 新存储 / 新锁。判断标准：
> 实施后**调用点数量必须净减少**，凡出现「原有链路 + 工作集链路」并存的过渡态，
> 该过渡态只允许存在于单个阶段内，阶段收口即删除旧路径。

### 1. 结构与生命周期（`SessionWorkingSet` + `WorkingSetRegistry`）

```rust
/// 单会话工作集（物化段 + 统一读写方法）。
pub struct SessionWorkingSet {
    session_id: String,
    header: RwLock<SessionHeader>,        // 前缀快照 + 会话元数据（库的镜像）
    entries: RwLock<Segment>,             // 压缩点起段：Vec<WorkingEntry { seq, msg }>
    notice_cursor: AtomicI64,             // 通知投递水位（持久化来源见 §3.4）
    last_activity: Mutex<Instant>,        // 空闲失效判定（§7）
}

/// 工作集注册表（生命周期 + 会话锁的唯一持有者）。
pub struct WorkingSetRegistry {
    sets: Mutex<HashMap<String, Arc<SessionWorkingSet>>>,
    locks: Mutex<HashMap<String, Arc<TokioMutex<()>>>>,  // 现有 turn_locks 迁入
}
```

- 注册表是**唯一**持有 per-session 锁的地方（§6）；`AgentLoop::turn_guard` 改为
  委托注册表，不新建第二把锁。
- 生命周期：`ensure(session_id)`（命中复用 / 未命中从库重建）、`remove(session_id)`
  （删除会话）、`sweep_idle(ttl)`（空闲卸载）。

### 2. 段边界：只物化「最近压缩点 → 现在」

- `Segment` 的起点 = 最后一个 `compression_marker`；起点 = 末尾时退化为整段
  （无压缩点，与 ADR-027「无压缩点从头」一致）。
- **组装读段**：`snapshot()` 返回 `{ prefix, entries, notice_cursor }`，供
  `ContextAssembler` 直接使用——`assemble_context` 不再自行遍历
  `structured_messages` 全表。
- **段外读取（压缩点之前）**：一律经库按需读，不驻留内存：
  - 回退/删消息需要「压缩点 → 目标 id」区间做校验与回填 → `read_range(from, to)`；
  - 导出（`vfs_read` 兼容层）/前端上滚 → 直接查库（§8）。
- **段自愈**：任何一次段外写（回退到压缩点之前、外部直接改库）后，段按
  「重建点 = 最近压缩点」语义整体重建（简单、正确；不做增量修补）。

### 3. 写入收口四类（全部经工作集，内部先落库后更缓存再派发）

| 类别 | 语义 | 方法 | 内部动作 |
|------|------|------|----------|
| **① 追加** | 轮内消息、工具结果、通知、摘要消息 | `append(msg, { index_fts }) -> seq` | `store.append_message(_no_fts)`（原子取号）→ 段尾追加 `(seq, msg)` → 派发 `MessageAppended{session_id, seq, msg}` |
| **② 整表重写** | 删消息、回退、编辑 | `rewrite(messages) -> ()` | `store.rewrite` → 段整体重建 → 派发 `MessagesRewritten{session_id, last_seq}` |
| **③ 头部更新** | title / ended_at / working_directory / **前缀快照** | `update_header(f) -> ()` | `store.update_header` → `header` 镜像更新 → 派发 `HeaderUpdated`（前缀变化时才派发） |
| **④ 删除会话** | 用户删除（含级联子会话） | `delete() -> ()` | `store.delete`（递归级联）→ 注册表移除自身与全部子会话工作集 |

- **通知写入是 ① 的特例**：异步任务落库通知不再直连 `SessionManager`，经工作集
  `append`（同样原子取号 + 事件）。这样「通知落库」与「工作集知道它」是同一动作。
- **水位持久化（T1-24 根治）**：`notice_cursor` 不再是纯内存态——初值取
  「**进程启动前该会话最后一条 seq**」（boot 前历史视为已读），运行中每次推进
  同步落到 `SessionHeader`（`notice_cursor: Option<i64>`，随 ③ 一起写）。重启后
  水位从库恢复，历史通知**不重放**。
- **收口方式**：`SessionManager` 的写方法**降级为工作集的内部依赖**——agent 层与
  server 层的写调用点全部改走工作集（阶段二完成时，`load_and_build_state` 与
  直接 `add_structured_message` / `rewrite_messages` 调用点清零）。

### 4. 读语义两种 + seq 一等公民

- **快照（轮开始）**：`snapshot()` —— 前缀 + 段 + 水位。轮启动时取一次，轮内沿用。
- **增量（轮边界）**：`delta_since(seq) -> Vec<(i64, StructuredMessage)>` —— 只取
  水位之后的条目。`AgentLoop` 的轮边界通知投递（T1-23）改为用工作集增量，不再
  每次 `store.last_seq` + `load_after`。
- **seq 一等公民**：工作集内部 `WorkingEntry { seq: i64, msg: StructuredMessage }`。
  `StructuredMessage` **不加 seq 字段**（避免改动核心序列化格式与会话 JSON 兼容性）；
  seq 由工作集承载，经 API 边界（`ChatMessage.seq`、事件 payload、分段加载响应）
  显式暴露。前端一切落位**按 seq**，不按到达顺序（根治 U10 症状三）。

### 5. 前缀策略（存库 + 压缩点刷新 + 内容哈希）

- 前缀（soul / rules / memories）**存库**：`SessionHeader.injectable_snapshot`
  （现状已具备，ADR-012 / ADR-030）——工作集只是把它纳入统一读写（经 §3 的 ③）。
- **压缩后更新**：压缩点即前缀失效点（摘要重建了 system 前缀，此刻刷新零额外缓存
  成本）。现状 `invalidate_injectable_snapshot` 清空 → 下一轮重载并固化，语义保持。
- **内容哈希（新增）**：重检索结果与旧快照**内容哈希一致时保留原快照**，不重写、
  不派发 `HeaderUpdated`——避免"压缩后无条件重写"造成无意义的前缀字节漂移
  （前缀一变即打掉 prompt 缓存）。哈希覆盖 `InjectableContext` 全字段的确定性序列化。
- 会话重建（重启/卸载后 `ensure`）从库读快照直接恢复，不重新检索——前缀与重启前
  逐字节一致。

### 6. 并发：单写者 + per-session 锁（不引入第二把锁）

- **单写者语义**：单会话同一时刻只有一个活动轮（现状 `turn_guard`，ADR-013）。
  工作集的一切读改写都在这把锁内（`WorkingSetRegistry::lock(session_id)`），
  锁的**所有权从 `AgentLoop` 迁到注册表**——`turn_guard` 改为
  `registry.lock(session_id)` 的薄包装。
- **禁止两把锁**：不新建「工作集锁」。两把锁必然产生「锁顺序」问题，而工作集
  与轮的临界区本就重合（都是"这个会话现在在干什么"）。
- **锁内不做 LLM 调用**：工作集的锁只在「读段 / 写段」临界区持有；`run_stream`
  期间不持锁（与现状一致——`turn_guard` 是**整个轮**的互斥，工作集方法在轮内
  嵌套取锁时用可重入的设计：由轮持有者把段句柄传入，或方法内部检测已在锁内）。
  实施约束：**工作集方法不自行取轮锁**，由调用方（轮持有者）在锁内调用；
  无锁调用方（如 API 只读查库）走库、不经工作集。这条避免"锁内取锁"死锁。
- **并发写者实测场景**：用户轮 + 通知异步 append + 唤醒轮——三者都经 ①，
  由单写者 + 原子取号保证无丢号无重复。

### 7. 失效：空闲卸载 + 重建点 = 最近压缩点

- `sweep_idle(ttl)`：`last_activity` 超 **30 分钟**（常量
  `WORKING_SET_IDLE_TTL`）→ 从注册表移除（`cleanup()` 由空实现落地为
  「释放段 + 前缀镜像」）。
- **重建点 = 最近压缩点**：`ensure` 未命中时按此语义重建段（压缩点前历史不加载）。
- 卸载是**纯缓存语义**：卸载不写库、不丢数据；再访问重建后行为一致
  （回归清单 §6.7 验证）。

### 8. 前端分段加载（展示直查库，与工作集解耦）

- **前端历史展示直接查库**，不读工作集：工作集只服务 core 侧组装。
- 端点扩展（新增可选参数，向后兼容）：

  ```
  GET /api/v1/sessions/{id}/messages?before_seq={n}&limit={k}
  → { session_id, messages: [{ seq, ... }, ...], next_before_seq, has_more, last_seq }
  ```

  默认（无参）= **最近 k 条**（k 默认 50）；上滚 = 传 `before_seq` 游标。
- **`ChatMessage` 增 `seq` 字段**（`SessionMessagesResponse` / `SessionDetail` 同步）。
- **订阅快照改「最近 N 条 + cursor」**：`build_snapshot_payload` 不再推完整链，
  改为最近 N 条 + `cursor = last_seq`；更早历史由前端上滚经 §8 端点补齐。
  ADR-029 的「快照与实时同一条流」不变——只是快照体积受 N 约束。
- **组装仍取压缩点后全部**（core 侧直接读段，不经前端端点）。

### 9. 推送收口：轮状态与 seq 落位

- **`turn_state` 事件**（U10 根治）：轮开始/结束时发
  `{ type: "turn_state", session_id, state: "running" | "idle", auto: bool }`
  （`auto = true` 表示唤醒轮/子代理轮，非用户轮）。
  - 前端输入框状态由**后端轮状态**驱动，不再只由「用户发消息」置位：
    `auto` 轮 → 输入框保持可输入但发送按钮转为**停止**（可中止唤醒轮）；
    用户轮 → 发送按钮禁用（现状保持）。
  - 前端据 `state` 显示排队提示（`turn_guard` 排队时用户可见"等待前一轮结束"）。
- **按 seq 落位**：所有会话消息事件（append/rewrite/快照）携带 seq，前端 reducer
  按 seq 插入/替换，不按到达顺序 append。乐观渲染（ADR-031）保留：本地乐观条目
  在收到真实 `user_message_id` + seq 后**确认并落位**。
- **层边界不变**：core 只派发**内部事件**（`events` 模块通道）；server 转 SSE
  （`event_push.rs`）。core 不感知 SSE、不直连前端。

---

## 理由

1. **一次收口解决多个表面缺陷**：唤醒轮重复汇报、通知重启重放、顺序倒置、长会话
   每轮全量成本——都是「缺会话级物化 + 缺统一读写入口」的不同投影。分头打补丁
   （T1-23 三处小改、T1-24 水位初值、U10 前端状态）只治症状；工作集是共同机制根因。
2. **库仍是唯一权威**：工作集是缓存/物化视图，回退、撤销、导出、前端展示都以库
   为准，架构上不承担一致性责任——这让它可随时失效重建，风险可控。
3. **与 ADR-027 正交**：ADR-027 管「存储层不截断、组装层从压缩点开始」——工作集
   只是让「组装层起点」在内存里有个稳定物化位置，不改变契约。
4. **前缀稳定性延续 ADR-012 / ADR-030**：前缀存库 + 压缩点刷新 + 内容哈希，把
   「前缀零漂移」从"机制正确"推进到"不做无意义重写"。
5. **前端与后端解耦**：分段加载让前端不依赖工作集（前端只认库 + 事件），工作集的
   失效/重建对前端完全透明——两层的演进不再互相牵制。

---

## 反方观点与回应

- **「工作集是不是第二权威？」** 回应：不是。唯一权威是 SQLite 会话表；工作集无
  独立持久化（只镜像 `SessionHeader` 中已存在的字段），无独立生命周期语义（可随时
  卸载重建）。判定标准：**任何时刻丢弃工作集，系统行为不变**（仅性能下降）——
  这是缓存而非权威的定义，也是回归清单 §6.5/§6.7 的验收项。
- **「为什么不放进 VFS（统一存储抽象）？」** 回应：会话内容不经 VFS 是 ADR-018 的
  明文例外（流式追加 vs 整块文档，结构性不匹配）。且工作集**不存储**，它读的正是
  `SessionStore`——放进 VFS 反而是给缓存造第二份持久化。
- **「为什么水位不直接塞进 StructuredMessage 或用独立表？」** 回应：塞进消息会
  污染消息格式（水位是会话级状态，不是消息）；独立表违背"会话级状态收敛在
  `SessionHeader`"（ADR-018 已将 title/created_at/working_directory 收敛于此）。
  随头部一起写，与 ③ 同路径，零新增结构。
- **「为什么不冻结工具表以进一步省缓存？」** 回应：ADR-030 后续演进已论证——冻结
  需把会话维度穿进请求组装链（`run_turns` → step 闭包 → `prepare_request`），而
  「顺序/内容确定 + 会话级变化检测触发压缩」已等价达成目标。本 ADR 不重开该取舍。
- **「30 分钟空闲会不会太短/太长？」** 回应：这是**纯缓存** TTL——卸载只省内存，
  再访问重建（重建成本 = 压缩点后段的读取）。30 分钟覆盖"用户切走又回来"的常见
  场景，同时避免大量闲置会话常驻。若重建成本实测偏高，调大常量即可，无结构影响。
- **「回退时段的增量修补是不是更省？」** 回应：不做。段的修补需要处理"截断点在
  段内/段外/压缩点之前"的所有组合，而**整体重建**（重建点 = 最近压缩点）只有一条
  路径、一种正确性。回退是低频用户操作（非热路径），简单正确优先。

---

## 与既有 ADR 的关系

| ADR | 关系 |
|-----|------|
| 018（会话权威存储迁 SQLite） | **遵守**。工作集是缓存；写入仍经 `SessionStore`；VFS 例外不变。`notice_cursor` 落在 `SessionHeader`（已收敛的会话级状态）。 |
| 027（存储完整链 / 组装压缩点） | **遵守**。段 = 压缩点起；段外（压缩点前）经库按需读；不在存储层截断。 |
| 012 / 030（前缀快照持久化 + 压缩点刷新） | **继承并收口**。三个前缀私有方法归入工作集；新增内容哈希避免无意义重写。 |
| 013（串行化 + 唤醒） | **修订机制**。单写者保留（锁迁到注册表）；唤醒的条件判定改用工作集 `delta_since`（替代 `store_last_seq` + `load_after` 组合）。 |
| 029 / 031（订阅快照 / 乐观渲染） | **延续**。订阅快照改为最近 N 条 + cursor；乐观渲染保留，落位改按 seq。 |
| 026（子智能体会话） | **扩展**。子会话同样有工作集（独立键）；级联删除（④）时一并移除。 |

---

## 影响文件清单

**core**

- 新增 `core/src/session/working_set.rs` — `SessionWorkingSet` + `WorkingSetRegistry`
  + `WorkingEntry` + `WORKING_SET_IDLE_TTL`
- `core/src/session/types.rs` — `SessionHeader.notice_cursor: Option<i64>`
- `core/src/session/store.rs` — `read_range(from, to)`（段外按需读；复用 seq 索引）
- `core/src/session/manager.rs` — 写方法定位为工作集内部依赖；顶部过时注释修正
  （"含 compression_marker 截断与消息上限兜底"与 ADR-027 矛盾）
- `core/src/agent/session_state.rs` — 承载职责迁入工作集（`cleanup()` 落地为卸载）
- `core/src/agent/agent_core.rs` — `load_and_build_state` / `assemble_context` /
  `ensure_injectable` / `persist_injectable_snapshot` / `invalidate_injectable_snapshot`
  → 经工作集；`update_session_header` 合并入 ③
- `core/src/agent/coordinator.rs` — 五个入口改 `registry.ensure`
- `core/src/agent/loop.rs` — `notice_delivered` / `store_last_seq` /
  `mark_notices_delivered` / `has_pending_notices` → 工作集水位与 `delta_since`
- `core/src/agent/tool_registry/agent_ops.rs` — 子会话工作集创建与级联移除

**server**

- `server/src/api/sessions/types.rs` — `ChatMessage.seq`；`SessionMessagesResponse`
  增 `next_before_seq` / `has_more` / `last_seq`
- `server/src/api/sessions/{handlers,services}.rs` — `before_seq` / `limit` 分段加载
- `server/src/api/tasks/handlers.rs` — 订阅快照最近 N 条 + cursor
- `server/src/event_push.rs` — `turn_state` 事件；消息事件带 seq

**前端**

- `gui-vite/src/hooks/use-session-history.ts` — 分段加载（上滚游标）
- `gui-vite/src/hooks/use-unified-events.ts` — 订阅快照窗口（最近 N 条 + cursor）
- `gui-vite/src/lib/store/chat-slice.ts` — 按 seq 落位；`turn_state` 驱动输入框状态
- `gui-vite/src/components/chat/` — 输入框/停止按钮按 `turn_state`；队列提示

---

## 实施顺序（三阶段，每阶段可独立发布）

**阶段一 · 读侧缓存（不改变对外行为）**

1. `SessionWorkingSet` + `WorkingSetRegistry`（`ensure` / `remove` / `sweep_idle`）；
   `turn_locks` 迁入注册表。
2. 五个入口改 `registry.ensure`；`assemble_context` 改读段。
3. seq 一等公民（`WorkingEntry`）；`delta_since` 接入轮边界投递。
4. `notice_cursor` 持久化（T1-24 根治）+ 空闲卸载落地。
   **验收**：连续轮/唤醒轮不再全量重建；重启不重放历史通知；core 全绿。

**阶段二 · 写侧收口**

5. 四类写入全部经工作集；agent 层/server 层直连 `SessionManager` 写方法清零。
6. 分段加载端点 + `ChatMessage.seq` + 前端上滚。
   **验收**：写调用点净减少；分段窗口拼接 == 库全量。

**阶段三 · 推送收口**

7. `turn_state` 事件（含 `auto`）；前端输入框/停止按钮/排队提示。
8. 事件按 seq 落位；订阅快照改最近 N 条 + cursor。
   **验收**：唤醒轮可中止；消息顺序与库一致；打开长会话不推全量。

---

## 一致性回归清单（每条 = 一个测试，判别力要求"先红后绿"）

1. **回退（rollback）**：锚点后截断 → 段与库一致，前缀不变，段外（压缩点前）可读。
2. **删消息（delete_message）**：单条删除 → `rewrite` → 段重建一致；回退后工作集
   不残留已删条目。
3. **压缩前移**：段起点前移到新压缩点；前缀刷新；**内容哈希不变时不重写快照**
   （判别力：篡改为"无条件重写"必红）。
4. **并发写**：用户轮 + 通知异步 append + 唤醒轮 → 单写者，seq 无丢号、无重复。
5. **重启重建**：重启后 `ensure` 段 = 压缩点后全部；**历史通知不重放**（判别力：
   移除水位持久化必红）；前缀逐字节一致。
6. **子会话**：独立工作集；级联删除时工作集一并移除（含孙会话）。
7. **空闲卸载**：TTL 后卸载，再访问重建一致（丢弃工作集行为不变）。
8. **分段加载**：上滚游标拼出的完整窗口 == 库全量（无缺、无重、顺序正确）。
9. **快照恢复**：订阅快照（最近 N 条 + cursor）+ 后续事件按 seq 落位，最终视图
   == 库全量（判别力：改为按到达顺序 append 必红）。
10. **轮状态**：唤醒轮运行中前端可停止；用户轮运行中输入禁用（判别力：移除
    `turn_state` 推送必红）。

---

## 与已知缺陷（T1-24 / U10）的收敛关系

这两个缺陷是工作集缺位的直接症状，**快修先兜、架构根治**：

| 缺陷 | 快修（随 0.4.8） | 根治（本 ADR） |
|------|-----------------|----------------|
| **T1-24** 重启重放历史通知 | 水位初值取「进程启动前最后一条 seq」 | `notice_cursor` 持久化到 `SessionHeader`（§3.4），与工作集生命周期一致 |
| **U10** 停不掉唤醒轮 / 发了没响应 / 顺序乱 | 唤醒轮接取消标志；`turn_state` 事件；消息按服务端 seq 落位 | §9 推送收口（轮状态 + seq 落位）；§6 单写者排队提示 |

快修只做**最小正确性修补**（不加新层）；阶段一/三落地后，快修的实现自然被
工作集版本替换（不保留两套）。

---

## 回滚

回滚 = `git revert` 本 ADR 的代码变更，逐阶段可独立回滚（阶段一→二→三 无反向依赖）。
回滚即回到「每轮全量重建 + 分散写入」，**功能正确性不受影响**（只是唤醒轮风暴
等性能/体验缺陷回归）——这正是"非权威缓存"定位带来的安全回滚性质。
