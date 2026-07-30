# Harness Engineering 核心精要

> **来源**: Mitchell Hashimoto、OpenAI 官方博客、Martin Fowler / Thoughtworks 分析、Claude Code 源码泄露分析等
> **整理日期**: 2026-05-05

---

## 一、根本问题：稀缺性倒置

LLM 让**代码生成**变得近乎无限、近乎免费。这一变化重新定义了软件工程的根本矛盾：它不再是"如何更快地产出代码"，而是"**如何以有限的人类注意力，审查和验证海量的 AI 产出**"。

传统软件工程中，写代码是瓶颈，审查代码相对充裕。LLM 时代倒置了这个天平——代码产出不再是瓶颈，人类注意力才是。

Harness Engineering 是对这个矛盾的**系统性回应**。它不提高人写代码的效率，它提高人**确保代码质量的效率**。

> 就像马具不削弱马的力量，而是把它引导到正确的方向——Harness 不限制智能体的生成能力，而是把它引导到可维护的、正确的结果。

---

## 二、核心机制：失败驱动的环境增强

2026 年 2 月 5 日，Mitchell Hashimoto（HashiCorp 创始人）在博客中首次命名了这个概念。六天后的 2 月 11 日，OpenAI 发表了同样的观点并提供了大规模实验数据。这种独立收敛本身就说明问题——足够多的人同时撞上了同一堵墙。

### 2.1 Hashimoto 的原始定义

> "每当智能体犯了一个错误，你就投入时间设计一个解决方案，**让它再也不会犯同样的错误**。"

不是去修代码。是去修**环境**。

### 2.2 两种手段

**隐性提示（Implicit Prompting）**

每次智能体失败后，在 `AGENTS.md` 或类似文件中追加一条新规则。智能体下次启动时自动加载这些规则。Hashimoto 的 Ghostty 项目中，`AGENTS.md` 的每一行都追溯到一次具体的智能体失败。这一机制接近**免疫系统**的逻辑：遇到 bug 行为 → 写入抗体 → 系统对同类问题免疫。

**程序化工具（Programmatic Tools）**

编写脚本和工具让智能体**自我验证**——截图检查 UI、筛选运行测试、验证 API 返回格式、对比 golden file。智能体被明确告知这些工具存在，并在合适时机主动调用它们。

### 2.3 核心公式

```
Agent = Model + Harness
```

- **Model（模型）**: 提供推理和生成能力，越来越商品化
- **Harness（Harness）**: 模型之外的一切——指令、上下文、工具、权限、约束、反馈循环、验证系统

> 技术类比：模型是 CPU，上下文窗口是 RAM，Harness 是操作系统，Agent 是应用程序。只选模型就像买电脑不装系统。

### 2.4 与 Vibe Coding 的本质区别

| | Vibe Coding | Harness Engineering |
|---|---|---|
| 认知姿态 | 被动：放弃理解，"感受正确" | 主动：理解对象从"代码"转为"约束系统" |
| 失败处理 | 反复 prompt 同一个问题 | 失败转化为环境增强，永不重复 |
| 适用范围 | 一次性原型和脚本 | 需要长期演进的生产系统 |
| 工程师角色 | Prompt 写手 | 环境设计师 |

> Vibe coding 是"放开缰绳，相信马知道方向"。Harness Engineering 是"紧握缰绳，但不再亲自跑"。

---

## 三、OpenAI 的验证实验

### 3.1 实验参数

| 指标 | 数值 |
|------|------|
| 持续时间 | 5 个月 |
| 代码总量 | ~100 万行 |
| 人工手写代码 | 0 行 |
| 合并 PR 数 | ~1,500 |
| 工程师人数 | 3 人（后增至 7 人） |
| 人均日产量 | 3.5 PR/人/天 |
| 时间节省 | 约 90%（为传统开发的 1/10 时间） |
| 约束条件 | **不允许人工修代码**——失败时必须增强 Harness |

### 3.2 三个关键发现

