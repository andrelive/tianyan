# ADR-017: 统一自演化架构（摘要独立 + 每日演化智能体任务 + GEPA 数据层 + FTS5 会话回忆）

**日期**: 2026-08-19
**状态**: 已采纳（设计定稿，待实施）
**影响范围**: 调度任务（`core/src/scheduler/tasks/`）、技能学习（`core/src/skills/learning/`）、
记忆提取（`core/src/memory/`）、会话存储与检索（`core/src/session/`）、
检索路由（`core/src/context/retrieval/`）、可观测性（`core/src/agent/tool_registry/observability.rs`）、
角色注册表（`core/src/agent/roles.rs`）、配置（`core/src/config/`）、任务注册装配（`server/src/lib.rs`）

---

## 背景

1. **现状：四类加工任务各自为政。** `summary_generation`（5min）、`memory_extraction`（10min）、
   `rule_extraction`（15min）三个定时任务 + 会话末 GEPA 技能/角色学习 hook，输入维度不同
   （全文/轨迹/摘要）、频率不同、产物互不收敛：记忆只写不合并去重、技能/规则只能由模板类
   轨迹重复触发、事实与技能由两条不相交的输入生成。
2. **"会话末"是伪概念。** `Session.end()` / `ended_at` 是死代码（仅测试调用），无 API/GUI 入口；
   会话是跨天跨周的长期 JSONL 文件。任何依赖"会话结束"事件的设计都不可靠。
3. **四项演化工作圈定**（讨论确认）：
   - **记忆**：从会话内容绘制用户画像、总结喜好（`memory/`）；
   - **摘要**：对 VFS 内容摘要化，支撑 L0/L1 向量检索；
   - **技能**：从执行数据、对话材料总结工作经验与方法论（`skill/learned/`，规则 `agent/learned/` 归入）；
   - **子智能体**：从协作数据探索组织协作模式（`agent_role/`）。
4. **目标形态（用户愿景）**：不再有记忆提取任务、技能提取任务、子智能体提取任务——
   只有一个交给智能体自己的自我总结演化任务。归纳机制（GEPA 或其它统计）可替换。
5. **检索现状**：会话回忆（"刚才/之前/上次"）走向量检索会话 L0/L1——语义弱（关键词指代）、
   成本高（embedding + 摘要生成）。

---

## 决策

### 1. 摘要独立（业务逻辑不变，降频 6-8 小时）

- `SummaryTask` 保留，cron 从每 5 分钟改为每 6-8 小时（`0 0 */6 * * *` / `0 0 */8 * * *`；
  自研 cron 仅支持 `*/N` 间隔）；
- 低风险依据：知识导入路径**同步生成摘要**（`knowledge/ingestor` 内联 `generate_summaries`），
  记忆/技能/角色/规则写入时内联写 abstract——SummaryTask 只是兜底（补缺摘要/过期摘要）；
- **范围排除 Session 命名空间**：会话不再生成 L0/L1（回忆改 FTS5，见决策 6）。

### 2. 每日演化智能体任务（`evolution`）

新增 `TaskHandler`（`EvolutionTask`），注册进 `TaskScheduler`：

- 任务 ID：`evolution`，cron 默认 `0 0 */24 * * *`（每天一次），仍受 `has_providers` 门控；
- 内部三阶段（单实例、串行）：

```
阶段 0 采集    水位线驱动的"新材料"清单：新消息批次（消息索引 seq 水位线）、
              新执行记录（GEPA 数据层时间水位线）、新委托记录
阶段 1 综述    演化智能体（delegate 到内置角色 evolution_reviewer）：
              读统计工具 + 自行查看历史记忆/技能/组织形态 → 输出增删改 diff 计划
阶段 2 记账提交 软删除/归档 + 版本链 + 演化报告（无机械阈值，仅记账）
```

- **演化智能体**：`delegate_to_agent` 到内置新角色 `evolution_reviewer`
  （工具白名单 = 只读 VFS + 统计查询 + 会话回忆，无写注册表工具）；专门提示词约束：
  写新技能前查现有技能（语义重复则完善而非新建）、只把稳定跨会话偏好写入画像、删除需证据；
- **无机械阈值**：去重（Jaccard）、偏好校验（verify_preference）、G2b 打分全部移除——
  由智能体判断 + 提示词承担（智能体具备查看历史的能力）；
- **幂等**：`TaskStateStore` + 水位线；阶段 2 仅应用"引用的条目仍存在"的变更。

### 3. GEPA 数据层（不再生成；完整 GEPA = 数据层 + 判断层 + 记账层）

