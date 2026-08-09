# 竞品对标与差距分析（2026-08）

> 日期：2026-08-09
> 方法：2 路并行外部调研（代码 agent 9 产品 + 本地助手/记忆 16 产品，官方文档/GitHub/一线评测）+ 代码事实核对
> 定位过滤器：**本地优先、单用户、综合智能体**（聊天 + 代码 + 知识 + 记忆 + 技能 + 自动化一体）
> 衔接：[capability-gap-analysis.md](./capability-gap-analysis.md)（能力缺口 P0/P1/P2 表）——本报告补充竞品视角，二者互为补充

---

## 1. 竞品全景（2026-08 时点）

### 1.1 代码 agent 层（已高度收敛）

| 产品 | 差异化能力 | 天演对应项 |
|------|-----------|-----------|
| Claude Code | 文件级 checkpoint（代码/对话分恢复）、并行后台 subagents、hooks（生命周期钩子）、5 档权限模式、auto-compaction、Agent Teams | 快照 ✓ 半覆盖；子任务 ✓；审批 ✓ 无规则引擎 |
| Codex CLI | **OS 级沙箱**（Seatbelt/bwrap+seccomp）、默认断网 + 域名 allowlist、命令前缀策略（allow/prompt/forbid）、auto-review 审批代理、云端任务 | 无沙箱；无网络策略；审批无规则 |
| Gemini CLI | **Plan mode 默认开启**（只读 + ask_user 澄清 + 只读 MCP）、subagents markdown 定义、四层记忆、1M 上下文 + PTY | 无 Plan mode；ask_user ✓ |
| Cursor | **/multitask 自动分解 + git worktree 隔离**、Cloud Agents（VM 并行 + artifacts）、每模型定制 harness、程序化 lifecycle hooks | 无自动分解；无 worktree |
| Cline | **Plan/Act 硬门控**（代码级阻断）、shadow git checkpoint（每 tool call）、MCP 自举（agent 自建 MCP server）、命令安全分类（强制询问） | 无 Plan 门控；快照 ✓ 无对话回退 |
| Aider | **repo map（tree-sitter 符号图按 token 预算注入）**、双模型 architect/editor、自研评测 leaderboard | LSP 符号 ✓ 半覆盖；eval ✓ 无外部基准 |
| OpenHands | **Agent Canvas（ACP 驱动多 agent 控制中心）**、两层持久记忆、Critic/Stuck Detector、评估 harness | 无 ACP；双层摘要 ✓ |
| Devin（云） | 并行 agent 舰队、PR 反馈闭环、从历史轨迹学习、可微调 | 云形态，过滤 |

**关键结构性变化**：Windsurf → Devin Desktop（Cascade + Devin Local 双 agent）；本地 agent 安全模型从"审批"升级为"**审批 + OS 沙箱 + 网络过滤**"三件套（Codex 最彻底，Claude Code 次之）。

### 1.2 本地助手/记忆层（四层分化）

| 层 | 代表 | 特征 | 天演位置 |
|----|------|------|---------|
| 推理基座 | Ollama / LM Studio / LocalAI | 无记忆、无 UI、无主动 | 天演通过 config 接入 ✓ |
| 记忆基础设施 | Mem0 / Zep-Graphiti / Letta | 有记忆无产品；时序图谱 / 事实提取 / sleep-time 整理 | **天演已覆盖第二层并更强**（VFS L0/L1 + MemoryExtractor + MemoryTask；双摘要检索远超 Mem0 单向量） |
| RAG/对话产品 | AnythingLLM / Jan / Khoj | 有 UI 记忆浅；cron 调度 | 天演覆盖第三层（GUI + 知识库 + 调度） |
| 系统级/主动型 | Recall / Rewind（已死）/ Dot / HA / n8n | 捕获形态 / 主动提醒 / 事件驱动 | **天演缺第四层——本报告核心差距区** |

**行业教训（Rewind 关停）**："完美记忆"存在公司开关——本地优先 + 数据可导出是正当性论据；天演本地定位在此维度天然正确。

### 1.3 桌面 agent 新品类（2026-04 形成）

Dispatch（VM 沙箱 + 默认拒网 + 文件夹授权，但不支持后台）、Manus（无沙箱）、Perplexity Computer（后台 daemon + cron + 多模型路由 + 多日持久化）、OpenClaw（DIY 13700+ skills）。**无一产品在安全/后台/模型灵活/易用四轴全胜**——品类未定形，天演无被锁死风险。

---

## 2. 差距矩阵（维度 × 天演现状 × 业界做法 × 判定）

判定标准：✅ 已覆盖（天演不劣）｜◐ 半覆盖（有基础缺形态）｜**T1/T2/T3** 真差距（按推荐优先级）｜🚫 定位过滤（明确不做）