**发现一：打破 Brooks 定律**

传统软件工程中，增加人手会让项目更慢（沟通开销非线性增长）。但此实验中，团队从 3 人增至 7 人，吞吐量**不降反升**——因为人类工作从"写代码"转为"设计 Harness"，彼此独立增量。

**发现二：同一失败绝不发生两次**

OpenAI 团队的铁律：智能体失败时绝不人工修代码。必须回答："Harness 缺少什么能力，如何让智能体下次自主解决？"每次失败都转化为 Harness 的永久增强——新工具、新约束、新文档。Harness 越用越强。

**发现三：模型是商品，Harness 是护城河**

LangChain 的实验提供了量化证据：**同一模型，仅改 Harness，任务成功率从 52.8% 提升至 66.5%**（Terminal Bench 2.0 排名从第 33 跃升至第 5）。改进手段全部是 Harness 层面的：自验证循环、上下文工程优化、循环检测、推理策略调整。

---

## 四、Harness 的三大工程支柱

Martin Fowler 及 Birgitta Böckeler（Thoughtworks）将 Harness 归纳为三个类别：

### 4.1 Context Engineering（上下文工程）

**目标**: 确保智能体在正确时间获得正确信息。

**静态上下文**:
- 仓库本地文档（架构规范、API 契约、风格指南）
- `AGENTS.md` / `CLAUDE.md` 文件
- 交叉链接的设计文档（由 linter 验证链接有效性）

**动态上下文**:
- 可观测性数据（日志、指标、追踪）
- CI/CD 状态和测试结果
- 目录结构映射

**核心规则**: 从智能体视角，它无法访问的信息等于不存在。知识在 Slack、Google Docs、人脑中 = 对智能体不可见。仓库必须是唯一事实来源。

**AGENTS.md 的正确设计**:

OpenAI 团队起初写了庞大的 `AGENTS.md`，失败了。上下文窗口是稀缺资源，大文件挤占任务和代码的空间，"当一切都是重要的时候，什么都不是"。

正确做法：
- `AGENTS.md` 保持 ~100 行，作为**目录/地图**，而非百科全书
- 深层知识放在 `docs/` 目录的版本化文档中
- **渐进式披露**: 智能体从小入口开始，学会"去哪里找"
- 总大小限制在 32 KiB 以内

```
AGENTS.md          ← ~100 行，指针 only
docs/
├── design-docs/   ← 设计文档，含验证状态 + "核心信念"
├── exec-plans/    ← 执行计划，活跃 + 已完成 + 技术债务
├── product-specs/ ← 产品规范
└── references/    ← 参考资料
```

### 4.2 Architectural Constraints（架构约束）

**目标**: 不告诉智能体"写好代码"，而是**机械强制执行"好代码长什么样"**。

**分层架构**:

每个业务域分为固定层次，依赖只能单向流动：

```
Types → Config → Repo → Service → Runtime → UI
```

跨层关注点（auth、telemetry、feature flags）通过单一显式接口 **Providers** 进入。

**执行手段**:
1. **自定义 linter**（由 Codex 生成）
2. **结构测试**验证依赖方向
3. **CI 阻断**违反层边界的 PR

**关键洞察**: 约束是速度的乘数——一旦编码，到处同时生效。没有机械约束，坏模式会指数级复合，因为智能体会复制仓库中已存在的模式，**包括次优的**。

**Linter 错误信息的设计**:

不是简单报错，而是将修复指令注入智能体上下文：

```
错误: "core 模块依赖了 server 模块"
修复指引: "core 模块不允许依赖 server。请将共享类型提取到 types 模块，
          或改用依赖倒置模式。参考 docs/architecture/dependency-rules.md"
```

### 4.3 Garbage Collection（垃圾回收 / 熵减）

**目标**: 对抗系统熵增——文档漂移、规则违反、架构退化。

**机制**:
- 周期性运行的智能体扫描文档不一致性
- 发现架构约束违反
- 自动提出修复 PR
- 质量评分随时间追踪

