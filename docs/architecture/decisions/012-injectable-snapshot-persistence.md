# ADR-012: 注入上下文快照持久化 —— 前缀零漂移 + 会话边界/压缩点技能刷新

**日期**: 2026-08-09
**状态**: ✅ 已采纳（存储载体更新：SessionHeader 存 `session_meta.header_json`，ADR-018；注入/刷新语义不变）
**影响范围**: 会话存储（`session/`）、上下文管线（`context/`）、Agent 编排（`agent/agent_core.rs`）、server 装配（`server/src/state.rs`）

---

## 背景

system 前缀（soul + learned rules + memories，即 `InjectableContext`）是 prompt 缓存的 key 组成部分。审查发现：

1. **前缀随会话固化缺失**：`injectable_context` 仅为内存级会话缓存——重启后 `load_and_build_state` 重建，旧会话恢复时**重新检索最新 rules**，前缀内容与重启前可能不同（GEPA 期间学到新规则）→ 旧会话前缀缓存失效 + 语义漂移（旧会话突然注入新经验）。
2. **GEPA 进化产物可见性盲区**：新技能/新规则只对"启动后首次加载的会话"可见；长会话进行中（会话级缓存冻结）和重启后（重新检索）的行为不一致。
3. **压缩点未利用**：压缩（摘要消息插入）已重建 system 前缀——这是会话内**唯一的免费刷新点**，但未用于刷新 learned rules / 技能注册表。

## 决策

**前缀内容按会话快照固化；三个免费刷新点（启动全量 / 新会话增量 / 压缩点刷新）承载全部更新。**

### 1. 注入上下文快照持久化（JSONL 首行 SessionHeader）

- `InjectableContext` 加 `Serialize`/`Deserialize`；`SessionHeader`（`session_header` 标记恒为 1，与消息行天然互斥）存于会话 JSONL **首行**
- 首次加载（无快照）→ 检索 → 写内存 + **持久化快照**；重启后 `load_and_build_state` 直接恢复快照，**不重新检索**（前缀与重启前逐字节一致 → 缓存命中）
- `rewrite_messages` 保留 header（会话编辑/清空不丢快照）；旧格式会话（首行即消息）兼容加载，下次重写时补齐

### 2. 会话边界技能刷新（新会话增量注册）

- `SkillManager::refresh_registry`（幂等：contains 去重，返回新注册数）——新会话创建时（`resolve_or_create_session` is_new）把 GEPA 新技能增量注册进 SkillRegistry，新会话立即可见
- 会话内保持冻结（前缀稳定，prompt 缓存不失效——pi 拒绝 MCP 同源考量：tools-in-context 的 token 成本）

### 3. 压缩点刷新（会话内唯一免费刷新点）

- 压缩（自动 / 手动 `POST /sessions/{id}/compress`，共用 `maybe_compress_and_persist`，仅触发点不同）成功后：
  a. 清空 injectable_context（内存 + 持久化）→ 下一轮重新检索 learned rules（GEPA 经验对会话后续阶段可见）
  b. 触发 `SkillRefresher`（压缩时增量注册技能）
- 压缩已重建前缀，此刻刷新零额外缓存成本

## 后果

### 正面
- 重启后旧会话前缀零漂移（缓存命中、语义稳定）；新会话看到最新进化；压缩后长会话进入新阶段
- GEPA 进化产物可见性无时间盲区：启动（全量）→ 新会话（增量）→ 压缩点（增量 + rules 重载）
- 手动压缩 API 让用户主动触发阶段转换

### 负面 / 代价
- 会话 JSONL 增加首行 header（含快照内容，rules 大小随检索结果）；`update_session` 写回 header 需全量读写（低频，可接受）
- 快照固化的旧会话看不到新技能（直到压缩/新会话）——这是"前缀稳定"约束的必然代价（设计权衡，非缺陷）

### 边界条件（违反即重新评估）
- 会话内（未压缩）**不得**重新加载 injectable / 刷新技能列表——前缀稳定是硬约束
- SessionHeader 标记必须保持与 StructuredMessage 字段互斥（`session_header: Some(1)`）
- 压缩点清空快照后，若进程立即退出（未经过下一轮 prepare_context），持久化快照保持清空——重启后重新检索（可接受，压缩已发生）

## 关键文件

- `core/src/session/types.rs` — SessionHeader；`core/src/session/manager.rs` — 首行解析/写入
- `core/src/skills/manager.rs` — refresh_registry / SkillRefresher
- `core/src/agent/agent_core.rs` — 快照恢复/固化/压缩点清空
- `server/src/state.rs` — SkillSync；`server/src/api/chat/services.rs` — 会话边界触发
- `server/src/api/sessions/` — 手动压缩端点
