# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08

### Added
- 桌面应用（Tauri）：托盘常驻、系统通知、剪贴板、自动更新（用户确认后安装）
- GitHub Actions 发布流水线（签名构建 + latest.json + GitHub Release）
- 发布文档：known-issues.md / RELEASE_NOTES.md

### Changed
- 编辑链路重构：apply_patch 提为主力（unified diff + 上下文锚定，对齐 omo/Codex，@@ 可选）；apply_edit 改为内容匹配（old_string/new_string）
- read_file 输出纯内容（去掉行号/哈希前缀）
- 工具描述全部中文化；soul 清理（不列工具/技能，技能经 search_vfs 语义发现）
- search_knowledge 更名为 search_vfs（语义搜索整个 VFS：文档/记忆/规则/技能）
- 前端修复：\r 渲染归一化、回撤横幅残留、历史会话滚动定位

### Fixed
- 模型生成截断工具调用导致整轮 400（降级护栏）
- apply_patch 裸 @@ 块头报错（宽容解析 + 内容定位）
- apply_edit 长行锚点不一致（整行哈希 + 完整行返回）

## [Unreleased]

### Changed（技能系统回归 VFS 方法论文档，2026-09-09）
- **技能 = VFS 方法论文档，删除全部执行型基础设施**：6 个桥接技能（file_read/file_write/file_delete/file_list/system_command/http_request）与 `SkillExecutor`/`SkillHandler`/`SkillRegistry`/`ExecutorConfig`/`SkillExecutionRequest`/`SkillExecutionResult` 全部删除——文件/命令/网络能力由内置工具直接覆盖，技能只承载"怎么做"的方法论
- **`call_skill` 改为读 VFS**：按 ID 读 `tianyan://skill/{id}`（L0 摘要 + L2 详情）返回，由 LLM 参考后自行用工具执行；不再有 handler/执行语义
- **planning 预置进 VFS**：bootstrap 时写入 `skill/planning/`（原 PlanningHandler 静态指南），与 GEPA 学习技能同构，可被进化引擎完善/复审
- **注册表与刷新机制移除**：`SkillRefresher`/`SkillSync`/`refresh_registry`/会话边界技能刷新全部删除（VFS 实时读，无需注册表）；压缩点刷新只保留 injectable_context 清空
- **配置清理**：`security.skill_*` 6 个字段 + `allow_dangerous_skills` 删除（技能只剩 Safe 级文档，开关失去意义）
- **技能 API 改读 VFS**：`/api/v1/skills` 列表/详情/执行统一走 `SkillManager`（VFS 发现 + 读取）；"执行" = 返回技能文档

### Fixed（0.3.5 构建：唤醒轮可见性与事件通道重构，2026-09-08）
- **唤醒轮输出前端不可见**（切走再切回才看到）：后台命令完成 → 通知落库 → 唤醒轮跑 loop → 输出经 `chat_stream` 事件推送——但前端 `routeChatStreamEvent` 依赖**活跃流归约器注册表**（发消息时注册、流结束即注销），主循环空闲时事件到达无归约器可路由 → **静默丢弃**；且 `useUnifiedEvents` 挂在 `ChatPanel`/`AgentTasksPanel` 上，最后一个消费者卸载即 `stopUnifiedEvents()` **关闭 EventSource**——切到设置等面板期间订阅连接断开，切回才由 onopen 重放订阅（看到的其实是重连快照而非实时事件）
- **修复 1：EventSource 应用级常驻**——连接生命周期与组件卸载解耦（`startUnifiedEventsOnce` 模块级启动、进程存活期间不关闭），切到任何面板订阅连接保持，断线自动重连 + onopen 重放订阅不变
- **修复 2：chat_stream 归约器纯函数化**——删除 `createChatStreamReducer`/`activeStreamReducers` 注册表/`registerStreamReducer`/`unregisterStreamReducer`/`routeChatStreamEvent`（历史包袱：每个 SSE 响应一条独立连接时代的 per-stream 实例；ADR-028/031 收敛为统一事件通道后事件自带 session_id，实例前提消失）；改为无状态纯函数 `handleChatStreamEvent`（订阅级常驻，事件 → store 直接映射，唤醒轮/主对话流/子代理事件同一入口）；`liveWindow` 闭包状态一并移除（实际无消费点，圆环回退窗口恒为模型声明值）
- **副作用收敛**：纯函数不 `setCurrentSession`（唤醒轮事件不再拉回当前会话视图）；停止不再注销归约器（无注册表可注销）
- **回归测试**：chat-stream.test 改造为直接测 `handleChatStreamEvent`（10 用例，含唤醒轮消息边界事件写入 dict）；ChatPanel/AgentTasksPanel 同步更新

