# ADR-041: 控制面命令化与租约——Command + Idempotency + Lease

**日期**: 2026-09-24
**状态**: ✅ 已落地（波次 3 服务端 + 波次 4 前端，2026-09-24）
**修订**: ADR-028/029/031（事件与订阅）之补充——控制面（非数据流）的互斥与幂等
**影响范围**: 服务端状态（`server/src/state.rs`）、对话与会话 API
（`server/src/api/{chat,sessions}/`）、前端（`gui-vite/src/components/chat/`、
`gui-vite/src/lib/store/chat-slice.ts`）

---

## 背景（2026-09-24 侦查实证）

**症状**：① 撤销回退横幅延迟数秒、甚至下一轮之后才出现；② 回退期间发送消息偶发被吞；
③ 回退偶尔取消掉用户刚发起的新轮。

| 现状 | 证据（代码） | 后果 |
|------|-------------|------|
| 流互斥是**裸标志位** | `stream_cancels: HashMap<String, Arc<AtomicBool>>`（`state.rs:296-300`）；`try_register_stream` 409（`state.rs:531-547`、`register_stream_slot`） | 只表达"有流在跑"，不表达"有控制面操作在跑" |
| 槽释放**无 owner 校验** | 流结束 `remove(&session_id)`（`chat/handlers.rs:130-133`） | 任何后到者都可清掉他人槽（当前靠 409 侥幸安全） |
| 回退**无条件置位当前流 cancel** | `sessions/handlers.rs:145-155` | 若槽内已是新轮 flag → **取消用户刚发的轮** |
| 回退无"进行中"语义 | 无任何状态标记；`rollback_session` 返回后仍在截断/快照 | 前端只能"等"，无进度、无禁用、可重复点击 |
| 前端操作态靠推导 | `ChatPanel.tsx` 内 **8 处** `streamStatus === / !==` 判定 + 并行 `turnState` 来源；横幅条件 `lastRollbackMessageId !== null && streamStatus !== 'streaming'`（`:653`） | 横幅延迟/残留/被新轮压住；规则无承载物 |
| 迟到响应**无条件覆盖** | `setSessionMessages(sessionId, resp.messages)`（`ChatPanel.tsx:390`） | 旧响应抹掉新轮消息与流式内容 |
| 409 语义被复用为"断线" | `onError` → `setStreamError(true)`（`ChatPanel.tsx:243-249`），UI 文案"连接断开，任务继续在后台运行" | 用户被告知错误原因（实为并发保护） |

---

## 决策

### 1. `stream_cancels` 升级为 `session_leases`（概念 1 换 1，不叠加）

同一张注册表升级，**不新增第二个注册表**：

```rust
enum LeaseKind { Stream, DestructiveOp }   // 用户轮/唤醒轮 | 回退/重做/压缩/删除

struct SessionLease {
    kind: LeaseKind,
    owner: LeaseToken,          // 唯一 token（非 Arc 指针复用，避免复用误判）
    cancel: Arc<AtomicBool>,    // Stream 的取消标志（保留现有语义）
    op_id: Option<String>,      // 控制面命令的幂等键
}
```

规则（**状态机式，一处表达**）：

| 已有 \ 请求 | Stream | DestructiveOp |
|------------|--------|---------------|
| 无 | ✅ 授予 | ✅ 授予 |
| Stream | ❌ 409 `stream_in_progress` | ❌ 409 `stream_in_progress`（先停止并等待轮退出） |
| DestructiveOp | ❌ 409 `destructive_op_in_progress` | ❌ 409 `destructive_op_in_progress` |

释放必须校验 `owner`（token 相等）——修掉当前"无 owner 校验"。

### 2. 控制面命令化 + 幂等键

- 控制面操作 = Command：`rollback` / `redo` / `compress` / `delete_session`，
  请求携带客户端生成的 `operation_id`；
- 服务端**每会话**保留最近 N 条命令结果（进行中 / 已完成 + 结果），
  重复 `operation_id` → 返回既有结果（**不重复执行**，杜绝双击/网络重试二次截断）；
- 错误语义结构化：忙 → `409 + reason ∈ {stream_in_progress, destructive_op_in_progress}`；
  命令失败 → 结构化 `RollbackOutcome` 级错误（ADR-040）。

### 3. 前端：操作状态机（State Machine）替代推导

```ts
pendingOpBySession: Record<string, {
  op: 'rollback' | 'redo' | 'compress' | 'delete';
  opId: string;
  phase: 'in_flight' | 'done' | 'failed';
}>;
```

- UI 由 `pendingOp + turnState` 驱动：横幅、禁用、进度指示**只读状态**，
  不再从"消息列表 + streamStatus"现场推导（删除 8 处判定中的推导类）；
- `in_flight` 期间：输入禁用、操作按钮禁用、显示"回退处理中…"
  （消除"点了没反应 / 重复点击"）；