- **数据层**：observability 监听器把每次工具执行记录持久化到 SQLite（`executions` 表：
  task_description、category、success、耗时、skills_used、session_id、时间戳、steps JSON）；零 LLM；
- **统计查询工具**（供演化智能体调用）：`execution_stats`（按类别/时间/成功率，支持 since 参数）、
  `execution_detail`（原始记录）、`delegation_stats`（角色使用统计，复用 `record_role_usage` 数据）；
- **移除**：`SkillLearningEngine` / `RoleLearningEngine` 的 LLM 生成、G2b 门、Jaccard 门、
  成功率门控——生成与判断并入演化智能体；
- **完整 GEPA 定义**：演化循环 = 环境感知（数据层统计）→ 生成候选（智能体）→ 评估（智能体对照历史）
  → 选择（智能体决定）→ 保留/淘汰（版本链/归档）。现在的机械流水线是残缺版，
  加上智能体判断层才是完整 GEPA。

### 4. 片段模型（替代"会话末"）

- 处理单元 = **片段（episode）**：压缩点 marker ∪ 空闲间隔（>24h）∪ 新会话；
- 会话 = 片段序列；跨天会话的"周一聊 X、周三聊 Y"各自成片段，独立总结/对照注册表；
- 增量 = 水位线：消息 seq 水位线（自上次提取以来的新消息）、执行记录时间水位线；
- 不依赖任何"会话结束"事件。

### 5. 规则归入技能 + 实时记录保留

- `RuleRecorder`（工具失败即时记录，无 LLM、管线内）**保留**——"同一失败别犯第二次"不能等每日任务；
- 规则**提炼**（聚类 → 规则升级，原 `RuleTask`）并入演化任务阶段 1；
- 规则产物 `agent/learned/` 与技能 `skill/learned/` 同属"经验方法论"，同一演化管线管理。

### 6. FTS5 会话回忆（方案 A：JSONL 权威 + 派生消息索引）

- **会话不进向量检索**：不再生成会话 L0/L1，省掉摘要 + embedding 成本；
- **JSONL 保持权威**（VFS 不变，会话加载/压缩/快照零改动）；
- **派生消息索引**（SQLite，可重建）：`messages` 表（session_id、seq、message_id、role、
  text——仅 user/assistant 文本、time、tokens、has_tool）+ FTS5 倒排索引
  （trigram tokenizer 支持中文子串匹配，BM25 排序）；
- **写入**：`append_message_to_vfs` 同步插索引行（hook，零 LLM）；
- **回忆流程**：intent 路由 Session 关键词 → `SessionRecall` 服务 → FTS5 搜索 → 命中定位（seq）
  → 取附近窗口（`seq BETWEEN hit±N AND role IN ('user','assistant')`）→
  **工具调用/结果天然被过滤**（未进 text 列）；
- **记忆提取**：保留工具结果但**截断到要点**（前 N 字符）——工具结果有时是关键信息
  （失败教训来源）；回忆检索完全忽略工具；
- **架构落位**：`SessionRecall` 是 session 模块能力（共享 SqliteDb，派生数据），
  **不新增平行检索抽象**——"VFS 是唯一检索抽象"约束不破；
- **检索分工**：向量 = 知识/记忆/技能/画像（语义）；FTS5 = 会话回忆（关键词）。
  两者互补，唯一交汇点是 intent 路由；未来可 RRF 融合（VFS 已有 RRF 机制）。

### 7. 保留 / 移除清单

| 现有项 | 处理 |
|--------|------|
| `summary_generation`（5min） | **保留**，降频 6-8h，范围排除 Session |
| `memory_extraction`（10min） | **移除注册**，并入演化任务（智能体经消息索引提取） |
| `rule_extraction`（15min） | **移除注册**，提炼并入演化任务 |
| 会话末 `learn_skills_from_session`（GEPA/角色） | **移除**，改为轨迹持久化 hook（GEPA 数据层输入） |
| `RuleRecorder`（失败即时记录） | **保留**（实时、管线内、无 LLM） |
| ToolRegistry observability 轨迹采集 | **保留**，改为持久化到 SQLite |
| `garbage_collection` / `snapshot_gc` / `usage_stats_flush` / `reminder` | **保留**（工程维护/独立功能，不并入） |
| 会话 L0/L1 向量化 | **移除**（FTS5 回忆替代） |

### 8. 软删除与版本链（记账层，非判断门）

- 记忆/技能/角色/规则的删除一律**软删除**：条目移入 `_archive/`，保留 `lineage` 与 `version`
  （可回退，对 ADR-016 版本链）；
- 每次演化任务写演化报告（`memory/events/evolution_reports/{ts}.md`：增删改清单 + 统计），
  可观测性 + 回退依据；