| 能力维度 | 天演现状（代码事实） | 业界标杆做法 | 判定 |
|---------|-------------------|-------------|------|
| 本地推理 | Ollama 集成（test/scan/add-model） | Cline/Aider 同款；主流商业产品反而不支持 | ✅ 差异化 |
| 双层摘要 RAG | VFS L0/L1/L2 + RRF 融合 | 无同构（OpenHands 两层记忆最接近） | ✅ 领先 |
| 记忆提取/整理 | MemoryExtractor + MemoryTask | Letta sleep-time agent（同构） | ✅ |
| 子任务编排 | delegate 并行 + 嵌套 3 层 + 后台 + 取消 | Claude subagents / Cursor multitask | ✅（嵌套深度与后台等同业水准） |
| 快照回退 | 工作区快照（gzip+GC+similar diff） | Cline shadow git / Claude checkpoint | ✅ |
| 会话压缩 | 自动 + 手动 + 压缩点刷新 | Claude auto-compaction | ✅ |
| 定时任务 | RuleTask/MemoryTask/SummaryTask/GcTask | AnythingLLM cron / Perplexity cron | ✅ |
| 浏览器感知 | MCP Playwright 截图（只读） | Cline Puppeteer 点击/输入；computer use | ◐ 后置（屏幕感知后置项） |
| 代码结构感知 | LSP（symbols/跳转/诊断） | Aider repo map（tree-sitter 图） | ◐ 半覆盖（LSP 更准但无 token 预算自适应注入） |
| **事件驱动触发** | 仅定时（scheduler） | n8n 文件监听/webhook；Cline connectors；HA 事件触发器 | **T1** |
| **主动提醒** | 无推送通道（记忆只被动检索） | Dot alerts（relevant-now）/ Vellum 自检 / LocalGPT heartbeat | **T1**（行业最薄点之一） |
| **剪贴板 I/O** | 无 | **全行业几乎无人做** | **T1**（差异化空白） |
| **桌面集成补全** | 托盘 ✓（build_tray） | 全局快捷键（Rewind/OpenClaw）、系统通知（Alpaka D-Bus / HA announce） | **T1**（补快捷键 + 通知） |
| **Plan 模式** | 无（planner 已删，无只读门控） | Gemini 默认开 / Cline Plan-Act 硬门控 / Devin megaplan 持久化计划文件 | **T2** |
| **命令级审批策略** | 全局风险配置（无规则引擎） | Codex 命令前缀 allow/prompt/forbid；Devin glob 三档规则 | **T2** |
| **审批前命令编辑** | 无（只能批准/拒绝） | Devin Local 审批卡编辑命令 + wand 改写 | **T2** 子项 |
| 对话回退 | 消息重做 ✓，无"回退到指定 prompt" | Claude Code rewind（代码+对话） | 维持现状（Checkpoint 决策已收敛，不追） |
| worktree 隔离并行 | 无（后台任务同工作区） | Cursor /multitask + worktree；Cline worktree 会话 | **T3** 可选 |
| 大任务自动分解 | 手动 delegate | Cursor /multitask 自动拆分 | **T3** 可选 |
| 外部基准评测 | LLM-as-Judge 四维（离线） | Aider leaderboard / OpenHands SWE-bench harness | ◐ 后置（与 CI 评测闭环一并） |
| 执行沙箱 | 无（裸命令） | Codex OS 级 / Claude SandboxSettings / Devin Local | 🚫 过滤（见 §4） |
| 网络访问策略 | 无 | Codex 默认断网 + 域名 allowlist；Devin deny/ask/allow | 🚫 过滤（本地信任模型；web 工具受审批管控） |
| 云任务/多设备 | 无（本地定位） | Codex cloud / Cursor Cloud Agents / Devin fleet | 🚫 定位违背 |
| 多用户/企业治理 | 无（单用户） | SAML/RBAC/审计（全员企业向） | 🚫 定位违背 |
| ACP 多 agent 控制中心 | 无 | OpenHands Agent Canvas / Claude Agent Teams | 🚫 过滤（不依赖外部 agent 产品；子任务已覆盖内部编排） |
| 模型 harness 定制 | 单 harness | Cursor 每模型调优 | 🚫 过滤（模型自动路由已决策不做） |
| 插件市场 | 无（技能系统内部） | 全员 marketplace | 🚫 已决策不做 |

---

## 3. 推荐路线（按天演定位过滤后的真差距）

### T1 —— 强烈推荐（契合"综合智能体"目标 + 行业最薄 / 空白）

