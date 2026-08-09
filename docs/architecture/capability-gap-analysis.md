# 综合智能体能力差距评估（2026-08 对标）

> 状态：**调研完成**（2026-08-08，4 路并行外部调研，~120 个一手来源）
> 日期：2026-08-08
> 范围：天演 vs 2026 年最先进智能体产品的能力对标，识别"综合性智能体"差距
> 基线：代码事实（本仓库当前实现）+ 外部调研（官方文档/公告/一线媒体）
> 竞品视角补充：[competitive-landscape.md](./competitive-landscape.md)（2026-08-09 双路调研：9 代码 agent + 16 本地助手/记忆产品；推荐路线 T1 事件驱动/主动提醒/剪贴板/桌面集成）

## 1. 背景与目标

天演已完成感知与评估能力补全（浏览器 MCP 截图链路、对话图片输入、子 agent 编排增强、回答质量 eval），本报告对标 2026-08 时点业界最先进产品（Claude Code、OpenAI Codex/ChatGPT Work、Cursor、Gemini Spark、Microsoft Copilot、Manus、OpenClaw、Perplexity Comet 等），确定距离"综合性智能体"的剩余差距。

调研方法：4 路并行（①主流 agent CLI 能力矩阵 ②通用智能体平台 ③多 agent 协作与编排 ④agent 工程基础设施），以官方文档为主、一线媒体与社区深度评测为辅；第三方数字（OSWorld/WebArena 分数、Manus 并发数）已标注可信度。

## 2. 天演现状基线（2026-08，代码事实）

| 维度 | 能力 |
|------|------|
| 工具层 | 21 内置（文件/代码/知识/VFS/测试/LSP/委托/技能）+ MCP 动态工具桥接（stdio，截图落盘） |
| 感知层 | 对话图片输入（全链路）✅、浏览器感知（MCP playwright 截图）✅、纯 HTTP 抓取（http_request 技能，SSRF 防护） |
| 编排层 | 子 agent 委托（同轮并行 + 嵌套深度 3 + max_turns/timeout）✅、AgentLoop 单循环 |
| 记忆/知识 | VFS L0/L1/L2 双层摘要索引、记忆提取、GEPA 技能进化、会话压缩 |
| 评测 | `core/src/eval` 评分式离线评测（黄金用例）、verify_build LLM-as-Judge 门控 |
| 可观测性 | AgentMetrics（内存态）、UsageStats（SQLite）、检索轨迹 FIFO |
| 状态管理 | JSONL 会话持久化、消息级快照回退/重做、取消/优雅关停 |
| 界面 | Tauri 桌面 + 聊天 UI + 编程工作台（只读+编辑）+ 管理面板 |
| 定时 | scheduler（规则/记忆/摘要/GC） |

## 3. 业界 2026 公认能力清单（12 项，调研结论）

1. 屏幕级 computer use（截图感知 + 鼠标键盘操作）
2. 云托管 + 设备离线长任务
3. 跨设备委派与会话恢复
4. 持久、可审阅、自动更新的记忆
5. 并行子 agent 扇出（Manus Wide Research 20-100+）
6. 定时/触发/监控型任务
7. MCP + 连接器 + 插件/技能市场
8. 语音输入输出（agent 级）
9. 安全四件套：审批/权限分层/沙箱/prompt-injection 防护
10. 评测与可观测性闭环（trace → eval → 优化）
11. agent 身份与治理（企业向）
12. 主动性与监控上报（heartbeat/监控型任务）

天演当前覆盖：4 的部分（VFS 记忆/记忆提取）、6 的部分（scheduler 周期型）、7 的部分（MCP 桥接）——约 **4/12**。

## 4. 差距清单（按优先级）

### P0 — 全行业标配，天演完全缺失

