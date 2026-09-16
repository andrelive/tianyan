# ADR-035: 会话工作集——物化上下文缓存与读写收口

**日期**: 2026-09-16
**状态**: ✅ 已采纳（**阶段一已实施** 2026-09-16；阶段二/三待做）
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
| **唤醒轮风暴的机制根因** | （a）`request_wake`（`background.rs:661`）**无按会话去重**——每次调用直接 spawn 一个任务；（b）`AgentWakeForwarder` 的入口水位判定只在 **spawn 前**跑一次，排队中的任务执行时不再重判；（c）每个 `process_wake` 都 `load_and_build_state` + `prepare_wake_context`（读**整段历史**，含已汇报过的通知） | 7 条通知 = 7 个任务全部入队 → 依次重读同一批通知 → 反复汇报（实测脚手架会话 9 轮唤醒，模型自称"已在前两轮逐条汇报"） |
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

- 会话物化上下文**缓存**：`session_id → 物化段（前缀 + 压缩点后消息 + seq）`；
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
> 叠加——它把当前 5 处 `load_and_build_state`、5 类写入调用点、3 个前缀私有方法、
> 1 处订阅快照组装**归并于同一处**，并**删除**通知投递水位三件套（净减少），
> 不引入新 trait / 新存储 / 新锁。判断标准：实施后**调用点数量必须净减少**，凡
> 出现「原有链路 + 工作集链路」并存的过渡态，该过渡态只允许存在于单个阶段内，
> 阶段收口即删除旧路径。

### 1. 结构与生命周期（`SessionWorkingSet` + `WorkingSetRegistry`）

```rust
/// 单会话工作集（物化上下文 + 读取进度 + 活动时间）。
pub struct SessionWorkingSet {
    session_id: String,
    store: Arc<SessionStore>,
    state: Arc<RwLock<SessionState>>,     // 完整链 + 前缀镜像（load_and_build_state 返回源）
    last_seq: AtomicI64,                  // 库尾 seq（-1 = 无消息）
    last_consumed_seq: AtomicI64,         // 最后一次被 loop 消费到的上界（§4）
    last_activity: Mutex<Instant>,        // 空闲失效判定（§7）
}

/// 工作集注册表（生命周期 + 会话锁的唯一持有者）。
pub struct WorkingSetRegistry {
    store: Option<Arc<SessionStore>>,   // None（测试桩）时 ensure 不可用，锁仍可用
    sets: Mutex<HashMap<String, Arc<SessionWorkingSet>>>,
    locks: Mutex<HashMap<String, Arc<TokioMutex<()>>>>,  // 现有 turn_locks 迁入
}
```

- 注册表是**唯一**持有 per-session 锁的地方（§6）；`AgentLoop::turn_guard` 改为
  委托注册表，不新建第二把锁。
- 生命周期：`ensure(session_id)`（命中复用 / 未命中从库重建）、`remove(session_id)`
  （删除会话）、`sweep_idle(ttl)`（空闲卸载）。

### 2. 段边界：只物化「最近压缩点 → 现在」（**实施修订：阶段一保留完整链**）

> **实施修订（2026-09-16）**：本节的「只物化压缩点后」与 ADR-027「内存态不裁剪」
> 之间**不是真冲突，而是捕获端用了隐式表达**：`capture_workspace_snapshot` 用
> `state.structured_messages.len()` 当快照索引，而该值只在「内存态 == 完整链」时
> 才恰好等于「位置」（见 §4 的 seq 语义澄清）。**解耦方式（低成本）**：捕获端改用
> **全链位置**（全链尾 seq + 1）——完整链下与现状行为相同，段化后仍正确。
> **阶段一保留完整链**（与 ADR-027 一致），只引入 seq 语义与跨轮复用；段化其余
> 工作（工作集内部段结构 + 重建从压缩点读 + 其余 `len()` 消费点审查）留待后续批次。
> 下文「段」在阶段一即指完整链，`start_seq = 0`。

- `Segment` 的起点 = 最后一个 `compression_marker`；起点 = 末尾时退化为整段
  （无压缩点，与 ADR-027「无压缩点从头」一致）。