### Fixed（0.3.5 构建：唤醒轮空输出豁免移除，2026-09-08）
- **唤醒轮产生碎片垃圾消息**（"思考过程 @if"）：cmd 退出后唤醒轮执行时，模型返回仅含碎片思考（如 "@if"）、无正文无工具调用的空输出——`process_wake` 此前给唤醒轮加 `allow_empty_answer` 豁免（空响应直接放过不重试），垃圾消息被持久化到会话，用户重启后看到莫名其妙的"思考过程 @if"。
- **修复：唤醒轮与用户轮同构（统一循环框架）**——`AgentLoopConfig.allow_empty_answer` 字段与 `with_allow_empty_answer()` 方法移除；空响应（无正文无工具调用）在 `run` / `run_stream` 统一重试一次（同轮内，turn 不增加），重试仍空才作为合法结束（空输出 = completed，对齐 DSH 无工具调用收尾语义）；`process_wake` 不再调用 `with_allow_empty_answer()`。失败场景指令"必须汇报" + 空响应重试 = 模型获得第二次机会正确输出。
- **回归测试**：`test_run_empty_response_wake_turn_retries_once`（唤醒轮空响应重试一次，mock times(2)）+ `test_run_empty_response_retries_then_completes`（既有用户轮语义不变）

### Fixed（0.3.4 构建：压缩会话反馈与占用统计，2026-09-05）
- **压缩中无进行中反馈 + 可输入发送**：压缩（LLM 摘要生成，通常 10-30 秒）期间消息流底部无提示、输入框与发送按钮仍可用——用户可能在压缩完成前输入，与服务端压缩基于的分叉状态并发。压缩期间消息流底部显示「压缩中...」（spinner + aria-live），textarea 与发送按钮禁用（`compressing` 纳入 `canSend`/`disabled`/Enter 守卫），压缩完成/失败后恢复。
- **压缩后圆环占用不更新（下轮才变）**：圆环占用 = `lastMessageUsage`（从末尾向前找第一条带 usage 的消息）——压缩摘要消息 `tokens` 此前全零 → 前端映射 `usage:null` → 查找跳过摘要、命中压缩前最后一条 assistant 的 usage（压缩前完整占用）→ 圆环不变；下一轮新请求完成后才更新。同时摘要请求走 `chat()` 便捷方法，真实 usage 被丢弃（压缩消耗从未入账）。修复：
  - **摘要消息携带压缩请求真实用量**：`summarize`/`incremental_summarize` 改 `chat_completion()`（`CompressionResult.summary_usage` → `CompressionOutcome`）——摘要 StructuredMessage 的 tokens = 压缩请求真实 input/output/cache，随消息持久化（刷新后一致）
  - **圆环对摘要消息特殊处理**：`lastMessageUsage` 命中 `compression_marker` 消息时改用「第一条 assistant 的 prompt_tokens（≈系统前缀）+ 摘要 completion」估算压缩后上下文（摘要 input 是压缩前上下文，不能直接作占用）；新请求完成后新 assistant 消息优先命中
  - **会话消耗自动计入压缩**：`sumSessionUsage` 累加所有带 usage 消息——摘要消息自带压缩请求消耗（输入/输出/缓存），无需特殊分支即自动入账（最准：压缩请求 = 一次完整 LLM 请求）
  - **API 契约**：`ChatMessage` 新增 `compression_marker`（历史加载 + 流式边界事件从 `StructuredMessage.compression_marker` 映射），前端识别摘要消息