- 组织形态（角色）退役尤其保守：长期会话中智能体稳定性优先，未必立刻删除。

### 9. 配置

新增 `[evolution]` 节：`enabled`（默认 true）、`cron`（默认 `0 0 */24 * * *`）、
`review_role`（默认 `evolution_reviewer`）、`mechanism`（`delegate` | `wake`，默认 delegate）、
`max_items_per_run`（默认 200）、`idle_episode_hours`（默认 24）。
摘要任务 cron 与记忆/提醒配置同模式（serde default fn + validate）。

---

## 后果

### 正面
- 加工任务收敛为一个：只有一次交给智能体自己的自我总结演化任务；
- 跨会话综合提炼层落地：去重/合并/冲突消解/退役/全局对照注册表；
- 会话回忆成本大幅下降（FTS5 零 embedding，省掉会话向量化）；
- "会话末"伪概念消除，片段模型对齐压缩点（ADR-012 刷新点）；
- 会话末零 LLM（GEPA 移到低频任务），日常对话延迟与 token 消耗下降。

### 负面 / 代价
- 技能/角色学习从"会话末即时"变成"每天一次"：新技能/角色当天不可见（config 可调频缓解）；
- 全量综述 token 成本集中在单次运行（统计工具 + 片段摘要 + `max_items_per_run` 控制）；
- 单点风险：演化任务失败则该周期无加工（水位线保证下周期续跑不丢数据）；
- 智能体判断非确定性：跨运行可能不一致——靠演化报告 + 版本链回退兜底；
- 消息索引与 JSONL 可能短暂漂移（append hook 失败）——索引可重建，无害。

## 边界条件（违反即重新评估）

- 综述导致注册表错删/劣化且回退未兜底 → 强化删除证据要求或改人工确认；
- 每天一次导致技能/角色时效性明显不足 → 拆出增量触发点（会话边界或低水位即时触发）；
- FTS5 回忆命中率不足（用户体感）→ 会话命名空间恢复向量化或 FTS5+向量 RRF 融合；
- `evolution` 单次运行超过成本上限 → 分片处理或回退多任务拆分。

## 与既有决策的关系

| 决策 | 关系 |
|------|------|
| ADR-001（VFS 双层摘要） | 摘要任务保留（降频、排除 Session）；会话回忆改 FTS5，不引入独立向量库 |
| ADR-005（SQLite 后端） | 消息索引/执行记录为派生数据，共享 SqliteDb；内容权威仍在 VFS |
| ADR-012（前缀快照） | 技能/角色更新仍在会话边界/启动/压缩点刷新——演化任务产物在免费刷新点自然可见 |
| ADR-013（唤醒轮） | 演化任务可选用 `process_wake` 机制（mechanism=wake） |
| ADR-016（角色化自演化） | 演化智能体经 `delegate_to_agent` 运行；角色产物落 `agent_role/`；本 ADR 补上综合提炼环节 |
| REJECTED #17（全事件驱动） | 不冲突：`SchedulerTaskTrigger` 的 `task:` 动作只是触发入口 |

## 后续演进（占位）

- 演化报告进 GUI（最近 N 次演化统计 + lineage 回退按钮）；
- 高风险删除走人工确认（复用 `executor/approval`）；
- 会话回忆 RRF 融合（FTS5 BM25 + 向量相似度）；
- 冲突消解结果沉淀为新记忆类别。

## 关键文件

- `core/src/scheduler/tasks/evolution_task.rs` — 新增：三阶段编排（采集/综述/记账提交）
- `core/src/scheduler/tasks/summary_task.rs` — 降频 + 排除 Session 命名空间
- `core/src/scheduler/tasks/{memory_task.rs,rule_task.rs}` — 移除注册（逻辑并入演化任务）
- `core/src/agent/coordinator.rs` + `agent_core.rs` — 移除会话末 `learn_skills_from_session`，改轨迹持久化 hook
- `core/src/agent/tool_registry/observability.rs` — 执行记录持久化到 SQLite（GEPA 数据层）
- `core/src/agent/roles.rs` — 新增内置角色 `evolution_reviewer`（只读 + 统计 + 回忆工具白名单）
- `core/src/session/search.rs` — 新增 `SessionRecall`（messages 表 + FTS5 索引 + 回忆查询）
- `core/src/session/manager.rs` — append hook 同步消息索引
- `core/src/context/retrieval/intent.rs` — Session 分支改走 FTS5 回忆
- `core/src/config/` — 新增 `[evolution]` 配置节
- `server/src/lib.rs` — 任务注册收敛（evolution + summary 降频）
- `AGENTS.md` — 补"派生索引模式"说明（内容权威在 VFS，索引/统计共享 SqliteDb）