| 缺口 | 现状 | 业界标准（2026） | 参照 |
|------|------|-----------------|------|
| Web 搜索 | ❌ 仅 http_request 裸抓取 + MCP 浏览器 | 7/7 产品内置 web search + fetch；Codex cached 索引模式（防注入）是新范式 | Codex、Claude Code、Cursor |
| 语音 I/O | ❌ 无 | 全一线产品有 agent 级语音（语音启停任务/屏幕感知） | ChatGPT Work、Copilot Voice Live、Comet |
| 后台长任务 + 任务列表 | ❌ 仅周期型 scheduler | `/tasks`+`/background`、后台子 agent、云会话离线续跑 | Claude Code、Cursor、Copilot |
| 结果聚合工具 | ⚠️ 多 delegate 并行，聚合靠 LLM 手写汇总 | join/fan-in 节点、manager 合成、交叉校验 | AutoGen GraphFlow、Claude workflows |
| 用户反馈采集 | ❌ 无 👍/👎 | Annotation Queues / User Feedback API——最低成本质量信号 | Langfuse、LangSmith |

### P1 — 明显差距，工程价值高

| 缺口 | 现状 | 业界标准 | 参照 |
|------|------|---------|------|
| Checkpoint 升级 | ✅ **已由现状覆盖**（2026-08-09 核实） | 每轮自动快照已存在：`capture_workspace_snapshot` 在 process_message/process_message_stream 每条消息处理前自动捕获（ADR-006/008：gzip + GC + similar diff）；恢复链路：消息回退/重做 API（`/messages/delete` + `/messages/redo`）+ 前端按钮。与业界剩余差距仅"分选回滚（对话/代码分开恢复）"与"快照时间线视图"，单用户场景价值低，不追 |
| 防失控硬限制 | ❌ **已决策不做**（2026-08-09） | 本地单用户场景：嵌套深度上限 3 + 后台并发上限 4 已构成足够边界；任务级 `max_turns`/`timeout_secs` 由 LLM 自主设置；终止条件由用户/会话自然边界承担。**不再评估该缺口** |
| HITL 审批门 | ⚠️ 有审批工作流但未挂接委托/敏感工具 pause-resume | `needs_approval` + RunState 序列化跨进程等待 | OpenAI SDK、CrewAI @human_feedback |
| 跨 agent 通信 | ❌ 无 | agent teams 共享任务清单+消息；文件信道；共享 state（共识：多数场景不需要） | Claude Code、Cursor |
| 进度可视化 | ❌ 无 | `/tasks`、Agents Window、agents panel | Cursor、Copilot |
| 结构化 Trace | ❌ 内存态指标，无 span 树 | Run/Trace/Thread 四层模型 + OTel GenAI 语义约定 | LangSmith、Langfuse |

### P2 — 差异化能力，按需补

| 缺口 | 说明 | 参照 |
|------|------|------|
| 屏幕级感知 | 浏览器 MCP 覆盖不了无 API 桌面应用；需权限分层 + PI 扫描基线 | Claude/OpenAI Computer Use、Nova Act |
| 插件/技能市场 | 有技能+GEPA，缺可分发插件包+目录（OpenClaw 32.6K MCP/10.7K 技能为形态标杆） | OpenClaw ClawHub、Cursor marketplace |
| 跨应用连接器 | 行业原则"连接器优先于屏幕操作"；天演仅 MCP | Claude Connectors、Gemini Connected Apps |
| CI 评测闭环 | eval 模块已有，缺 CI Quality Gate + 在线评估 + Dataset 管理 | Promptfoo GitHub Action、Langfuse |
| 任务状态持久化 | 无 durable work ledger/heartbeat（长任务场景才需要） | Temporal、LangGraph durable execution |

### ✅ 已与业界平齐（非缺口）

- 嵌套委托深度 3（与 Claude Code 默认一致）
- 同轮多 delegate 并行（JoinSet 与 asyncio.gather 同构）
- MCP 桥接（+ ACP 可作加分项）
- 图片输入全链路
- 评分式回答 eval（四维度，对标 Cursor evals SDK 方向）

## 5. 行业共识要点（决策参考）