### Fixed（0.2.11 构建：唤醒轮失败场景静默，2026-09-03）
- **后台任务失败时主 agent 无反馈**：通知/注入/唤醒链路正常（System 通知入库 + 唤醒轮触发），但唤醒指令允许"空输出结束"，模型在失败场景下选择沉默——主 agent 静默无反馈（"任务失败但没通知"）。唤醒指令区分失败/完成场景：**失败必须向用户汇报**（ADR-013 shouldReply = allComplete || isTaskFailure 的语义落地），只有全部成功且无需输出才允许空输出；空输出日志从 debug 升级为 info（取证可见）。回归测试锁定失败场景指令含"必须汇报"、成功场景仍允许空输出

### Fixed（会话消耗汇总漏计工具轮输入，2026-09-03）
- **流式 usage 事件逐轮下发**：AgentLoop 每轮 LLM 调用都有真实 usage（完整上下文重发，O(n²) 量级），但流式事件只有最终轮经 `send_complete` 下发——中间工具轮的 usage 从未下发，前端本地消息只有每轮对话的最后一条 assistant 消息带 usage，`sumSessionUsage` 漏计所有中间轮输入（刷新历史后正确，流式过程中错误）。新增 `StreamEventSender::send_turn_usage`（delta 空、is_complete=false，前端归约器只消费 usage 字段，无正文/边界副作用），工具轮 persist 后逐轮下发（回归测试锁定）

### Fixed（ollama 思考字段回传，2026-09-11）
- **根因**：0.2.7 只修了"读"（`DeltaContent` 加 serde alias `reasoning`）没修"写"——`inject_reasoning_content` 把历史思考无条件写到 `reasoning_content`，而 ollama 的 OpenAI 兼容层（`openai/openai.go` 的 `Message.Reasoning`）**只解析 `reasoning`**。Go 的 `encoding/json` 静默忽略未知字段，所以请求照样成功，但思考内容根本没进 prompt——多轮上下文与思考一致性悄悄受损（实测：ollama cloud 上 `reasoning_content` 的 `prompt_tokens` 零增长，`reasoning` 则 +2300；类型探针也证实只有 `reasoning` 报 400）
- **修复**：新增 `ThinkingField` 传输层方言（`ReasoningContent` / `Ollama`），在 `AsyncOpenAIClient::from_provider` 按「**显式配置 > endpoint/名称嗅探 > 默认**」解析一次并缓存；`inject_reasoning_content` 按方言写字段名（互斥、不双写）。嗅探只认已知 ollama 域名与默认端口（`ollama` / `:11434`）及名称含 `ollama`，其余回落 `reasoning_content`——**刻意不猜自建域名**，猜不到时用 `thinking_field` 配置项显式覆盖
- **新增配置**：`[[models.providers]] thinking_field = "reasoning" | "reasoning_content"`（可选；缺省不序列化，不污染用户配置文件）
- 回归测试：方言写入互斥、嗅探命中/未命中、显式配置覆盖嗅探、TOML 解析与缺省省略

### Fixed（前端配置往返丢弃 provider 新字段，2026-09-11）
- **根因**：前端 `config-transform.ts` 对 provider 是**白名单映射**（from/to 双向都只列已知字段），而后端 `PUT /api/v1/config` 是**全量替换**——后端新增的 provider 级字段（本次的 `thinking_field`）不在白名单里，用户按文档手写进 `tianyan.toml` 后，只要打开设置页点一次"保存"，该字段就被静默抹掉。同类缺陷曾在 `learned_rules_top_k` / `shortlist_tools` / `safety_mode` 上发生过（契约快照测试的由来）
- **修复**：`ProviderConfigState` 增加 `thinking_field`，`fromBackendConfig` / `toBackendConfig` 双向透传（缺省不序列化）；设置页 provider 卡片新增「思考字段名」下拉（自动 / `reasoning_content` / `reasoning`）——逃生门字段必须能在 UI 里设置，否则只能手改 TOML
- **回归测试**：`config-transform.test.ts` 新增 from→to 往返保留用例（含缺省省略）；`contract.test.ts` 新增**通用防线**——逐字段断言后端 provider 的每个键都能往返回来，防止下一个新增 provider 字段重蹈覆辙

