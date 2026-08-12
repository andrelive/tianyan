# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Tauri + React + TypeScript + Axum** 桌面应用架构（替代原 CLI-only 模式）
- **Agent Loop 架构**：LLM 自主工具调用 + 流式 SSE 响应（6 种 chunk_type）
- **VFS 双层摘要索引**：L0/L1/L2 三层内容 + LanceDB RRF 融合检索
- **StructuredMessage** 单一真相源：持久化（JSONL）、会话组装、Token 统计
- **ToolRegistry**：14 个 OpenAI function calling 兼容工具
- **Skill 系统**：6 个内置技能 + GEPA 进化引擎自动学习
- **Scheduler 定时任务**：RuleTask、MemoryTask、SummaryTask、GcTask
- **ContextPipeline**：soul → rules → retrieval → compression → assemble
- **ModelServices** 容器：统一管理 ChatService / EmbeddingService / VlmService
- **PersistentSessionManager**：VFS 持久化会话管理
- **AgentMetrics**：可观测性存储和 Agent 自省接口
- **KnowledgeIngestor**：知识库导入管道（已通过 knowledge_ingest 工具接入 Agent 流程）
- **/chat/clarify 追问链路**：Agent 澄清问题 → 用户回答 → 继续执行（前端 ClarificationBubble）
- **会话标题编辑**：POST /sessions/{id}/title + Sidebar 双击重命名
- **审批降级询问用户**：高风险操作默认需用户确认，拒绝时自动转为追问（指纹确认缓存闭环）
- **CI 前端质量门禁**：lint + typecheck + vitest 进入 push/PR 检查
- **核心路径 P0 测试**：MemoryExtractor 提取链路、AgentLoop 完整循环（工具调用→回答）
- **MCP 工具桥接**：配置的 MCP 服务器工具注册进 ToolRegistry，对 LLM 可见可执行（此前仅可配置/测试连接）；跨 reload 保持连接一致，关闭时显式断开
- **记忆浏览 + 统计端点**：GET /api/v1/memory（VFS 记忆命名空间读路径）+ GET /api/v1/stats（UsageStats 摘要）——补齐 scheduler/observability"只写不读"缺口
- **检索轨迹持久化**：GET /api/v1/retrieval/traces — 单次检索完整过程快照（意图分析/每步搜索分数/内容加载层级/耗时），FIFO 保留 500 条；统计是"流量计"，轨迹是"黑匣子"，用于诊断上下文路由失败
- **调度器状态端点**：GET /api/v1/scheduler/status — 定时任务一览（ID/名称/优先级/Cron/执行次数/距上次执行），补齐 scheduler"只写不读"最后一块；无模型 Provider 时返回空状态
- **审批状态端点**：GET /api/v1/approval/status — 审批链路全景（风险分级配置/自动审批规则/待人工审批/询问用户降级队列/审计记录）——此前审批完全黑盒，危险操作被拒后只能从对话里感知
- **Tauri 端口管理**：首选 3000 被占用时动态选择空闲端口（消除 server.rs/lib.rs 两处硬编码重复）；实际端口经 `window.__TIANYAN_API_BASE__` 注入前端，`getApiBase()` 优先读取——修复"端口冲突导致桌面应用无法启动"的隐患（不含认证层，单独立项）
- **检索轨迹接入结果清单**：轨迹的 `results` 字段此前恒空，现在记录最终命中 URI；删除死代码（`add_l0_search` 无真实调用点、`TokenStats`/`TokenPercentages` 与分析辅助方法仅测试使用），摘除 4 处"为调试 UI 预留"的 dead_code 豁免
- **技能/工具安全路径测试**：85 个新测试覆盖文件读写删、系统命令、HTTP 处理器与 14 个工具执行器，测试驱动修复 3 个安全缺陷（IPv6 回环 SSRF、127/8 段 SSRF、Windows verbatim 路径误拒）
- **集成测试真实路由**：server/tests 驱动真实 `create_app()`（完整 VFS+AppState），替代手工模拟假 router
- **MCP 图片链路**：MCP 工具返回的图片（如浏览器截图）base64 解码落盘 `{data_dir}/mcp_images/` 并以路径追加到工具结果，截图对 LLM 可见可访问（`call_tool_detailed` + `McpImage`/`McpCallOutput`）
- **对话图片输入**：五层全打通（前端粘贴/拖拽/选图 → API `images` 字段 → 多模态 LLM 请求 → `Part::Image` 持久化 → 历史重放与回显）；`ContentPart`/`ImageUrl` 上移 common 基础层（ADR-010）
- **子 Agent 编排增强**：`delegate_to_agent` 支持嵌套委托（树状编排，深度上限 3 + RAII guard 防失控派生）+ `max_turns`/`timeout_secs` 参数；同轮多次委托经 `execute_parallel` 天然并行
- **回答质量评测**：`core/src/eval/` LLM-as-Judge 评分式（四维度 1-10 分 + 加权总分 + 分级判定），JSON→行格式→中性分回退链，`run_eval_suite` 批处理 + 5 个黄金用例（离线基准，不接入在线链路）
- **Web 搜索与抓取**：`web_search`（结构化结果：标题/URL/摘要；DuckDuckGo 零配置后端 + 可切换 SearXNG）与 `web_fetch`（可读正文提取：标题 + 主文本 + 链接）；SSRF 防护（仅公网 http/https，与 http_request 同策略）+ 响应大小上限 + TTL 缓存 + 防注入可信度提示（[web] 配置节）
- **后台任务**：`delegate_to_agent(background: true)` fire-and-forget——立即返回 task_id，任务独立运行；完成时自动向父会话注入 System 通知（结果摘要 + 剩余任务计数 join 信号），主 LLM 下一轮聚合继续（业界模式：opencode task(background) / Claude Code background subagents）；`task_status`/`task_cancel` 工具 + `GET /api/v1/tasks`；并发上限 4 + 任务注册表
- **会话边界技能刷新**：GEPA 进化出的新技能对新会话立即生效——新会话创建时增量注册（`SkillManager::refresh_registry` 幂等，仅注册新增）；会话内保持冻结，system 前缀稳定不破坏 prompt 缓存（启动时全量加载保留）
- **压缩点技能刷新 + 手动压缩**：上下文压缩（自动或手动触发）是会话内唯一的前缀重建时刻——压缩成功后同步清空注入上下文缓存（learned rules/memories 下一轮重新检索，GEPA 经验对会话后续阶段可见）并增量注册技能（`SkillRefresher` 钩子）；`POST /api/v1/sessions/{id}/compress` 手动压缩入口（与自动压缩共用 `maybe_compress_and_persist` 逻辑，仅触发点不同）
- **注入上下文快照持久化**：soul/rules/memories 前缀快照随会话固化（JSONL 首行 SessionHeader）——重启后旧会话沿用同一份快照，不重新检索，前缀内容与重启前一致（prompt 缓存不失效、语义不漂移）；仅在会话首次加载（无快照）与压缩点更新；旧格式会话兼容加载
- **后台任务并发正确性**：session_id 从共享可变字段（`current_session_id`）改为**调用链显式参数传递**（`execute_parallel`/`execute_single`/`delegate_to_agent`）——多会话并发 turn 不再互相覆盖归属，后台任务可靠挂到正确的父会话；任务注册表新增单调递增 `seq` 排序键（替代毫秒时间戳——同毫秒注册 + HashMap 随机迭代导致快照顺序不确定的竞态）
- **审批挂起通知**：wait_for_approval 模式（GUI 审批面板通道）挂起时，`ApprovalPendingNotifier` 把审批请求作为 System 消息注入所属会话——前台任务用户获得面板指引；**后台子 agent 的审批请求不再静默**（此前无人知晓、只能等超时拒绝），用户到审批面板批准/拒绝后任务经 oneshot 通道恢复继续
- **子任务无交互审批**（原则落地：子 agent 是主 agent 意图的执行器，任务下发即授权边界）：子任务（subagent）上下文审批**永不等待**——已确认指纹命中（主 agent 确认过）直接执行（指纹共享），未授权新危险操作立即拒绝（`request_approval_no_wait`，拒绝原因携带"主任务授权"标记）→ 子 agent 上报 → 主 agent 在主对话确认 → 指纹记录 → 重新委托；交互只发生在主 agent 与用户之间。同时修复审批调用的真实 session_id 传递（此前硬编码 "tool-execution"，挂起通知/归属全部错乱）
- **角色化子 Agent 委托**：`delegate_to_agent` 新增 `role` 参数——内置 researcher（检索/调研）/ editor（代码编辑）/ reviewer（验证/评审）三角色，各带中文系统提示与工具白名单；`tianyan.toml` 新增 `[agent_roles]` 配置节：同名角色整体覆盖内置定义，新名字新增角色；角色提供模型/系统提示/工具白名单/max_turns/timeout_secs。`model` 参数为显式模型逃生舱（优先级：显式 model > role.model > 主 Agent 模型；system_prompt：显式 > 角色 > 无；轮数/超时：显式 > 角色 > 默认 200）。角色白名单外工具调用不执行，合成错误结果回喂模型（模型可换用允许的工具重试，循环不硬失败）；未知角色在循环启动前报错并列出可用角色；后台委托与前台共用同一过滤路径
- **记忆溯源补强（G3）**：`MemoryEntry` 新增 `source_message_ids`（消息级引用）——MemoryTask 从会话 JSONL 解析消息 ID 随提取结果写入记忆（`**来源消息**` 字段）；记忆 L1 摘要携带 `来源会话`（注入上下文时出处对 LLM 可见）；`[memory] verify_preferences`（默认关闭 opt-in）开启后偏好类记忆先经 LLM 写前校验（是否稳定长期偏好），未通过/校验失败即丢弃（`MemoryExtractor::verify_preference`，保守拒绝策略）
- **技能激活可观测性（G2a）**：`AgentMetrics` 新增技能激活统计（`record_skill_call` + `query_skill_activation`，按技能 ID 计数、Top-20 排序）；`execute_call_skill` 自动记录；`self_check`/Harness 健康摘要新增 `skills_invoked` 与 `skill_calls_total`——Agent 自省可即时看到技能是否真的被激活（与 UsageStats 持久化统计互补：后者面向 `/api/v1/stats`）
- **工具短路选择（G1）**：LLM 可见工具数超过阈值（40）时按 query 相关性过滤工具 schema——核心工具集（19 个基础操作/通用检索）恒存；条件工具（knowledge_ingest/run_tests/discover_tests/verify_build/symbol_outline/lsp）按关键词表匹配；MCP 动态工具按名称/描述与 query 英文分词（≥3 字符）与中文片段（≥2 字符）重叠匹配。**执行层不受影响**（被过滤工具被调用仍正常执行，无"未加载"错误路径）；工具数 ≤40 或无用户消息时行为零变化。`[agent] shortlist_tools`（默认 true）可关闭。实现：`tool_registry/definitions_shortlisted` + `AgentLoop::tools_for_turn`
- **GEPA 候选技能验证门（G2b）**：`SkillLearningEngine` 新增 `verify_candidate`（LLM 质量审查：0-10 打分 + 问题清单）；新生成技能注册前经验证门，低于阈值（默认 6）标记为**试验性技能**——L2 内容带 `**状态**: 试验性` 标记、L0 摘要带 `[试验性]` 前缀（仍注册可发现，渐进式披露时提示谨慎使用）；验证不可用（LLM 错误/解析失败）降级正式注册，不阻塞学习回路。配置：`SkillLearningConfig.candidate_verification`（默认 true）/`candidate_score_threshold`（默认 6）
- **后台任务结果自审门（G4）**：`BackgroundTaskManager` 新增可选 `TaskReviewer`（`LlmTaskReviewer` 实现，复用 `VERDICT: PASS|FAIL|NEEDS_CHANGES` 判定格式）——后台任务完成通知（ADR-013）注入父会话前经 LLM 自审，未通过则结果前缀 `[自审未通过] {reason}` 标记（通知与 `task_status` 均携带），主 agent 复核决策，**不自动重跑**；空结果/自审不可用默认通过（不阻塞通知）。配置：`[agent] background_self_review`（默认 false opt-in，每任务多一次小调用）
- **结构化 Trace（G6）**：`observability/trace.rs`——span 模型（turn/tool/task 三类）持久化到 SQLite `trace_spans` 表（热路径内存缓冲 + 查询前自动 flush + 全局保留窗口 10K 条清理）；埋点：AgentLoop 每轮（含 token 消耗）、execute_single 每工具（参数摘要 + 耗时 + 成败）、BackgroundTaskManager 每任务终态（task_id 关联独立子树）；按 `(session_id, task_id, turn_index)` 分组回放还原调用树。API：`GET /api/v1/traces?session_id=&task_id=&limit=`（分组视图，前端回放面板数据源；G9 CI 评测闭环地基）
- **MCP streamable HTTP 传输（G7）**：`McpClient::connect_http` 支持远程 MCP 服务器（`rust-mcp-sdk` 启用 `streamable-http` feature，`with_transport_options` 入口，对齐 2026-07-28 stateless 核心规范）；配置 `[[mcp.servers]] transport = "http"` + `url`（http/https 绝对地址，非 http 前缀在发起连接前拒绝）；server 装配与 `POST /config/mcp/servers/{name}/test` 按传输方式分流（http → connect_http / 缺省 stdio 子进程）；旧配置无 transport/url 字段兼容（默认 stdio）
- **定时任务跨运行状态（G5）**：新增 `TaskStateStore`（VFS `tianyan://memory/events/task_states/` 命名空间，容错读写/删除）——`TaskContext.task_state` 供任何任务读写自己的持久状态（"carries state between runs"）；MemoryTask 示范用例：每次运行读上次周期摘要、结束后写入本次摘要（处理会话数/提取记忆数）；内置任务自包含不受影响