1. **"综合智能体"能力天花板仍然很低**：OSWorld 2.0（108 长时程工作流）最强模型仅 20.6%——长任务保持、中途信息处理、主动询问是全行业痛点，天演无劣势
2. **先单 agent 再上多 agent**（Anthropic/LangChain 共识）：优先补工具和技能（web 搜索、连接器），而非更多 agent 架构
3. **能代码固化的并行别交给 LLM**：workflow（parallelization/orchestrator-workers）与 agent 是两回事
4. **云托管长任务是 2026 主线**（"关电脑继续跑"）：本地桌面天演应做**本地后台任务**（关应用恢复）而非云端
5. **安全基线**（权限分层 + PI 扫描）是桌面 agent 准入门槛，做屏幕级感知前必须补
6. **多 agent 的触发条件**：上下文爆炸 / 可并行独立子任务（读多写少）/ 信息超出单窗口 + 多复杂工具 / 组织原因（第三方 agent）；写密集任务与顺序依赖链应保持单 agent（LangChain benchmark：单 agent 在 1 个干扰域时仍最优）

## 6. 建议路线图

```
Phase 1（P0）
  Web 搜索工具（cached 索引模式防注入的本地等价：搜索摘要 + SSRF 防护 + 缓存）
  → 用户反馈采集（👍/👎 入库）
Phase 2（P1）
  后台任务（delegate fire-and-forget + 任务列表 + VFS 状态暴露 + GUI）
  → 结果聚合工具（collect_delegations / 交叉校验）
  → checkpoint 升级（每轮自动 + 分选回滚）
Phase 3（P2）
  语音 I/O → 屏幕级感知（先浏览器内，再桌面，权限分层先行）
  → 插件分发（目录式）→ CI 评测闭环
```

## 7. 实施记录

> 2026-08-08 起按路线图推进。

| 项 | 状态 | 实现 |
|----|------|------|
| Web 搜索工具（Phase 1） | ✅ 已实施 | `core/src/executor/web.rs`：web_search（DuckDuckGo/SearXNG 双后端）+ web_fetch（可读正文提取）+ SSRF 防护 + TTL 缓存；`[web]` 配置节；25 个内置工具 |
| 后台长任务 + 结果聚合（Phase 2） | ✅ 已实施 | `core/src/agent/background.rs`：delegate(background) fire-and-forget + 任务注册表（状态机/并发上限 4）+ 完成通知注入父会话（结果摘要 + 剩余计数 join 信号）；task_status/task_cancel 工具 + GET /api/v1/tasks |
| 结果聚合工具（P1） | ✅ 已消除（设计替代） | 2026-08-08 审查：事件驱动 + LLM 聚合完整覆盖——完成通知携带**完整结果** + join 信号（剩余计数/汇总指令）；`task_status` 返回完整任务（含 result，serde 全量）作兜底；前台并行委托同轮直接返回聚合。业界代码级 fan-in（collect_delegations）面向 20-100 扇出规模，天演并发上限 4 + LLM 合成质量更高，无需工具级实现 |
| 注入上下文快照持久化 + 会话边界/压缩点技能刷新 | ✅ 已实施 | JSONL 首行 SessionHeader 固化 soul/rules/memories 快照（重启后旧会话零漂移）；新会话边界增量注册 GEPA 技能（`SkillManager::refresh_registry` 幂等）；压缩点（自动/手动 `POST /sessions/{id}/compress`）清空快照 + 刷新注册表（会话内唯一免费刷新点） |

## 附录 A：调研来源（代表）

- 主流 CLI 矩阵：code.claude.com/docs、developers.openai.com/codex、cursor.com/docs、geminicli.com/docs、docs.github.com/en/copilot、aider.chat/docs、opencode.ai/docs
- 通用平台：help.openai.com、claude.com/blog、blog.google、microsoft-365/blog、manus.im、openclaw.ai、perplexity.ai
- 多 agent：openai.github.io/openai-agents-python、microsoft.github.io/autogen、docs.crewai.com、langchain-ai.github.io/langgraph、anthropic.com/research/building-effective-agents、langchain.com/blog（multi-agent 系列）
- 工程基础设施：docs.langchain.com/langsmith、langfuse.com/docs、promptfoo.dev/docs、github.com/mem0ai/mem0、docs.zep.ai、docs.letta.com、temporal.io、developers.cloudflare.com/agents、mastra.ai/blog、github.com/open-telemetry/semantic-conventions-genai
- 基准：OSWorld 2.0（arXiv 2606.29537）、WebArena、GAIA、SWE-Bench Verified、BrowseComp、METR HCAST、decodethefuture.org 汇总
- 一线媒体：Reuters（Meta-Manus）、AP、The Verge（Mariner 关闭）、Wired