- **组装读段**：`snapshot()` 返回 `{ prefix, entries, last_seq }`，供
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
- **无投递水位（T1-23 的水位机制整体删除）**：seq 进入上下文（§4）后，「读到
  哪了」是**推导值**而非存储值——`ctx.last_seq` 即本轮上下文上界。**通知不投递、
  不记账**；异步写入使工作集段尾前进，轮边界判出增量后**续跑一轮**即可（§4）。
  连带删除：`notice_delivered` HashMap、`NOTICE_WATERMARK_INIT`、
  `has_pending_notices`、`store_last_seq` / `mark_notices_delivered` 配对、
  `AgentWakeForwarder` 的入口水位判定（改为 `ensure_loop_running` 幂等，§4）。
  **T1-24 因此机制性消失**：没有「投递」动作，就没有「重启后把历史通知再追加
  一遍」这条路径——历史通知只在段里出现一次。
- **收口方式**：`SessionManager` 的写方法**降级为工作集的内部依赖**——agent 层与
  server 层的写调用点全部改走工作集（阶段二完成时，`load_and_build_state` 与
  直接 `add_structured_message` / `rewrite_messages` 调用点清零）。

### 4. seq 一等公民 + 续跑判定（取代通知投递水位）

**seq 一等公民**：工作集内部 `WorkingEntry { seq: i64, msg: StructuredMessage }`。
`StructuredMessage` **不加 seq 字段**（避免改动核心序列化格式与会话 JSON 兼容性）；
seq 由工作集承载，经 API 边界（`ChatMessage.seq`、事件 payload、分段加载响应）
显式暴露。前端一切落位**按 seq**，不按到达顺序（根治 U10 症状三）。

> **seq 语义澄清（位置，非身份）**：`seq` 是消息**在完整链上的位置**（0 起连续），
> **不是消息的稳定标识**——身份是 `message_id`。`SessionStore::rewrite`（删消息 /
> 回退 / 编辑）后**被保留消息的 seq 会整体重排**（`enumerate` 重新编号）。由此
> 直接推出两条实现约束：
>
> 1. **快照索引与本 seq 同坐标系**（`trees/{index}.json`，捕获=位置、回退=`position`）。
>    捕获端**不得用内存态条数 `structured_messages.len()` 表达位置**——完整链下
>    二者恒等只是巧合（隐式不变式），段化即失效（编号错位 → 回退找不到或恢复错
>    的旧快照）；应用**全链位置**（全链尾 seq + 1）。
> 2. **前端「按 seq 落位」只适用于追加**：`rewrite` 后 seq 已重排，必须**整体替换
>    窗口**（§3 ② 的 `MessagesRewritten{session_id, last_seq}`），不得按 seq 增量
>    合并（否则旧 seq 的消息会归位到错位置）。

**「读到哪了」= 上下文自身的上界**（不设独立水位）：

```
轮上下文 ctx 携带 last_seq（本轮 ctx 引用过的最大 seq，随每条消息进 ctx 自然推进）
loop {
    resp = llm(ctx, tools)
    if 无工具调用 {
        持会话锁 {                                            // 与 ensure_loop_running 同一临界区
            if ws.last_seq() > ctx.last_seq() {              // 段尾跑到 ctx 前面 → 有新消息
                ctx.append(ws.delta_since(ctx.last_seq()))    // 增量注入（只取新条目）
                ws.last_consumed_seq = ctx.last_seq();
                continue                                      // 再跑一轮
            }
            ws.last_consumed_seq = ctx.last_seq();            // 回写消费上界
            ws.running = false;                               // 同一临界区标记退出
        }
        break                                                 // 收尾
    }
    ... 工具执行 ...
}
```

- **为什么天然只对「新东西」为真**：轮内自己产生的消息（assistant 回复、工具结果）
  既落库又进 ctx，两边同号推进；**只有异步写入**（后台任务通知）会让 `ws.last_seq`
  跑到 `ctx.last_seq` 前面。于是判定为真 ⟺ 有异步新消息。
- **实施约束**：轮内消息必须**先经工作集 `append` 取号 → 再进 ctx**（顺序不可
  颠倒），否则 `ctx.last_seq` 落后于工作集，产生假阳性续跑。轮内消息内容在落库前
  已完整（assistant 在流式结束后 append），约束可满足。
- **读语义**：`snapshot()`（轮开始：前缀 + 段 + `last_seq`）与
  `delta_since(seq)`（轮边界增量）——后者不再以「水位」为界，而以**调用方 ctx 的
  `last_seq`** 为界。
