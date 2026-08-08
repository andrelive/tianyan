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

### Changed
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