**质量文档**:
- `docs/quality/domain-grades.md` — 各域质量等级
- `docs/quality/verification-status.md` — 验证状态
- `docs/quality/golden-principles.md` — 核心原则的机械规则

**运行策略**:
- 每次运行结束执行轻量 cleanup
- 每天至少一次深度清理扫描
- 清理结果必须回写质量文档

### 4.4 隐形的第四支柱：数据层

Harness Engineering 文献中反复警告一个容易被忽视的维度：**即使 Guides 和 Sensors 设计完美，Harness 仍可能在数据层面无声地失败**。

关键数据：
- 88% 的 AI 智能体项目从未抵达生产环境
- 27% 的智能体生产失败根因是**数据质量问题**，而非 Harness 架构或模型限制
- 65% 的企业智能体失败可追溯到**上下文漂移**
- 仅凭原始 schema 数据，智能体准确率 10-31%；配备受控上下文层后，准确率达 94-99%

三种典型的数据层失败模式：
1. **新鲜度衰减（Freshness Rot）**: RAG 上下文悄无声息地过期，智能体自信给出错误答案
2. **未认证源选择（Uncertified Source Selection）**: 智能体查询已废弃的数据表，Harness 无从预警
3. **Schema 漂移（Schema Drift）**: 字段定义在不同系统间分叉，产出"语法正确但事实错误"的结果

核心洞察：**Harness 的设计缺陷可以被 Guides 和 Sensors 捕获，但数据层的错误往往是无声的——没有异常，只有错误的答案。**

---

## 五、控制论框架：Guides 与 Sensors

Birgitta Böckeler 引入了控制论框架，将 Harness 的控制机制分为两类：

### 5.1 Guides（前馈控制）

在生成**之前**引导，提高首次尝试成功率。

| 类型 | 示例 |
|------|------|
| 项目级指令 | `CLAUDE.md` / `AGENTS.md` |
| 规范文档 | 结构化需求，定义"正确"是什么 |
| 架构约束 | 依赖分层规则 |
| 技能定义 | 窄域、按需的特定任务工作流 |

### 5.2 Sensors（反馈控制）

在智能体行动**之后**观察，启用自我纠正。

| 类型 | 示例 | 速度 |
|------|------|------|
| Linter / 类型检查器 | 确定性快速验证 | 毫秒级 |
| 测试套件 | 行为验证 | 秒级 |
| CI/CD 流水线 | 集成级验证 | 分钟级 |
| LLM-as-judge | 语义分析（确定性工具无法捕获的问题） | 秒级 |

### 5.3 关键洞察

- **仅有前馈** → 智能体僵化执行规则，无法适应意外
- **仅有反馈** → 智能体反复试错，效率低下
- **前馈 + 反馈** → 可靠且灵活的系统

---

## 六、核心设计原则

### 原则 1：失败即信号（Failure as Signal）

> 智能体失败时，不是修代码的问题，是 Harness 缺能力的问题。

这是 Harness Engineering 的根基原则。每次失败问："Harness 缺少什么，让智能体下次能自主解决？"人工修代码是一种资源浪费——它不会让下一次变得更好。

### 原则 2：环境可读性（Environment Legibility）

> 智能体只能和它能检索到的东西一样正确。

结构化日志、可追踪指标、可自动化验证、清晰的入口文档——这些不是运维部门的任务，而是智能体的"感官系统"。

### 原则 3：约束即乘数（Constraints as Multipliers）

> 约束不是限制速度，而是允许速度而不发生衰减。

规则一旦编码为机械检查，就**在每处代码同时生效**。约束粒度聚焦不变量（边界、依赖方向、可复现性），不微观规定具体实现手法。

### 原则 4：渐进式披露（Progressive Disclosure）

> 上下文窗口是稀缺资源。一次给所有信息不是帮助，是噪音。

`AGENTS.md` 是地图，不是百科全书。知识分层：入口 → 概览 → 细节。任务特定信息通过指针引用，不内联。