- **唤醒 = `ensure_loop_running(session_id)`（幂等，不是入队）**：唤醒的语义是
  「确保该会话有一个 loop 在跑」，而非「为本条通知排一个轮」。

  ```
  AgentWakeForwarder::wake(session_id):        // 实际实现（实施修订）
      guard = try_lock(session_id)              // 拿不到 → 已有 loop → no-op
      if guard is None { return }
      if ws.has_unconsumed() == false { return } // 无未消费新消息 → 不空转
      spawn { 持 guard 跑完整轮（process_wake_locked） }   // 锁跨越整轮
  ```

  > **实施修订（2026-09-16）**：实现用**锁的两种获取方式**表达幂等（`lock` 排队 =
  > 用户消息；`try_lock` 幂等 = 通知唤醒），不再需要 `running` 字段——「是否已有
  > loop 在跑」由锁自身表达（且持锁跨越整轮，天然与收尾判定同一临界区）。

  - **N 条通知 → 1 个 loop**：第一条拿到启动权，后续的看到 `running` → no-op——
    不需要队列、不需要去重集合（「是否已有 loop」本身就是那个状态）。
  - **后来者不丢**：它们的通知在段里，running loop 的轮边界续跑判定（§上文）会
    读到并消化。
  - **唤醒轮不补唤醒**：其收尾的续跑判定自然覆盖「期间又有新通知」。
- **串行化点：`running` 检查 / `last_consumed_seq` 更新 / loop 退出判定必须同一
  临界区**（per-session 锁）。否则存在**丢通知窗口**：

  ```
  ✗ 错误：loop 已判定「无新消息」→ 准备退出 → 此刻通知落库
          → 通知方 ensure 看到 running=true → no-op → 双双不管，通知丢失
  ✓ 正确：两者持同一把锁——
          通知先到：ensure 持锁看到 running → no-op；
                    但 loop 在锁内的退出判定会看到这条通知 → 续跑 ✓
          loop 先退：loop 持锁判定无新消息并清 running → 释放；
                    ensure 持锁看到 !running 且有未消费 → 启动新 loop ✓
  ```

  `last_consumed_seq` = loop 每次把上下文消费到的上界回写（锁内）——它就是
  `ctx.last_seq` 在工作集里的投影（同一语义，一个字段，非独立水位机制）。
  **初值 = 段尾**（重启/重建后第一次组装必然读到段尾，视为已消费）——避免重启后
  把历史通知当「未消费」多起一轮（T1-24 的“不重现”在这一层兑现）。
- **与用户消息的区别（两类语义，不可混）**：
  - **用户消息**：排队（`turn_guard` 阻塞等待）——用户输入必须被处理，不能因
    「已有 loop」而丢弃；
  - **通知唤醒**：幂等（`ensure_loop_running`）——后到的通知由在跑的 loop 消化，
    不需要各自一轮。
- 现状缺陷（实测根因）：`request_wake`（`background.rs:661`）把唤醒当作**入队**
  （每次调用直接 spawn 一个任务，`turn_guard` 是 `lock()` 排队而非 `try_lock()`
  幂等），且 `AgentWakeForwarder` 的入口判定只在 **spawn 前**跑一次、排队中的任务
  执行时不再重判——7 条通知 = 7 个任务全部入队，依次重读整段历史。本决策连同
  §3 的水位删除一起消除该形态。

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
- **唤醒幂等与单写者同源**：`ensure_loop_running`（§4）的 `running` /
  `last_consumed_seq` 检查与 loop 退出判定共用同一把会话锁——不与写路径竞争，
  也不需额外锁。

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
   **其中“通知投递水位”是 seq 缺失的替代品**：一旦 seq 进入上下文，读进度就是
   推导值——T1-23 建的水位、条件唤醒、入口失效判定三件套随之全部退场（§3/§4）。
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
- **「为什么连水位都不需要？」** 回应：水位的唯一作用是回答「哪些通知还没进过
  模型视野」。而 seq 进上下文后，这个问题由 **`ctx.last_seq` vs `ws.last_seq` 的
  比较**直接回答（§4），无需旁路状态；无活动轮时则由「确保一个 loop 在跑」
  （`ensure_loop_running` 幂等，§4）回答。两者都不需要“读到哪了”的持久化字段——**T1-24（重启重放）随之机制性
  消失**（无投递动作，就无“重投历史通知”的路径）。保留水位只会多一份需持久化、
  需初始化、需三处同步的内存态（现状 `has_pending_notices` / `store_last_seq` /
  `mark_notices_delivered` 正是这份重复记账的成本）。
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
| 018（会话权威存储迁 SQLite） | **遵守**。工作集是缓存；写入仍经 `SessionStore`；VFS 例外不变。**无新增会话级字段**（水位删除，不再需要 `notice_cursor`）。 |
| 027（存储完整链 / 组装压缩点） | **遵守**。段 = 压缩点起；段外（压缩点前）经库按需读；不在存储层截断。 |
| 012 / 030（前缀快照持久化 + 压缩点刷新） | **继承并收口**。三个前缀私有方法归入工作集；新增内容哈希避免无意义重写。 |
| 013（串行化 + 唤醒） | **修订机制**。单写者保留（锁迁到注册表）；唤醒判定的“投递水位”整体删除，改由 `ctx.last_seq` vs `ws.last_seq` 的续跑判定 + `ensure_loop_running` 幂等承担；唤醒从“入队”改为“确保 loop 在跑”。 |
| 029 / 031（订阅快照 / 乐观渲染） | **延续**。订阅快照改为最近 N 条 + cursor；乐观渲染保留，落位改按 seq。 |
| 026（子智能体会话） | **扩展**。子会话同样有工作集（独立键）；级联删除（④）时一并移除。 |

