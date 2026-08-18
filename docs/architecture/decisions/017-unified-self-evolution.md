# ADR-017: 统一自演化任务（EvolutionTask）—— 一个交给智能体自己的自我总结与演化

**日期**: 2026-08-19
**状态**: 提议（待评审）
**影响范围**: 调度任务（`core/src/scheduler/tasks/evolution_task.rs` 新增，`summary_task.rs` / `memory_task.rs` / `rule_task.rs` 并入或移除）、
技能学习（`core/src/skills/learning/`）、记忆提取（`core/src/memory/`）、
会话末演化触发（`core/src/agent/coordinator.rs` / `agent_core.rs`）、
角色注册表（`core/src/agent/roles.rs`）、配置（`core/src/config/`）、
任务注册装配（`server/src/lib.rs`）

---

## 背景

1. **现状：四类加工任务各自为政，产物互不收敛。** 天演现有三类定时加工任务
   （`summary_generation` 每 5 分钟、`memory_extraction` 每 10 分钟、`rule_extraction`
   每 15 分钟）加上会话末的 GEPA 技能/角色学习 hook（`learn_skills_from_session`）。
   它们的输入维度不同（全文 / 轨迹 / 摘要）、频率不同、产物互相独立：
   - 记忆（`memory/`）只写不合并去重、无冲突消解、无退役；
   - 技能（`skill/learned/`）与规则（`agent/learned/`）只能由模板类轨迹重复触发，没有
     "回头扫所有历史、对照现有注册表全局收敛"的视角；
   - 事实/记忆与技能/规则由两条不相交的输入（会话正文 vs 工具轨迹）生成，互不见面。
2. **缺口即讨论结论**：天演缺"跨会话综合提炼层"——一个定时/低频、把"未总结/未抽象的会话"
   与当前 智能体/技能/事实/记忆 注册表对照、逐个增删改的环节。现有水位线机制
   （`memory/events/extraction_state/{session_id}` 的 `last_extracted_message_count`、
   摘要的 metadata updated_at 比对）已经承担"找未处理增量"，但它们只做单点加工，不做全局收敛。
3. **目标形态（用户愿景）**：不再有记忆提取任务、技能提取任务、子智能体提取任务——
   只有**一个交给智能体自己的自我总结演化任务**。归纳机制（GEPA 或其它统计）可替换。
4. **承载能力已全部存在**：`process_wake`（ADR-013 唤醒轮：外部触发主 agent 跑一轮）、
   `delegate_to_agent` + 角色（ADR-016：三类来源平级、注册表 VFS 持久化）、
   `TaskScheduler` + `SchedulerTaskTrigger`（事件 `task:` 动作可触发任意调度任务）、
   VFS 双层摘要（L0/L1）。本 ADR 只做"收口"，不发明新积木。

---

## 决策

### 1. 统一演化任务 `evolution`

新增 `TaskHandler`（`EvolutionTask`），注册进 `TaskScheduler`：

- 任务 ID：`evolution`，显示名"自演化综述"；
- Cron：默认 `0 0 */24 * * *`（**每天一次**；自研 cron 只支持 `*/N` 间隔，
  小时位 `*/24` = 86400s，见 `parse_cron_interval`。可配置调高频率）；
- 仍受 `has_providers` 门控（与现状一致：无启用模型 provider 时不装配）。

任务内部固定四阶段（单实例、串行执行）：

```
Phase 0 采集    读未消费执行轨迹 + 未提取会话 + 缺摘要条目（增量，水位线驱动）
Phase 1 归纳    轨迹 → GEPA 技能/角色引擎；缺摘要条目 → 摘要引擎生成 L0/L1
Phase 2 综述    交给智能体自己：对照注册表，产出"增/改/删"diff 计划（只读，不落库）
Phase 3 守卫提交 逐类型验证门 + 软删除/版本回退保障 + 应用到 VFS + 演化报告
```

### 2. 阶段定义

**Phase 0 — 采集（增量）**
- 执行轨迹：会话结束时的行为从"即时喂 GEPA 学习"降级为"**持久化轨迹**"（廉价 hook、
  无 LLM、类似 `RuleRecorder`）：把本会话 `drain_execution_history()` 的批次追加到
  `tianyan://agent/_evolution/traces/{session_id}.jsonl`。演化任务读全部未消费轨迹
  （消费水位线见"状态与水位线"）。
- 未提取会话：复用 `memory/events/extraction_state/{session_id}` 水位线找
  `current_count > last_extracted_message_count` 的会话（现 `MemoryTask.scan_sessions` 逻辑原样搬入）。