---

## 七、常见反模式

| 反模式 | 症状 | 正确做法 |
|--------|------|----------|
| 巨大 AGENTS.md | 指令 2000+ 行，智能体要么忽略要么过拟合 | ~100 行目录式，指针到深层文档 |
| 单次 PR 包含所有变更 | 每次运行试图完成整项任务 | 拆分短周期 PR，单次聚焦一个变更 |
| 无隔离的任务混行 | 智能体交叉污染多个功能 | Git worktree 隔离，每 PR 一个独立环境 |
| 文档 vs 代码的不一致 | 文档描述的架构和实际代码分叉 | 自定义 linter + 结构测试强制执行 |
| 忽略数据层的失效 | 丢弃正确的答案因为上下文已过期 | 周期性验证上下文新鲜度，治理数据血缘 |

---

## 八、采纳路径：Hashimoto 的六步

Mitchell Hashimoto 以自身经历总结了从 AI 怀疑论者到 Harness Engineering 践行者的六个阶段：

| 步骤 | 内容 | 核心收获 |
|------|------|----------|
| 1 | 放弃聊天机器人 | Agent（能读文件+执行命令+调用 API）是唯一有效形态 |
| 2 | 人工 + Agent 双重作业 | 先手工写，再逼 Agent 产出同等质量——不惜时间，建立手感 |
| 3 | 收工前 30 分钟派发 Agent | 找到 Agent 真正擅长的任务类型：深度调研、并行试错、机械重构 |
| 4 | 外包必做题 | 把确定性高、目标清晰的小任务完全交给 Agent |
| 5 | 工程化 Harness | 每次失败转化为 AGENTS.md 规则或程序化工具——**本阶段开始** |
| 6 | 永葆 Agent 运行 | Agent 作为后台常驻进程，全天候接收任务——**目标状态** |

> Hashimoto 坦诚：他目前只在每天 10-20% 的时间保持 Agent 后台运行。Step 6 仍然是理想目标。

---

## 九、Claude Code 泄露：生产级 Harness 的实证

2026 年 3 月 31 日，Claude Code 的 51.2 万行 TypeScript 源码因打包失误泄露。这提供了迄今为止最完整的生产级 Harness 内部结构。

### 9.1 规模数据

| 组件 | 规模 |
|------|------|
| 总代码量 | 512,000 行 / 1,906 文件 |
| 查询引擎 | 46,000 行 |
| 工具定义库 | 29,000 行 |
| 内置权限门控工具 | 19 个 |
| 未发布功能标志 | 44 个 |
| MCP 传输类型 | 6 种 |

### 9.2 三层记忆架构

Claude Code 采用**索引式记忆**而非全量存储：

- **短期上下文**: 当前会话的活跃信息
- **会话级持久化**: `MEMORY.md` 索引文件（ID + 时间戳 + 嵌入指针），指向分布式主题文件
- **长期记忆**: 跨会话的持久化知识

**严格写入纪律**: 记忆更新先作为候选，验证成功后才提交——失败的工具调用不污染持久状态。

### 9.3 上下文熵管理

Claude Code 内部包含一套精密系统，持续决策什么留在上下文、什么被驱逐。这是区分可用智能体和玩具演示的**核心难题**。

### 9.4 KAIROS（未发布）

源码中引用 150+ 次的自主守护进程模式：Claude 作为持久后台智能体运行，响应文件系统变更或 CI 信号，无需人工提示。这对应 Hashimoto 六步中的 Step 6。

---

## 十、演进层次

Harness Engineering 不是凭空出现的。它是智能体开发方法的第三层递进：

| 层次 | 时间 | 关注点 | 代表 |
|------|------|--------|------|
| Prompt Engineering | 2022-2024 | 单轮指令质量 | Few-shot、CoT |
| Context Engineering | 2025 | 会话级信息流 | RAG、向量检索 |
| **Harness Engineering** | **2026+** | **系统级环境设计** | **Codex、Claude Code** |