---

## 影响文件清单

**core**

- 新增 `core/src/agent/working_set.rs` — `SessionWorkingSet` + `WorkingSetRegistry`
  + `WORKING_SET_IDLE_TTL`（**实施修订**：放 agent 模块而非 session——工作集承载
  `SessionState`，而 `agent` 已依赖 `session`，放 session 会形成模块循环依赖）
- `core/src/session/types.rs` — 无新增字段（水位删除；如需段重建辅助信息，用现有
  `compression_marker`）
- `core/src/session/store.rs` — `read_range(from, to)`（段外按需读；复用 seq 索引）
- `core/src/session/manager.rs` — 写方法定位为工作集内部依赖；顶部过时注释修正
  （"含 compression_marker 截断与消息上限兜底"与 ADR-027 矛盾）
- `core/src/agent/session_state.rs` — 承载职责迁入工作集（`cleanup()` 落地为卸载）
- `core/src/agent/agent_core.rs` — `load_and_build_state` / `assemble_context` /
  `ensure_injectable` / `persist_injectable_snapshot` / `invalidate_injectable_snapshot`
  → 经工作集；`update_session_header` 合并入 ③；`AgentWakeForwarder` 删除入口水位
  判定（改由 `ensure_loop_running` 幂等承担）
- `core/src/agent/coordinator.rs` — 五个入口改 `registry.ensure`
- `core/src/agent/loop.rs` — **删除** `notice_delivered` / `NOTICE_WATERMARK_INIT` /
  `store_last_seq` / `mark_notices_delivered` / `has_pending_notices`；
  `deliver_pending_notices` 改为“续跑判定 + `delta_since` 注入”（§4）
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
3. seq 一等公民（`WorkingEntry`）；`delta_since` 接入轮边界续跑判定；**删除通知
   投递水位三件套**（`notice_delivered` / `has_pending_notices` / `mark_notices_delivered`）。
4. 唤醒改为 `ensure_loop_running` 幂等（`running` / `last_consumed_seq`，与退出判定
   同一临界区）+ 空闲卸载落地。
   **验收**：连续轮/唤醒轮不再全量重建；N 条并发通知只产生 1 轮唤醒；重启后历史
   通知不重现；core 全绿。

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
4b. **唤醒幂等**：同一会话 N 条通知（N≥3）→ **只产生 1 个 loop**（判别力：
   改回“每条通知入队”（现状 `request_wake`）必红——回到 9 轮形态）。
4c. **丢通知窗口**（串行化判别力）：在 loop 收尾判定“无新消息”到退出之间注入一条
   通知 → **不得丢失**（判别力：把清 `running` 移到锁外/单独临界区必红）。
5. **重启重建**：重启后 `ensure` 段 = 压缩点后全部；**历史通知不重现**（判别力：
   重新引入“投递”路径必红——这正是 T1-24 的形态）；前缀逐字节一致。
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
| **T1-24** 重启重放历史通知 | 水位初值取「进程启动前最后一条 seq」 | **机制删除**：无投递动作、无水位，历史通知只在段里出现一次（§3/§4） |
| **U10** 停不掉唤醒轮 / 发了没响应 / 顺序乱 | 唤醒轮接取消标志；`turn_state` 事件；消息按服务端 seq 落位 | §9 推送收口（轮状态 + seq 落位）；§6 单写者排队提示 |

> 注：用户消息仍走 `turn_guard` **排队**（输入必须被处理，不能因“已有 loop”而丢弃）；
> 只有通知唤醒是**幂等**（§4）——两类语义不可混。