- 缺摘要条目：复用 `SummaryTask.needs_summary`（Detail 比 Abstract/Overview 更新即缺）——metadata 驱动，无状态。

**Phase 1 — 归纳**
- 轨迹 → `SkillLearningEngine.learn_from_history` + `RoleLearningEngine.learn_from_history`
  （ADR-016 双管线，原样保留内部守卫：分类聚类、G2b 候选验证门、bigram dedup 门、成功率门控）。
- 缺摘要条目 → `SummaryEngine.generate_summaries` 写 L0/L1（**先补摘要再过综述**，
  让 Phase 2 读 L0/L1 而非全文，控制 token 成本）。

**Phase 2 — 综述（交给智能体自己）**
- 机制（可配置，默认 B）：
  - **B（推荐）**：`delegate_to_agent` 到内置新角色 `evolution_reviewer`
    （工具白名单 = 只读 VFS 检索 + JSON 输出，无任何写注册表工具）；
  - A：`process_wake` 唤醒主 agent + 演化指令系统消息（复用 ADR-013 唤醒轮）。
- 提示词输入：① Phase 0 找出的"未摘要/未提取"会话的 L0/L1 摘要 + 来源溯源；② 当前注册表清单
  （`memory/*`、`skill/learned/*`、`agent/learned/*`、`agent_role/*`）。
- 输出：**结构化 diff 计划**（JSON）：
  `{ "adds": [{type, content, importance, source_ids}], "updates": [{id, new_content, reason}], "deletes": [{id, reason, evidence}], "conflicts": [{id_a, id_b, description}], "new_roles": [...] }`
- 边界：每轮处理条目数上限（config `max_items_per_run`，默认如 200），防止单次任务 token 爆炸；
  综述角色**只产出计划、不直接改库**——"提议"与"提交"分离是本 ADR 的核心守卫。

**Phase 3 — 守卫提交**
- 逐类型验证门（迁移+保留现状守卫，位置从各任务尾部统一到提交前）：
  - preference 写前校验（`memory_extractor.verify_preference`，opt-in `verify_preferences`）；
  - skill/role 新增仍走 dedup（与已有技能摘要 bigram Jaccard ≥ 阈值 → 完善而非新建）；
  - **delete 必须有证据**：被新条目取代 / 评审低分（`skill/_reviews/` 复用）/ 用户确认；
    满足才算合法删除。默认 `delete_requires_evidence = true`。
- 删除一律**软删除**：条目移入 `_archive/`（保 lineage 与 version，可回退；对 ADR-016 版本链）。
- 应用 diff → VFS 写入、版本号递增、记录 `lineage` 理由；
- 写演化报告 `memory/events/evolution_reports/{ts}.md`（本周期新增/更新/删除/冲突清单 +
  统计），既是可观测性，也是回退依据。

### 3. 被替换 / 保留清单

| 现有项 | 处理 |
|--------|------|
| `summary_generation`（5min） | **移除注册**，逻辑并入 Phase 1 |
| `memory_extraction`（10min） | **移除注册**，逻辑并入 Phase 0/1/2 |
| `rule_extraction`（15min） | **移除注册**，`RuleSuggester` 聚类→规则提炼并入 Phase 1/3 |
| 会话末 `learn_skills_from_session`（GEPA/角色） | 从 coordinator 移除，改为"轨迹持久化 hook"（Phase 0 输入源） |
| `RuleRecorder`（失败即时记录） | **保留**（实时、管线内、无 LLM；"别犯第二次"不能等到每天） |
| ToolRegistry observability 轨迹采集 | **保留**（GEPA 输入源，非任务） |
| `garbage_collection` / `snapshot_gc` / `usage_stats_flush` / `reminder` | **保留**（工程维护/独立功能，明确不属于演化，不并入） |

### 4. 状态与水位线

- 复用 `TaskStateStore`（G5）：`evolution` 任务跨运行状态读写；
- 轨迹消费水位线：`agent/_evolution/state`（已处理到哪个批次的 URI/timestamp）——
  服务器重启不丢轨迹（轨迹已持久化，水位线保证续跑幂等）；
- 会话提取水位线：复用现有 `extraction_state/{session_id}`；
- 摘要：metadata（updated_at）驱动，无状态。
- **幂等**：Phase 2 产出前先对照当前注册表核对；Phase 3 仅应用"引用的条目仍存在且未被并发修改"的
  变更（写前 exists/updated_at 复核）。演化任务单实例串行，天然无自竞争。

### 5. 配置