三者是**递进关系**，不是替代关系：Prompt 塑造单个请求，Context 管理多轮交互，Harness 设计让智能体自主运行数小时的完整系统。

---

## 十一、关键参考资料

1. **Mitchell Hashimoto** — "My AI Adoption Journey" (2026-02-05)
   - 来源: mitchellh.com/writing/my-ai-adoption-journey
   - 核心: "Engineer the harness" 起源，六步采纳路径

2. **OpenAI 官方博客** — "Harness engineering" (2026-02-11)
   - 来源: openai.com/index/harness-engineering
   - 核心: 100 万行代码实验的原始报告

3. **Martin Fowler / Thoughtworks** — "Harness Engineering" (2026-02-17)
   - 来源: martinfowler.com/articles/exploring-gen-ai/harness-engineering.html
   - 核心: Context + Constraints + Garbage Collection 三大分类

4. **Claude Code 泄露分析** (2026-04)
   - 来源: 社区多份深度分析
   - 核心: 51.2 万行代码揭示的生产级 Harness 架构

5. **Harness.io 博客** — "The Agent Loop Is the New OS" (2026-04-23)
   - 来源: harness.io/blog/agent-loop-new-os
   - 核心: MCP Server 设计哲学，操作系统类比

6. **Octopus Deploy 博客** — "Harness engineering" (2026)
   - 来源: octopus.com/devops/continuous-delivery/harness-engineering/
   - 核心: Vibe Coding vs Harness Engineering 对比，人类角色的重新定义

---

## 十二、最小可行 Harness 检查清单

- [ ] `AGENTS.md`（~100 行，目录式，指针到深层文档）
- [ ] 结构化 `docs/` 目录（设计文档、执行计划、产品规范）
- [ ] 可复现开发环境（一键启动）
- [ ] 机械执行的架构约束（自定义 linter + 结构测试 + CI 阻断）
- [ ] 智能体可读的可观测性（结构化日志 + 查询能力）
- [ ] 自动化验证门控（测试、lint、类型检查）
- [ ] 周期性 Garbage Collection（文档漂移检测、架构退化扫描）
- [ ] 安全护栏（最小权限、沙箱执行、审计日志）

---

## 十三、天演中的 Harness 实现映射

天演项目中，Harness Engineering 的核心概念已实现为多个松散耦合的模块。以下是理论到代码的映射关系。

### 13.1 组件映射

| 理论概念 | 天演实现 | 位置 | 状态 |
|---------|---------|------|:---:|
| 隐性提示（失败 → 规则） | `RuleRecorder` | `core/src/scheduler/tasks/rule_recorder.rs` | ✅ 已实现 |
| 记忆聚类 → 规则提炼 | `RuleSuggester` | `core/src/scheduler/tasks/rule_suggester.rs` | ✅ 已实现 |
| 规则定时调度 | `RuleTask` | `core/src/scheduler/tasks/rule_task.rs` | ✅ 已实现 |
| 可观测性（Metrics） | `AgentMetrics` | `core/src/observability/mod.rs` | ✅ 已实现 |
| 上下文工程 | `ContextPipeline` | `core/src/context/pipeline.rs` | ✅ 已实现 |
| Sensors（反馈控制） | `VerificationGate` + `LlmJudge` | `core/src/executor/` | ✅ 已实现 |
| 技能定义 + 学习 | `skills/` (含 GEPA 引擎) | `core/src/skills/` | ✅ 已实现 |
| GC / 熵减 | `GcTask` | `core/src/scheduler/tasks/gc_task.rs` | ✅ 已实现 |
| 架构约束 | 待实现（linter + 结构测试） | — | ❌ 未实现 |

> **注意**：`AgentHarness` 和 `AgentSkills` wrapper 结构体已删除，功能由 `Agent` 直接持有。规则记录不再通过 Agent 的即时路径，而是通过 Scheduler 的 `RuleTask`。详见 [module-map.md](../architecture/module-map.md) 中的"已删除/废弃组件"。