### Changed
- **死面清理包**：删除零调用的 `AgentMetrics::record_failure`/`failure_history`/`query_common_failures`（含 harness 摘要的 `common_failures_count` 字段）；删除恒为 Abstract 的 `SearchResult.matched_level`（无任何消费方）；删除无数据源的 `RetrievalResult.freshness_score`/`source_updated_at`（新鲜度常数 1.0 内联，排名行为逐位一致）；移除无人监听的 `tianyan-stop-stream` 快捷键死线（Ctrl+Alt+S，停止流式本就走 AbortController）
- **server 模块级静态清除**：事件 webhook 令牌与总线改经 `AppState` 直读（热更新即时生效，删 `set_event_bus`/`set_webhook_token` 及静态）；剪贴板 pending/outbox 迁入 AppState（`clipboard_outbox`/`clipboard_pending` 句柄注入，`ClipboardWriteTool::shared` 删除）——测试不再需要串行化锁（每个 AppState 独立状态）
- **调度器文档与实现对齐**：模块文档删除「使用 tokio-cron-scheduler」的不实声明（自研间隔循环 + `*/N` 简化 cron 解析，语义边界写入 `parse_cron_interval` 文档：固定值=相位忽略、日/月/星期不支持、回退 300s 告警）；移除只翻转运行标志、生产未使用的 `TaskScheduler::start()`（空操作易误导）
- **GcTask 配置接线**：`auto_cleanup` 从 `StorageConfig.auto_cleanup` 注入（原注释声称配置驱动但 new() 硬编码 true），注册处传 `[storage]` 配置；TTL 阈值保持模块默认值
- **VFS 内部重复收敛**：write/append 共用的 15 行幂等建目录块提取为 `VirtualFileSystemImpl::ensure_entry_exists`（已存在静默跳过，与 create_file 的报错语义区分）；知识摄入移除冗余双 upsert（update_metadata 前置写入会被 index_entry 的完整 payload 覆盖，content_hash 等元数据随向量 payload 一次写入）——每次摄入 LanceDB 写入减半
- **协调器三层穿透消除**：Agent 构造时从 ToolRegistry 提取后台任务/审批/待确认队列的**共享 Arc**（同一实例，非复制状态）——`approval_status`/`respond_approval`/`background_tasks`/`cancel_background_task`/`register_task_waker` 从 `Agent → AgentLoop → ToolRegistry` 穿透改为直连；删除 ToolRegistry 的纯转发 `set_task_waker`
- **LLM 输出截断收敛**：`common::llm_judge::truncate_output` 成为唯一实现（此前 4 份逐字复制：eval/judge、executor/judge、agent/background、executor/web），统一「内容被截断」标记并修复 UTF-8 字节边界 panic（CJK 安全）；`ChatCompletionResponse::first_choice_content` 收敛 3 处「取首个 choice」样板（实测探索报告高估了 judge 管道重复——提示词与回退链语义本就不同，未强行合并）
- **MCP 连接逻辑收敛**：传输分发唯一实现下沉 `mcp::McpClient::connect_from_config`（stdio/http/未知回退）；`McpClientManager` 补齐 `connect_from_config`/`sync` 并改为 `&self`（内部已同步）；server 层 `McpToolManager` 删除自建客户端注册表改为委托，配置测试端点同样走唯一分发——此前 3 处连接逻辑复制
- **`/chat` 与 `/chat/stream` 请求体瘦身**（破坏性变更）：`messages: [...]`（全量历史，服务端只读末条）→ `message: {role, content, images?}` 单条输入——历史由服务端会话持久化提供，消除冗余载荷、占位符耦合与双真相源
- **会话元数据持久化**：created_at/title/ended_at 收敛进 JSONL 首行 SessionHeader（重启恢复标题/排序，删除只写不读的 VFS custom metadata 路径）；`list_sessions` 按目录条目过滤，幽灵会话消失；记忆提取水位线迁出 Session 命名空间（`memory/events/extraction_state/`，旧 `_metadata` 自动迁移）
- **会话 JSONL 格式知识收敛**：`session::parse_message_lines` 成为唯一解析点（会话加载 / 记忆提取计数 / 消息溯源共用）；记忆提取水位线不再把首行 SessionHeader 计入（差一修复）
- **审批门控收敛**：6 个危险工具（write_file/apply_edit/apply_patch/execute_command/run_tests/verify_build）复制 6 份的审批序列合并为 `ToolRegistry::ensure_approved` 单一实现
- **使用统计刷盘接通**：`/api/v1/stats` 技能/文档计数恢复真实数据（查询前自动落库 + 每 1 分钟 `usage_stats_flush` 任务 + 退出前 shutdown 落盘）
- Workspace 多 Crate 重构（core/server/gui/tauri）
- `core/` → `tianyan-core`（lib name: `tianyan`）
- `Server` 层：知识 API 完成真实集成（ingestion + retrieval）
- `Server` 层：`anyhow` 迁移完成
- **KnowledgeIngestor** 泛型消除（4 泛型参数 → `Arc<dyn ...>` 具体 struct）
- **规则管线重构**：RuleRecorder/RuleSuggester 移至 `scheduler/tasks/`
- `DEFAULT_SOUL` 通过 `include_str!` 构建
- **审批默认关闭无人值守**：Medium/High 风险操作需用户确认；`wait_for_approval` 开关为 GUI 审批通道预留
- **AppState 复用 ModelServices**：消灭 3 处 block_in_place 重复重建（配置热更新时重建）
- **SummaryTask 已处理缓存**：超限 `clear()` 改为 FIFO 淘汰，避免全量重复 LLM 摘要
- **SSE 事件 id**：递增 chunk_id 改为每次响应唯一 id（Last-Event-ID 语义对齐）
- **前端类型契约对齐**：SkillParameter 序列化 `type` 字段、Ollama/MCP 类型统一收口、Skill 补 version/enabled
- **ToolRegistry 拆分**：execute_single 270 行拆为 14 个独立工具方法（均 ≤100 行）