- 迟到响应按 `opId` 匹配，不匹配即丢弃（消除横幅重贴）。

### 4. 前端：乐观并发（OCC）单调合并

- 会话消息响应/事件携带 `version`（复用 `last_seq`，或 header 版本）；
- store 写入改为**单调合并**：只接受 `version ≥ 本地 version`，旧响应直接丢弃；
- 与 ADR-031 的乐观渲染互补：乐观 = 先显示，OCC = 后到的旧版本不许回退 UI。

---

## 替代方案与否决理由

| 方案 | 否决理由 |
|------|---------|
| A. 只加前端守卫（禁用按钮 / 延迟） | 窗口在服务端；前端守卫挡不住唤醒轮、定时任务、多入口 |
| B. 引入分布式锁 / 外部协调 | 当前单进程（server + tauri 包装）假设成立；接口收窄以便未来替换，不提前引入 |
| C. 前端轮询命令状态 | 与 ADR-028/029 事件推送模型相悖；轮询是"抽水机"，事件已常驻 |
| D. 保留 `stream_cancels` 并另加 `pending_ops` 表 | 两张表 = 两处规则 = 状态组合爆炸（正是当前 8 处判定的来源） |

---

## 后果

**正面**
- 控制面互斥与"进行中"语义**单点表达**（一处状态机），替代散落守卫；
- 双击/重试幂等：不会二次截断、不会重复快照恢复；
- 回退不再误取消新轮；409 语义可区分并可被用户理解；
- 前端"迟到响应覆盖"在数据层面不可能（OCC）。

**成本 / 负面**
- 前端需引入 `pendingOp` 状态与 version 字段（`chat-slice` 改动）；
- 服务端需命令结果缓存（每会话内存环形，随会话 TTL 清理）；
- 客户端必须生成 `opId`（约定，非强制）——服务端对**无 opId** 的请求保持非幂等
  （向后兼容），文档需明示。

---

## 实施（波次 3–4）与验收判据

波次 3（服务端）：`session_leases` 升级 + owner 校验 + 命令幂等 + 忙语义；
波次 4（前端）：`pendingOp` 状态机 + OCC 单调合并 + 禁用/进度 + 409 文案分流。

**验收判据（判别力测试）**
- 双击回退（同 `opId`）→ 断言只截断一次、只保存一份可逆数据；
- 回退 in-flight 时发起新流 → 断言 409 `destructive_op_in_progress`，且前端显示
  "回退处理中"（不显示"连接断开"）；
- 回退请求**迟到返回** → 断言本地消息列表未被覆盖（version 更低被丢弃）；
- 旧行为注入（无 owner 校验 / 无 opId）→ 对应测试变红。

---

## 实施记录（2026-09-24）

### 波次 3（服务端，commit `934db54`）

- `session_leases`（**原地升级** `stream_cancels`，不新增第二张表）：`Stream` /
  `DestructiveOp` 互斥 + owner 令牌 + `opId` 台账 + 忙语义（409 + 结构化 reason）；
- **接管语义**（对草案的细化）：控制面命令到达时若有流在跑，不返回 409，而是
  「置位旧流取消标志 + 就地改写为 `DestructiveOp`（挡住新流）+ 等会话静默」——
  保留用户「点回退即停轮」的既有体验，同时杜绝「新轮插进回退事务」与
  「回退误取消用户刚发起的新轮」（旧实现无条件置位**当前**流的 flag）。
  静默超时（10s）放弃 → 409（宁可拒绝，也不与仍在写会话的旧轮竞态）；
- `delete/redo/compress` 统一控制面门 `gate_control_op`；请求可携 `operation_id`
  （缺省保持非幂等——向后兼容）；`ApiError::Busy` + `ErrorResponse.reason`；
- 7 条判别力单测（接管 / 接管后静默观测 / 幂等两态 / 控制面互斥 / 停止范围 /
  会话隔离 / 等静默超时）。

### 波次 4（前端）

- `controlBusy`（rollback/redo）+ **每会话操作序号**（迟到响应守卫）落地：
  进行中反馈、重复发起拒绝、迟到响应丢弃；`ChatInput.busy` 输入禁用；
- **与草案的有意偏差**：不做全量 `version` 单调合并，而用「in-flight 期间输入禁用
  （服务端互斥 + 前端 busy）+ 操作序号守卫」达到同一目标（旧响应不许回退 UI）。
  理由：消息级 version 需贯穿 store 的所有写入路径（`applyServerMessage` /
  `confirmUserMessageId` / 流式 upsert…），改动面与回归风险远大于收益；当前方案
  已覆盖实际窗口（控制面操作是唯一会「旧响应覆盖新状态」的来源，且被禁用与序号
  双重挡住）。**重新评估触发条件**：出现非控制面来源的迟到整体替换时，升级为
  version 方案；
- 忙文案分流（reason → 三类可读提示）；横幅去掉 `streamStatus` 依赖（消除被
  流状态压住的延迟）。