### 13.2 规则管线架构

规则提炼链路已从 Agent 的同步后台任务重构为 Scheduler 的 cron 定时任务：

```
TaskScheduler (每 15 分钟)
  └── RuleTask (TaskHandler)
        └── RuleSuggester
              ├── scan()  → 扫描 patterns/ + failed_tasks/
              └── promote_to_rule()  → LLM 聚类 → RuleRecorder
                                           └── record_with_kind()  → VFS agent/learned/
```

数据源：`MemoryTask`（定时）→ `MemoryExtractor`(LLM) → 分类记忆写入 VFS：
- `MemoryCategory::Pattern` → `agent/patterns/`
- `MemoryCategory::FailedCase` → `memory/cases/failed_tasks/`

### 13.3 可观测性（原 AgentHarness 已移除）

`AgentHarness` 和 `AgentSkills` 包装结构体已完全移除，功能直接由 `Agent` 持有。可观测性指标由 `AgentMetrics` 直接管理：

```
Agent {
    // ...
    metrics: Arc<AgentMetrics>,
}
```

- `metrics.record_execution(success)` — 记录每次 AgentLoop 执行结果
- `metrics.record_pipeline_failure()` — 记录管线失败
- `metrics.record_rule_hit()` / `metrics.record_rule_injection()` — 追踪规则有效性

### 13.4 失败驱动增强闭环

```
                        ┌───────────────────────┐
1. 执行失败              │ AgentLoop 失败 + MemoryTask 提取记忆 │
                        └───────────┬───────────┘
                                    │
2. MemoryTask (scheduler)           ▼
   MemoryCategory::                 ┌───────────────────┐
   FailedCase / Pattern             │ 写入 VFS           │
                                    │ patterns/         │
                                    │ failed_tasks/     │
                                    └─────────┬─────────┘
                                              │
3. RuleTask (scheduler, 每15分钟)             ▼
   RuleSuggester.scan()           ┌───────────────────┐
   → LLM 聚类                     │ 跨会话模式识别      │
   → RuleRecorder.record_with_kind│ 去重 + 写入        │
                                    │ agent/learned/     │
                                    └─────────┬─────────┘
                                              │
4. ContextPipeline.run()                     ▼
   (下次对话时)                   ┌───────────────────┐
   → load_relevant_rules()       │ 规则注入 prompt     │
                                    └─────────┬─────────┘
                                              │
5. AgentMetrics                              ▼
   record_rule_injection()       ┌───────────────────┐
   record_rule_hit()             │ 追踪规则有效性      │
                                    └───────────────────┘
```

### 13.5 FailureKind 类型

```rust
pub enum FailureKind {
    Transient,  // 瞬态错误，不记录
    Logic,      // 逻辑错误，记录为 learned rule
    Input,      // 用户输入错误，不记录
    System,     // 系统级错误，记录并告警
}
```

### 13.6 与理论框架的对应

| 理论框架 | 天演等价实现 | 说明 |
|---------|------------|------|
| Guides（前馈） | `ContextPipeline`（检索 + 规则注入 + 压缩） | 在生成前提供正确信息 |
| Sensors（反馈） | `VerificationGate` + `LlmJudge` + `AgentMetrics` | 生成后验证与记录 |
| 失败即信号 | `RuleRecorder.record_with_kind()` | 失败转化为永久环境增强 |
| 约束即乘数 | `VerificationGate` + 审批工作流 | 安全级别自动审批/拒绝 |
| 渐进式披露 | `ContextPipeline.run()` + `ContextCompressor` | Token 预算 + 分层压缩 |
| 垃圾回收 | `GcTask` (cron 定时) | 周期清理 + 文档漂移检测 |

---

**文档版本**: 2026-05-30
**最后更新**: 2026-05-30（规则管线重构：RuleRecorder/RuleSuggester 移至 scheduler/tasks/，新增 RuleTask，AgentHarness 精简，FailureKind 类型更新）