> T1-24 的快修与根治是**同一处代码的两次改动**：快修把水位初值改成“boot 前最后
> 一条 seq”（低风险、立即止血），阶段一直接把整套水位删掉。快修不是新增层，阶段
> 一落地后不留两套。

快修只做**最小正确性修补**（不加新层）；阶段一/三落地后，快修的实现自然被
工作集版本替换（不保留两套）。

---

## 实施记录（阶段一 · 2026-09-16）

**已完成（core 1283 全绿、clippy 0、fmt 干净；server 157 / tauri 13 全绿）**：

| 项 | 实现 |
|----|------|
| 工作集与注册表 | 新增 `core/src/agent/working_set.rs`：`SessionWorkingSet`（`store` + `state` + `last_seq` + `last_consumed_seq` + `last_activity`）、`WorkingSetRegistry`（`ensure` / `get` / `remove` / `lock` / `try_lock` / `sweep_idle` / `maybe_sweep`） |
| per-session 锁迁入 | `Agent::turn_locks` 字段删除 → 注册表；`turn_guard` 委托 `registry.lock`，新增 `turn_try_lock` |
| 入口经工作集 | `load_and_build_state` 有 store 时走 `ensure`（无 store 回退旧全量路径）；五个入口（`process_message` / `process_message_stream` / `process_wake` / `compress_session` / `rollback_session`）零改动自动受益 |
| 新鲜度自愈 | `ensure` 每次一次 `MAX(seq)` 比较，库尾不符即重建——外部直写（server 会话 API）不破坏一致性 |
| seq 与消费水位 | `mark_consumed`（单调 CAS）/ `has_unconsumed` / `delta_since`；重建与组装即消费到库尾 |
| 水位三件套删除 | `notice_delivered` / `NOTICE_WATERMARK_INIT` / `mark_notices_delivered` / `store_last_seq` / `has_pending_notices` 全部删除；`deliver_pending_notices` → `inject_pending_if_any` |
| 续跑判定 | `run_turns` 收尾处：有新消息 → 增量注入 + `continue`（自消化，不入队不唤醒） |
| 唤醒幂等 | `AgentWakeForwarder::wake` → `try_lock` + `has_unconsumed`；`process_wake_locked`（调用方持锁，避免重入死锁） |
| 落库收口（agent 层） | `Agent::persist_structured`（用户消息 / 助手消息 / 压缩摘要）；`AgentLoop::persist_turn_message` 经 `ws.append`；通知器 `persist_notification` 单点（任务 / 命令终态 / 就绪） |
| 空闲卸载 | `sweep_idle`（**跳过持锁中的会话**）+ `ensure` 机会式节流（60s）触发，不引入常驻任务 |

**判别力实证（注入旧行为 → 必红，均已复现后还原）**：

1. 续跑判定缺失（`inject_pending_if_any` 恒 `false`）→ `test_turn_boundary_injects_new_message_once` 红（"有新消息应注入并返回 true"）。
2. 重建水位归零（`last_consumed_seq.store(-1)`）→ `test_rebuild_marks_all_consumed` 红（-1 vs 2，即 T1-24 重放形态）。
3. 唤醒排队（`try_lock` 改走 `lock`）→ `test_try_lock_is_idempotent` 红（带 500ms 超时保护，不挂起）。

**本阶段未做（属阶段二/三，或需单独批次）**：

- **快照索引显式化**（解耦段化的那处小改）：捕获端 `capture_workspace_snapshot`
  仍用 `state.structured_messages.len()`（完整链下与全链位置恒等，行为不变）——
  尚未改为显式位置；段化前需先做（见 §2 实施修订）。
- **段化**（只物化压缩点后）：阻塞点 = 上面那处隐式表达；其余为工作集内部段结构
  （`start_seq` + 重建从压缩点读）+ 其余 `len()` 消费点审查。
- **server 写路径收口**：`delete_message` / `redo_message` / `update_title` 等仍直写库，靠 `ensure` 新鲜度自愈兜住（一致但会多一次重建）；待阶段二收口。
- **前端分段加载 / `ChatMessage.seq` / 订阅快照最近 N 条 / `turn_state`**：阶段二、三（§8/§9）。

---

## 回滚

回滚 = `git revert` 本 ADR 的代码变更，逐阶段可独立回滚（阶段一→二→三 无反向依赖）。
回滚即回到「每轮全量重建 + 分散写入」，**功能正确性不受影响**（只是唤醒轮风暴
等性能/体验缺陷回归）——这正是"非权威缓存"定位带来的安全回滚性质。