### Removed
- `planner/` 模块（Planner-Executor 架构废弃）
- `ModelRouter`（被 `ModelServices` 替代）
- `TokenBudget`（被 `ContentLoadStrategy::from_score()` 替代）
- `Chunker` / `DocumentChunker` / `ChunkingConfig`（VFS 双层检索替代）
- `ConversationSummarizer`（被 `ContextCompressor` 替代）
- `VisionEncoder`（被 VFS 图像双通道替代）
- `AgentHarness` wrapper（功能由 `Agent` 直接持有）
- `AgentSkills` wrapper（功能由 `Agent` 直接持有）
- `MemoryExtractionTrait`（简化为 `MemoryExtractor`）
- `ContextRetriever` trait
- `RetryService`（Providers fail fast）
- `KnowledgeSearchResult`（遗留检索管道死代码）

### Fixed
- VFS write/append 自带容错，移除上层冗余检查
- 配置查找顺序规范化：`./tianyan.toml` → `~/.config/tianyan/tianyan.toml` → `~/.tianyan/tianyan.toml`
- **SkillParameter 前后端字段名漂移**：`param_type` 序列化为 `type`，参数表单按类型正确渲染
- **错误分类结构化**：`TianyanError::not_found()/is_not_found()` 统一 13 处字符串前缀判断
- **静默错误补日志**：压缩失败、流式发送失败、快照清理等 5+ 处补 tracing
- **README API 表对齐**：全量核对 /api/v1/ 前缀与 35 个端点
- **MSW mock 对齐真实路由**：/knowledge/search 等方法修正
- **前端 typecheck/lint 全绿**：修复 42 文件 prettier 格式 + 类型错误

## [0.1.0] - 2024-01-15

### Added
- Initial release
- Basic CLI interface with `chat`, `search`, `ingest` commands
- OpenAI model support
- Local file storage backend
- Basic memory system
- Configuration file support
- Environment variable configuration

### Architecture
- Unified context storage with `tianyan://` URI scheme
- Three-layer summary for efficient context retrieval
- Virtual file system mapping to local storage
- Qdrant-based vector storage for semantic search

### Documentation
- README with installation and usage instructions
- Configuration guide
- Development guide
- Example configuration files

### Infrastructure
- GitHub Actions CI/CD pipeline
- Cross-platform build support (Linux, macOS, Windows)
- Installation scripts for all platforms

---

## Version History

| Version | Date | Description |
|---------|------|-------------|
| 0.1.0 | 2024-01-15 | Initial release |

---

## Upgrade Guide

### From 0.1.0 to Unreleased

No breaking changes in the unreleased version.

---

## Roadmap

> ⚠️ 此 Roadmap 自 2024-01 后未更新。当前架构已大幅演进，参见 [系统架构文档](./docs/system-architecture.md)。

---

## Contributing

See [README](./README.md) and [AGENTS.md](./AGENTS.md) for development information.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