### Fixed（唤醒轮事件转发死锁，2026-09-11）
- **现象**：后台命令/任务完成后，唤醒轮的输出不实时显示，退出重进才看到
- **根因**：唤醒轮转发器先 `await` 整个 `process_wake` 完成，之后才去 `rx.recv()` 取事件——期间事件持续写入容量 100 的通道，唤醒轮输出超过缓冲即 `send().await` 阻塞，形成死锁（轮等转发器收、转发器等轮完）
- **修复**：`process_wake` 改收 `sender` 参数（与 `process_message_stream` 同构），转发器在轮开始前就建通道并立即消费；新增防死锁回归测试

### Changed（流式事件转发三处接线收敛为单一实现，ADR-032，2026-09-11）
- **根因**："chunk → 事件 JSON → 统一通道"的接线在用户轮 / 唤醒轮 / 子代理三处手写，已出现漂移（容量 100/100/64 不统一、字段注入各写一遍）——上述死锁 bug 正是该结构的产物（顺序写错从代码上看不出来）
- **重构**：core 新增 `agent/stream_forward.rs`——`spawn_stream_forwarder` 原子地建通道并启动消费任务（反向顺序从结构上写不出来），返回 `(sender, JoinHandle)`；`StreamEventMapper` / `StreamEventDeliver` 为有意保留的可插拔 seam（映射器与送达目标），内置 `BroadcastJsonDeliver`（用户轮）/ `TaskSinkDeliver`（子代理）/ `NullDeliver`；`inject_stream_event_fields` 单点注入路由字段。三处调用点改为复用该函数，`process_message_stream` 签名与 `process_wake` 同构
- **回归测试**：`stream_forward` 6 项单测 + 端到端 `test_wake_forwarder_delivers_events_while_turn_running`（锁定"轮运行中事件已送达"）

### Fixed（托盘退出卡死，2026-09-03）
- **常驻 SSE 流阻塞优雅关停**：`GET /tasks/stream`（后台任务聚合 SSE，ADR-026）forwarder 死等 broadcast 消息，前端 EventSource 常驻订阅——axum `with_graceful_shutdown` 等待所有活跃连接结束永不完成，托盘「退出」卡死只能杀进程。forwarder 加 shutdown 感知（`tokio::select!` 轮询 `shutdown_flag` 1s 间隔，与 `/chat/stream` 同模式）；ADR-026 补充「常驻 SSE 端点必须 shutdown 感知」约束

### Fixed（流式 usage 丢失根因修复，2026-09-03）
- **流式请求显式要求 usage**：`stream_options: {include_usage: true}`（OpenAI 规范：流式响应默认不返回 usage，除非请求显式要求）——ollama 等规范网关此前永远不返回 usage（实测"流式不返回"实为未请求）；对齐 DSH 同名参数（回归测试锁定请求体）
- **usage-only 尾块保留下发**：SSE 解析不再丢弃 `{"choices":[],"usage":{...}}` 尾块（include_usage=true 的正常流尾），构造带 usage 的空 chunk 下发供校准；纯 cost 块与空块仍跳过（回归测试锁定）
- **思考内容正确回传**（DeepSeek 官方要求）：携带 tools 的请求必须完整回传 `reasoning_content`（即使该轮未实际工具调用）——`convert_messages` 序列化后按索引回填 `inject_reasoning_content`（async-openai 0.34 类型无此字段，此前被静默丢弃）；`assembler` 无条件保留 reasoning；`TokenEstimator` 估算计入 reasoning 与 tool_calls 参数
- **换模型旧实测值失效**：`last_input_usage` 升级为 `(model, tokens)` 配对，切换 provider/model 后旧实测值退回全量估算（对齐 DSH token-meter header 不匹配语义）

