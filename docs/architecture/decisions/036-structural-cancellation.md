# ADR-036: 结构性取消——等待盲区收口（drop 即取消）

**日期**: 2026-09-19
**状态**: ✅ 已采纳（已实施 2026-09-19）
**影响范围**: 取消原语（新增 `core/src/agent/cancel.rs`）、Agent 循环
（`core/src/agent/loop.rs`）、Agent 协调器（`core/src/agent/agent_core.rs`）、
模型层生产者（`core/src/model/provider/chat.rs`）、前端（`gui-vite/src/`：
`components/chat/ChatPanel.tsx`、`components/chat/ChatInput.tsx`、
`lib/store/chat-slice.ts`、`lib/chat-stream.ts`）

---

## 背景（「停止按钮有时候按了没反应」的机制根因）

取消的**置位链路**此前已统一：会话级 `Arc<AtomicBool>` 槽（停止端点
`request_cancel` / 服务关停 `cancel_all_sessions` / 唤醒轮自注册槽），
工具执行层已支持中断（T1）。但**响应侧只有"同步检查点"一种原语**——
3 处 `is_cancelled()`（流式 chunk 循环 / 轮顶 / 错误上抛）。同步检查覆盖
不了"阻塞时长 > 检查点密度"的窗口，实测三处 await 盲区：

| 盲区 | 机制 | 实测症状 |
|------|------|---------|
| A. LLM 请求在飞 | `chat_completion_stream` / `chat_completion` 的 future 等待响应头/首块期间无检查点 | 长思考模型 10–30s+ 不可取消 |
| B. 上下文组装 / 压缩 | `assemble_context`（检索/加载）与 `maybe_compress_and_persist`（压缩 LLM 调用）无取消传导 | 15s 延迟实证 |
| C. 流式 recv 间隙 | 检查点在 `rx.recv()` **返回之后**——无新 chunk（长思考停顿）时取消永不生效 | 停顿期间完全无效 |

**结论**：这不是三个独立 bug，而是同一机制缺口——响应侧缺"可等待"原语。
沿旧思路"在每个阻塞段补检查点"必然零碎且漏（检查点枚举永远不完备）；
照此修会得到 6–7 个互相独立、各写各的检查点，下一个阻塞点仍会漏。

## 决策

### 1. 统一语义：**"drop 即取消"（结构性取消）**

Rust/Tokio 的 future 被 drop 时，其内一切在途操作（HTTP 请求、通道等待、
退避重试）自动终止——取消**不需要逐层传导信号**（model / context 层签名
零改动），只需在**等待处**与取消信号竞争。

### 2. 原语单点（`core/src/agent/cancel.rs`）

全局唯一"如何等待取消"的知识点：

- `wait_cancelled(flag)`：已置位立即返回（零延迟）；未置位 50ms 粒度检查
  （与工具执行层既有取消粒度一致，见 `execute_command_action_cancellable`）；
  `None` 永不就绪（select 语义下不影响其它分支）。
- `cancellable(flag, fut)`：与取消竞争——取消先到 → `fut` 被 drop；
  **预置位快速路径不启动 fut**（避免"点击停止后仍发起新请求/新压缩"）。

### 3. 使用点（五处等待，模式统一）

| # | 点 | 位置 | 取消后行为 |
|---|----|------|-----------|
| ① | 流式请求发送 | `loop.rs::collect_streamed_turn` | 在途 HTTP / 退避重试终止 → 取消上抛（无内容不落库） |
| ② | 流式 recv | 同上 chunk 循环 | `accum.cancelled` → 已交付前缀走既有 interrupted 收尾 |
| ③ | 组装 | `agent_core.rs::run_agent_turn` | 轮未启动 → 早期取消收尾（复用 `apply_loop_result` 取消分支） |
| ④ | 压缩（轮前 / 轮末） | 同上 | 跳过（摘要下轮重算，幂等）；不回改已完成的轮结果 |
| ⑤ | 非流式请求 | `loop.rs::run` 的 step 闭包 | 同 ① |

### 4. drop 传导闭环（model 层生产者）

`chat.rs` 流读取任务补 `tx.closed()` select：接收端 drop → 立即退出读取
并断开连接（旧实现挂在 `timeout(read_idle)` 上——静默流要等下一块数据
或空闲超时才退出）。

### 5. 收尾语义零新增

所有新取消点走**既有**路径：已交付前缀 interrupted 落库（②）、
`AgentLoopResult::Cancelled`（①③⑤）、跳过压缩（④）。**不引入第二套
取消收尾语义**——这是本方案与"外层 abort 整轮"（REJECTED #21）的本质
区别。

### 6. 前端（「正在停止…」过渡反馈）

点击停止 → 后端收尾完成（`turn_state idle` 事件）或端点返回
`no_active_stream` 之间的窗口给出明确反馈：持久横幅 + 停止按钮禁用态
（`stoppingBySession` 纯前端内存态，不落库；20s 兜底防悬挂）。修复前该
窗口被"前端乐观置 idle + 后端继续跑"的割裂掩盖——用户观感即"按了没反应"。

## 非目标与后续

- **0ms 信号级（Notify/watch 通知）为后续计划**：届时只改 `cancel.rs`
  内部实现（`wait_cancelled`），五个调用点一行不动——接口先行。
- **不做全量类型升级**（`Arc<AtomicBool>` → `CancelToken`）：20+ 处类型
  迁移 + Notify"检查-等待"竞态处理，收益仅 50ms→0ms（人类无感）。
- **不做外层 abort 整轮**：见 REJECTED #21。

## 测试

- `cancel.rs` 原语单测 7 项：预置位零延迟 / 等待间隔唤醒 / `None` pending /
  正常完成透传 / 快速路径不启动 / 打断挂起 future / `None` 直接等待。
- `test_run_stream_cancel_during_recv_wait`（旧实现必红：recv 阻塞中取消
  不生效 → 挂死超时）：等待间隙取消 → Cancelled + 前缀 interrupted 落库。
- `test_raw_stream_producer_disconnects_on_receiver_drop`（旧实现必红：等
  read_idle=30s）：接收端 drop → producer 立即退出并断连。
- `test_cancel_at_turn_end_skips_compression`（旧实现必红：超阈值必调压缩
  mock）：轮末取消 → 跳过压缩、轮结果正常交付。
- 前端 store / 事件处理 2 项：`setStopping` 状态机 + `turn_state idle` 清除。