新增 `[evolution]` 节（`core/src/config/`）：
`enabled`（默认 true）、`cron`（默认 `0 0 */24 * * *`）、`max_items_per_run`（默认 200）、
`review_role`（默认 `evolution_reviewer`）、`mechanism`（`delegate` | `wake`，默认 delegate）、
`delete_requires_evidence`（默认 true）。与记忆/提醒配置同模式（serde default fn + validate）。

### 6. 归纳器可替换（用户关于"GEPA 或其它统计"的确认）

本 ADR 将"归纳器"视为**可替换的实现**：GEPA（轨迹→技能/角色）是默认实现，
任何"从轨迹/文本到结构化产物"的方案（关键词统计、模板聚类、纯 LLM 生成等）都可在 Phase 1
内部替换。**守卫管线**（验证门/去重门/成功率门控/diff 提交分离）是架构价值，与具体归纳器解耦、
保留不变——换归纳器不影响本 ADR 的其余部分。

---

## 后果

### 正面
- 加工任务收敛为**一个**："只有一次交给智能体自己的自我总结演化任务"，符合用户愿景；
- 跨会话综合提炼层落地：去重/合并/冲突消解/退役/全局对照注册表的增删改；
- 统一演化节奏后，产物之间不再有"谁先谁后"的时序竞态；
- 会话末零 LLM（GEPA 移到低频任务），日常对话延迟与 token 消耗下降。

### 负面 / 代价
- 技能/角色学习从"会话末即时"变成"每天一次"：新技能/角色当天不可见
  （产物在 ADR-012 的会话边界/启动/压缩点刷新，次日自然可用；config 可调高频率缓解）；
- 全量综述 token 成本集中在单次运行（L0/L1 + `max_items_per_run` 控制）；
- 单点风险：演化任务失败则该周期无加工（水位线保证下周期续跑不丢数据）；
- 综述的 LLM 判断仍有劣化风险——靠 diff 提议/提交分离 + 删除证据门 + 软删除回退兜底，
  ADR-016 的 `version/lineage` 回退链原样保留。

## 边界条件（违反即重新评估）

- 综述导致注册表错删/劣化且回退未兜底 → 强化"delete 需要证据 + 软删除"策略，或改人工确认删除；
- 每天一次导致技能/角色时效性明显不足（用户体感）→ 拆出增量触发点（会话边界或低水位即时触发归纳）；
- `evolution` 单次运行超过成本上限 → 分片处理（分段水位线）或回退多任务拆分。

## 与既有决策的关系

| 决策 | 关系 |
|------|------|
| ADR-013（唤醒轮） | Phase 2 机制 A 直接使用 `process_wake`；演化任务完成后可用 `AgentWakeForwarder` 通知 |
| ADR-016（角色化自演化） | Phase 2 机制 B 使用 `delegate_to_agent`；演进产物落 `agent_role/`；本 ADR 补上 P2/P3 占位的"综合提炼/机会检测"环节 |
| ADR-012（前缀快照） | 技能/角色更新仍在会话边界/启动/压缩点刷新——演化任务产物在这三个免费刷新点自然可见 |
| ADR-001（VFS 双层摘要） | Phase 2 综述只读 L0/L1，不引入任何独立检索/存储 |
| REJECTED #17（全事件驱动） | 不冲突：`SchedulerTaskTrigger` 的 `task:` 动作只是"触发该任务"的入口，不引入事件总线 |

## 后续演进（占位）

- 演化报告进 GUI（面板展示最近 N 次演化增删改统计 + 单条 lineage 回退按钮）；
- `delete` 待人工确认模式（高风险条目走审批，复用 `executor/approval`）；
- 冲突消解结果沉淀为新记忆类别（如 `resolution`）供检索。

## 关键文件

- `core/src/scheduler/tasks/evolution_task.rs` — 新增：四阶段编排（采集/归纳/综述/守卫提交）
- `core/src/scheduler/tasks/{summary_task.rs,memory_task.rs,rule_task.rs}` — 并入后移除注册（逻辑迁入 EvolutionTask 各阶段）
- `core/src/agent/coordinator.rs` + `core/src/agent/agent_core.rs` — 移除会话末 `learn_skills_from_session`，改为轨迹持久化 hook
- `core/src/agent/tool_registry/observability.rs` — 轨迹持久化写入 `agent/_evolution/traces/`
- `core/src/agent/roles.rs` — 新增内置角色 `evolution_reviewer`（只读白名单 + JSON diff 输出）
- `core/src/config/` — 新增 `[evolution]` 配置节
- `server/src/lib.rs` — 任务注册改为 `evolution`（移除 summary/memory/rule 三个注册）