### Fixed（ollama 网关兼容，2026-09-03）
- **思考过程不显示**：ollama 兼容层流式响应用 `delta.reasoning` 字段名（DeepSeek 用 `reasoning_content`）——`DeltaContent.reasoning_content` 加 serde alias `reasoning` 统一解析（回归测试锁定）
- **上下文圆环无数据**：ollama 流式 chunk 不携带 usage（非流式才返回）——流式 step 末尾加 usage 兜底（对齐 DSH token-meter 启发式：prompt 优先用上次实测值，completion 按正文+思考用 TokenEstimator 估算）；真实 usage 存在时不受影响

### Removed
- **工具短路选择（G1）回滚**：`[agent] shortlist_tools` 配置与 `definitions_shortlisted`/`dynamic_tool_matches`/`TOOL_SHORTLIST_THRESHOLD` 实现全量移除，`BUILTIN_TOOLS` 表去除 `core`/`keywords` 字段。原因：工具清单位于 OpenAI 请求前缀区（prompt 缓存 key），逐轮按 query 过滤使工具集合会话内漂移，破坏 ADR-012 前缀匹配缓存。全部工具改为恒常可见（确定性装配），前缀逐会话稳定。

### Fixed（前端交互审查落地，2026-08-25）
- **手动停止标记**：ChatPanel handleStop 给当前流式消息打 `interrupted:true`，残留部分输出显示「流式中断，已保留部分输出」，与服务端中断语义一致
- **发送竞态**：handleSend 改读 store 同步流状态防同帧双击并发两条流；useChatStream 增加代际计数，丢弃 stop 后旧流的迟到 onComplete/onError，不再覆盖新流 streaming 状态
- **后端运行时错误**：chat/stream 的 agent 运行失败分支发 `ChatStreamEvent::error`（原仅 log、SSE 干净结束 → 前端留空气泡无提示）
- **流式渲染**：chat-slice 文本 delta 若末段为 text 直接合并（O(n²) 段膨胀 → O(n)）；appendSkillCalls 按 skill_id 去重追加（不再整体覆盖丢多技能事件）
- **键盘快捷键**：焦点在可编辑元素时忽略全局快捷键（避免输入中误触 Ctrl+N / Ctrl+Shift+Delete 清空流式会话）
- **工具运行态**：ToolCallCard 结果未达时显示「运行中」spinner（区分执行中 vs 无结果）
- **知识库浏览**：切换 L0/L1/L2 层级时重取当前选中条目内容（原显示不变"点了没反应"）；知识搜索加「加载更多」分页（offset）
- **会话列表**：键盘导航改以 DOM 焦点为锚（修复分组/占位行导致的索引错位）；删除会话统一走 ConfirmDialog 二次确认；删除当前会话后跳回 /chat
- **文件树**：高度改为 ResizeObserver 自适应容器（原固定 600px 被裁剪）；子目录加载失败行内红色重试提示（原静默变空）
- **Diff 面板**：选中文件且存在当前会话时自动加载 diff（去掉手工填会话 ID + 点加载）；会话 ID 仍可手改以对比其他会话
- **侧边栏**：新增「文件」一级入口；审批/任务图标带待办角标（低频轮询）
- **洞察面板**：scheduler/stats 改 allSettled 分区容错（一处失败不再拖垮整面板，双失败保留原始错误）
- **任务面板**：结果/错误可展开全文；会话 ID 可点击跳转对应会话
- **错误边界**：路由级 ErrorBoundary 独立隔离各面板（单面板崩溃不拖垮整应用，导航按 pathname 重置）
- **列表搜索**：工具/技能/子智能体面板加名称/描述过滤（空结果有提示态）
- **审批倒计时**：待审批卡显示剩余自动处理时间（按 requested_at + timeout_secs 每秒刷新）
- **自动轮询**：检索轨迹/记忆面板 8s 静默刷新（useResource 新增 `silentReload` 不闪 Spinner；原仅手动刷新）
- **SSE 跑完再取（B）**：客户端断开不再取消 agent——本地助手后台任务不因 SSE 断线中断。后端每个 SSE 事件设 `id`；断线时 forwarder 进入排空模式（继续消费让 agent 跑完并持久化）；新增 `POST /chat/streams/{session_id}/cancel` 显式「停止」端点；前端停止走取消端点、断线显示「任务继续在后台运行」+「刷新结果」（不再 reload 覆盖 / 不再重跑该轮）
- **Toast 堆叠**：store 从单槽 `toast` 改为数组 `toasts`，多条提示各自 3s 自动消失、右上角纵向堆叠（连续错误不再互相覆盖）
- **死状态清理**：移除未使用的 `isSidebarOpen`/`toggleSidebar`/`setSidebarOpen` 及对应测试
- **记忆面板双栏**：列表 + 详情左右分栏（对齐工具/技能/角色面板布局），不再上下堆叠
- **apply_patch 提为主力编辑工具**：对齐 Aider/Codex/opencode 的业界共识（unified diff + 上下文锚定，抗行号漂移、无需逐字节复现整块原文）。工具描述/roles/ADR-009/default_soul 均改为「修改优先 apply_patch」；apply_edit 降为仅用于能精确复现原文的极小改动。**不做空白容差匹配**（容差会让模型漏掉的空行/格式问题被应用进文件，长期破坏格式）。
- **apply_edit 改为内容匹配（ADR-009 修订）**：`apply_edit` 从行号+hashline 锚点彻底改为 `old_string`/`new_string` 内容匹配（对齐 DSH/Claude Code），免疫行号漂移；`read_file` 输出纯内容（去掉行号/哈希前缀，省输入 token）；`ContentEdit{old_string,new_string,replace_all}` 唯一匹配，old_string 未找到/不唯一返回 409。移除 hashline 模块；同步更新工具描述、server workspace API、测试与全部文档（ADR-009/module-map/module-descriptions/gap-analysis/AGENTS.md）。
- **工具描述中文化 + 编辑链路说明**：全部内置工具描述改为中文（中文 token 更省）；read_file 说明行号+哈希供 apply_edit 用、apply_edit 说明哈希取自 read_file/报错返回值（可省略锚点用行号+内容）、apply_patch 说明超大单行文件应改用 diff 片段编辑
- **read_file 完整行返回**：read_file 不再截断行内内容（此前超长行被截断，模型读到不完整行 → 无法正确推理/整行替换）；改为返回完整行 + 整行哈希锚点，与 apply_edit 校验一致（长行锚点 bug 一并修复）。输出体量由 offset/limit 行数与统一字节预算兜底
- **输出 `\r` 归一化**：模型输出可能带裸回车符（CRLF/孤立 \r），渲染前统一转 \n（MessageBubble 正文 + 思考块），避免正文出现 `\r` 字符
- **回撤横幅残留修复**：发起新消息时清空 `lastRollbackMessageId`，「已回退—撤销回退」横幅不再残留在输出底部
- **历史会话定位**：进入会话（非流式）时自动滚动到最新输出位置，不再停在顶部；流式中仍仅近底部才跟随
- **定时智能体任务（新能力）**：可指令智能体（schedule_task 工具）或经 REST API 创建定时任务——按完整 cron 调用 agent 在指定工作区完成给定指令。任务定义持久化到 {data_dir}/scheduled_agent_tasks.json（重启恢复）。**调度基础设施收敛为一套**：核心 TaskScheduler 升级为完整 cron 语义（引入 cron crate 解析，替代原 */N 间隔）+ 支持运行期动态注册/注销；定时 agent 任务注册进同一 TaskScheduler（复用其调度循环/运行统计/状态查询），不再用独立循环。前端「任务」面板新增「定时任务」区（列表 + 新建 + 删除）。接口：GET/POST /api/v1/scheduled-tasks、DELETE /api/v1/scheduled-tasks/{id}。

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
- **前端质量门禁**：lint + typecheck + vitest 经 `scripts/test.ps1` 执行（CI `release.yml` 仅发布构建，质量门禁暂未接入 CI）
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
