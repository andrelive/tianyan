# ADR-019: 服务端权威消息时间线（ChatMessage.segments 统一渲染）

**日期**: 2026-08-23
**状态**: 已采纳（用户确认：天演未上线，不考虑向后兼容）
**影响范围**: 服务端消息序列化（`server/src/api/shared/types.rs`、`server/src/api/chat/`）、
会话历史加载（`server/src/api/sessions/services.rs`）、前端消息合并
（`gui-vite/src/lib/store/chat-slice.ts`）、渲染（`gui-vite/src/components/chat/MessageBubble.tsx`）

---

## 背景

1. **历史与流式渲染分叉**：流式期间前端本地累积 segments（按事件到达顺序的时间线），
   历史消息无 segments → `MessageBubble` 走固定顺序分支（thinking → content → tool_cards）。
   结果：**同一会话刷新前后显示不一致**——流式时工具卡片夹在正文中间，历史时全部按类型堆叠。
2. **顺序信息在序列化时丢失**：core 持久化的 `StructuredMessage.parts: Vec<Part>`
   本身就是真实到达顺序（Reasoning / Text / ToolCall 交错，ADR-002 单一真相源），
   但 server 序列化 `ChatMessage` 只导出平铺字段（thinking / content / tool_calls），
   交错顺序无法还原。
3. **渲染层无法自行重建顺序**：前端只有按类型分组后的平铺字段，区分不了
   "先思考后调用工具"与"调用中穿插正文"——双分支歧义是结构性必然，不是前端 bug。

## 决策

**服务端成为消息时间线的唯一权威：`ChatMessage` 新增 `segments` 字段，由
`StructuredMessage.parts` 按持久化顺序生成；历史加载与流式边界事件携带同一份
时间线；前端 `applyServerMessage` 以服务端 segments 为准。**

### 1. DTO（`server/src/api/shared/types.rs`）

- `ChatMessage.segments: Option<Vec<MessageSegment>>`；
- `MessageSegment`（serde tag=\"type\"，snake_case，与前端 `MessageSegment` 类型对齐）：
  - `{ "type": "thinking", "text" }` — 由 Part::Reasoning 生成
  - `{ "type": "text", "text" }` — 由 Part::Text 生成
  - `{ "type": "tool", "tool_call": { id, name, arguments, presentation } }` — 由
    Part::ToolCall 生成；`presentation` 复用 `ToolRegistry::default_presentation`，
    历史/流式卡片图标与标签一致
- `segments_from_parts(role, parts)`：**仅 assistant 消息产生**（思考/工具调用只属于
  assistant）；Part::ToolResult / Part::Image 不产生段（结果合并到 `tool_calls` 卡片、
  图片独立渲染）。

### 2. 两条路径携带同一时间线

- **历史加载**：`get_session_detail`（`api/sessions/services.rs`）映射消息时生成 segments；
- **流式边界**：`from_structured_light`（`api/shared/types.rs`）生成——边界事件
  （chunk_type=message，流结束携带完整 ChatMessage）即流式结束时的权威校准；
- 非流式/错误响应的 ChatMessage 构造补 `segments: None`。

### 3. 前端合并语义（`gui-vite/src/lib/store/chat-slice.ts`）

`applyServerMessage`：`segments: msg.segments ?? base[target].segments` ——
**服务端优先**；无 segments 的旧数据回退本地累积（防御性；天演未上线无迁移包袱）。

### 4. 渲染（`gui-vite/src/components/chat/MessageBubble.tsx`）

`message.segments` 非空 → `SegmentBlocks` 按序渲染思考/正文/工具卡片（工具段按
id/名称匹配 `message.tool_calls` 合并执行结果）；固定顺序分支仅作旧数据兜底。

## 理由

1. **单一权威**：顺序知识只存在于一处（core parts），序列化不再丢信息；
   渲染层删除本地顺序逻辑，消除双分支。
2. **刷新一致性**：历史与流式进入同一条渲染管线，同一会话刷新前后显示一致。
3. **契约可测试**：segments 是纯函数映射（parts → segments），server 单测锁定
   顺序与序列化形状；前端回归测试锁定历史消息时间线渲染；契约快照
   （contract.test.ts）防 DTO 漂移。

## 反方观点与回应

- **"前端本地累积更简单，无需改协议。"** 回应：本地累积正是分叉根源——顺序知识
  在前端重复实现，历史数据无法回放时间线。协议加一个字段换来渲染层单一管线。
- **"segments 与 thinking/content/tool_calls 内容重叠，增加 payload。"** 回应：
  冗余序列化（同内容出现两次）；本地单用户桌面场景带宽成本可忽略，换取渲染一致性。

---

## 影响文件清单

- `server/src/api/shared/types.rs` — ChatMessage.segments + MessageSegment +
  segments_from_parts + from_structured_light 生成（+ 单测 2 例）
- `server/src/api/sessions/services.rs` — 历史加载携带 segments
- `server/src/api/chat/services.rs`、`server/src/api/chat/types.rs` — 非流式/错误构造补 segments: None
- `gui-vite/src/lib/store/chat-slice.ts` — applyServerMessage 服务端优先
- `gui-vite/src/components/chat/MessageBubble.tsx` — 双分支渲染（segments 时间线 /
  旧数据兜底）+ 回归测试
- 文档：module-descriptions §4.4、module-relationships §5.3（本文档）

## 回滚

服务端停止生成 segments（移除 segments_from_parts 调用）+ 前端回退本地累积；
天演未上线，无存量数据兼容负担。