0. **统一消息通知与唤醒原语**（基建，[ADR-013](../architecture/decisions/013-unified-message-notification-wake.md)）：
   - 后台任务"全部完成/失败"自动触发主 agent 新一轮（shouldReply 语义，join 信号 `remaining==0` 已有）——当前通知只入库不触发，用户须手动再发消息（对照 Codex 反例：永不醒；omo 实证：shouldReply = allComplete || failure）
   - **前置：任务状态持久化**（SQLite）——任务实体化，重启可恢复，唤醒计数正确
   - 统一消息队列 + 唤醒语义是下述 1/2/3 的共同底座
1. **事件驱动触发**：scheduler 从"仅定时"扩展为"定时 + 事件"。
   - 文件监听（fs watcher）：工作区/知识目录变化 → 触发规则任务（n8n Local File Trigger 同构）
   - 外部触发 API：`POST /api/v1/events` webhook → 注入统一消息队列（Cline connectors 同构）
   - 价值：自动化从"定时"升级为"感知环境"，是"主动"的第一步
2. **主动提醒（记忆 → 相关时刻推送）**：
   - 通知通道：Tauri 系统通知（已有托盘基建，通知 API 轻量）
   - 触发评估循环：扩展 scheduler（新任务类型或 RuleTask 加条件），对记忆/规则做"relevant-now"评估，命中则经统一消息队列唤醒主 agent
   - 价值：行业最薄点；"个人助理"与"聊天机器人"的分水岭（Dot/Vellum 验证了需求，但都是云服务）
3. **剪贴板 I/O**（行业空白，低工作量高差异化）：
   - 输入侧：复制内容 → 快捷导入知识/上下文（"复制即记忆"）
   - 输出侧：agent 结果一键复制 / 自动写入剪贴板
   - 实现：Tauri clipboard 插件 + 前端入口
4. **桌面集成补全**：全局快捷键（唤起窗口/暂停任务）+ 系统通知（后台任务完成、审批挂起、主动提醒均走此通道）

### T2 —— 推荐（竞品标配，天演缺失，工作量可控）

5. **Plan 模式**：只读模式门控（plan 时写工具不可用，Cline 式硬阻断而非提示）+ 复用现有 ask_user 澄清 + 计划文本持久化（Devin megaplan 式）。
   - 注：不重建 planner 组件（保持"工具不做自主多轮决策"决策），只是执行模式门控
6. **命令级审批策略**：审批规则引擎（命令前缀 glob allow/prompt/forbid + 文件路径模式）——Codex rules / Devin permissions 同构；顺带审批卡支持"编辑命令后再批准"（Devin wand 简化版：前端编辑 + 直接执行，无需 LLM 改写）

### T3 —— 可选（视使用场景）

7. **git worktree 隔离并行**：后台任务默认在独立 worktree 执行，完成可合并——多后台任务并存时的结构性安全
8. **大任务自动分解**：delegate 前由主 agent 自动拆分（Cursor /multitask 模式）——收益取决于模型本身规划能力，非结构性差距

### 后置（与既有后置项合并）

- 屏幕感知/OCR（含浏览器交互自动化升级）——已有 ADR-010 图片输入，后置为统一"视觉理解"项
- 外部基准评测（SWE-bench 类）——与 CI 评测闭环一并

---

## 4. 明确不做（定位过滤器结论）

| 项 | 理由 |
|----|------|
| 执行沙箱（OS 级） | 本地单用户信任模型：审批流 + 快照回退已兜底；bwrap/Seatbelt 跨平台成本高。**以 T2 命令级审批策略作为替代性加固** |
| 网络访问策略 | 同信任模型；web 工具已在审批管控下 |
| 云任务 / 多设备 / 多用户治理 | 直接违背"本地优先、单用户"定位 |
| ACP 控制中心 | 不依赖外部 agent 产品；内部子任务编排已覆盖需求 |
| 插件市场 | 既有决策（内部技能系统 + GEPA 已承担扩展性） |
| 模型 harness 定制 | 既有决策（不做自动路由） |
| 防失控预算 / Checkpoint 升级 / 语音 I/O / 用户反馈采集 | 既有决策，维持不变 |

---

## 5. 结论

天演在**记忆与上下文工程层不劣于甚至领先业界**（双层摘要 + 记忆提取 + 会话压缩 + 前缀零漂移快照，商业产品均无同构），在**子任务编排、快照回退、审批工作流、定时任务**上已到 2026 竞品水准。真正的差距集中在**"主动层"**：事件驱动、主动提醒、剪贴板、桌面集成——而这恰好是行业最薄、且所有领先产品都因云架构无法本地化的区域。**T1 四项 = 把天演从"对话时强大的 agent"推进为"平时也在工作的本地助手"**，与"本地综合智能体"最终目标直接对齐。
