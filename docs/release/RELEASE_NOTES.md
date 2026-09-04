# 天演 Tianyan 0.2 发布说明

> 0.2.0 发布日期：2026-08-30 · 0.2.2/0.2.3 修复日期：2026-08-30 · 0.2.4 修复日期：2026-08-30 · 0.2.5 修复日期：2026-08-31 · 0.2.6 修复日期：2026-09-01 · 0.2.7 修复日期：2026-09-03 · 0.2.8 修复日期：2026-09-03 · 0.2.9 修复日期：2026-09-03 · 0.2.10 修复日期：2026-09-03 · 0.2.11 修复日期：2026-09-03 · 0.2.12 修复日期：2026-09-04

天演（Tianyan）是一个本地优先的通用智能助手：桌面应用（Tauri）+ 本地服务（Axum）+ 自进化智能体（VFS 统一知识/记忆/技能/规则）。

## 0.2.12 修复（手动压缩会话：无进行中反馈 + 可重复点击 + 摘要不显示）

- **根因**：① 压缩按钮点击后立即关闭详情面板，`compressing` 状态无处展示——整个压缩过程（LLM 摘要生成，通常 10-30 秒）界面无任何反馈；② `compressing` 状态未传给按钮做 disabled，期间可反复点击（服务端虽有 `turn_guard` 串行化 + 消息数守卫兜底，第二次返回"无需压缩"，但体验差）；③ 压缩成功后只弹 toast，**未刷新消息列表**——摘要消息（system 角色）已持久化到服务端，但前端 store 不会自动更新，摘要永远不出现在会话流中
- **修复**：`ContextRing` 新增 `compressing` prop——压缩期间面板保持打开，按钮就地变为「压缩中...」（spinner + disabled，防重复点击）；`ChatPanel.handleCompress` 压缩成功后调用 `reloadSession` 重新拉取消息列表，摘要消息（`[对话摘要]` system 消息）出现在会话流中（回归测试锁定：压缩中按钮禁用 + 压缩成功后消息刷新）

- **根因**：通知/注入/唤醒链路正常（System 通知入库 + 唤醒轮触发），但 `prepare_wake_context` 的唤醒指令允许"空输出结束"，模型在后台任务失败场景下选择沉默——主 agent 无任何反馈（构建失败后 5 分钟无反应）
- **修复**：唤醒指令区分失败/完成场景——感知任务状态（`background_tasks.snapshot()` + `command_tasks.list()` 任一 Failed），失败时指令明确"必须向用户汇报失败情况，禁止输出空文本"（ADR-013 shouldReply = allComplete || isTaskFailure 的语义落地）；全部成功且无需输出才允许空输出；空输出日志从 debug 升级为 info（取证可见）

## 0.2.10 修复（会话消耗汇总漏计工具轮输入：流式 usage 事件只下发最终轮）

- **根因**：AgentLoop 每轮 LLM 调用都有真实 usage（完整上下文重发，O(n²) 量级），且服务端持久化完整（每轮 assistant 消息都带 tokens）——但流式事件只有最终轮经 `send_complete` 下发 usage，**中间工具轮的 usage 事件从未下发**。前端本地消息因此只有每轮用户对话的最后一条 assistant 消息带 usage，`sumSessionUsage` 只累加"每轮最后一次 LLM 调用的输入"（如 84,447 + 99,252 = 183,699），而非所有 LLM 调用的输入总量。刷新历史后统计正确（历史加载路径逐消息映射持久化 usage），流式过程中本地累积错误
- **修复**：新增 `StreamEventSender::send_turn_usage`（delta 空、is_complete=false，前端归约器只消费 usage 字段附加到最后一条 assistant 消息，无正文/边界副作用）；`handle_llm_response` 工具轮分支在 persist 后逐轮下发该轮 usage——前端本地消息每轮都带 usage，会话消耗汇总恢复为真实总量（回归测试锁定工具轮事件下发）

## 0.2.9 修复（托盘退出卡死：常驻 SSE 流阻塞优雅关停）

- **根因**：`GET /tasks/stream`（后台任务聚合 SSE，ADR-026）的 forwarder 死等 broadcast 消息，无任何退出条件；前端 `AgentTasksPanel` 用 EventSource 常驻订阅该流（会话存在期间不关闭）。托盘「退出」→ `app.exit(0)` → ExitRequested 钩子 `prevent_exit` 后 `rt.block_on` 等待内嵌 server 优雅关停 → axum `with_graceful_shutdown` 等待所有活跃连接结束 → 常驻 SSE 连接永不结束 → 进程卡死，只能杀进程
- **修复**：`stream_tasks` forwarder 加 shutdown 感知（与 `/chat/stream` 的 `spawn_sse_forwarder` 同模式）——`tokio::select!` 轮询 `shutdown_flag`（1s 间隔），置位即结束流，优雅关停链得以完成（约 1-2 秒内退出）
- **约束**：任何新增常驻 SSE 端点必须带 shutdown 感知，否则阻塞应用退出（已写入 ADR-026）

## 0.2.8 修复（流式 usage 丢失根因：请求未显式要求 + 尾块被丢弃）

- **根因**：0.2.7 的 usage 兜底只是"症状缓解"——真正的根因是两条叠加的链路缺陷：① 流式请求从未发送 `stream_options: {include_usage: true}`（OpenAI 规范：流式响应默认不返回 usage，除非请求显式要求）——ollama 等严格遵守规范的网关因此永远不返回 usage，0.2.7 之前实测"ollama 流式不返回 usage"其实是没向网关要；② SSE 解析把 `{"choices":[],"usage":{...}}` 尾块（include_usage=true 的正常流尾）当作"正常流尾"静默丢弃——即使网关返回了 usage 也会丢掉
- **流式请求显式要求 usage**：`chat_completion_stream` 构造请求时设置 `stream_options: {include_usage: true}`（对齐 DSH 同名参数，回归测试锁定请求体含该字段）
- **usage-only 尾块保留下发**：SSE 解析识别 `{"choices":[],"usage":{...}}` 块后构造带 usage 的空 chunk 下发（loop 收到后更新 `last_input_usage` 校准值），不再静默丢弃；纯 cost 块（兼容层 `{"choices":[],"cost":"..."}`）、无 choices 无 usage 的空块仍正常跳过（回归测试锁定两种形态）
- **思考内容正确回传**（DeepSeek 官方要求）：携带 tools 的请求必须完整回传 `reasoning_content`（即使该轮未实际进行工具调用，官方 thinking_mode 文档）——`convert_messages` 此前用 `..Default::default()` 构造 assistant 消息（async-openai 0.34 无此字段），序列化后被静默丢弃；改为序列化后按索引回填（`inject_reasoning_content`，非流式与流式统一走 JSON 层）；`assembler` 去掉 `has_tool_calls` 条件无条件保留 reasoning（回归测试锁定无工具调用轮也保留）
- **估算计入思考与工具参数**：`TokenEstimator.estimate_message` 此前只算 `message.content`，漏掉 reasoning_content 与 tool_calls 参数——思考十几万字的会话被低估（5% 显示的根因之一）；估算纳入思考内容与工具调用参数（回归测试锁定），无 usage 兜底时占用显示包含思考
- **换模型旧实测值失效**：`last_input_usage` 从裸数字升级为 `(model, tokens)` 配对——同一会话切换 provider/model 后旧实测值不再适用（不同模型分词/计费口径不同），退回全量估算（对齐 DSH token-meter：header 不匹配时全量重估）（回归测试锁定）

## 0.2.7 修复（ollama 网关兼容：思考过程不显示 + 上下文圆环无数据）

- **根因**：聊天模型切换到 ollama 提供商（deepseek-v4-flash:0731）后，两个功能同时失效——ollama 兼容层流式响应与 opencode 网关存在两处协议差异：思考字段名不同（`delta.reasoning` vs `delta.reasoning_content`）、流式 chunk 不携带 usage（非流式才返回）
- **思考过程兼容**：`DeltaContent.reasoning_content` 加 serde alias `reasoning`，统一解析 ollama 与 DeepSeek 两种字段名（回归测试锁定）
- **流式 usage 兜底**（对齐 DSH token-meter 启发式）：网关不返回 usage 时用 TokenEstimator 估算（prompt 优先用上次实测值，completion 按正文+思考估算）——上下文占用/缓存命中展示与压缩判定不因网关差异而失效；真实 usage 存在时不受影响

## 0.2.6 修复（后台任务与子智能体统一面板，ADR-026）

- **委托只支持异步**：delegate_to_agent 删除同步分支——所有委托一律注册后台任务 + 立即返回 task_id（同步预算/超时提升逻辑删除），数据源统一、与 ADR-013 唤醒语义完全对齐
- **右侧任务面板（统一展示）**：后台任务与子智能体委托同一套展示机制——活跃在上、完成沉底、可展开、可取消、可折叠；委托任务展开 = 子智能体消息流（历史 + SSE 实时增量，渲染复用主对话流，视觉完全一致）；终端任务展开 = 输出尾部
- **子智能体 = 带父会话引用的会话**：子智能体消息流走会话存储（session_meta 加 parent_session_id，幂等迁移），逐轮落库（不进 FTS——子会话无"人"提供的信息，回忆检索天然排除，索引不膨胀）；面板展开复用 GET /sessions/{id}/messages 加载历史
- **子智能体过程实时可见**：GET /tasks/stream 聚合 SSE 端点（复用 ChatStreamEvent 协议 + task_id 归集），面板实时滚动显示子智能体思考/工具调用/中间输出
- **并发可配置**：[agent] 新增 max_background_concurrency（委托同时运行，默认 20）、max_background_queue（委托排队上限，默认 40，双信号量排队模型——排队满才拒绝）、max_command_concurrency（终端命令同时运行，默认 16）
- **数据有界**：后台任务注册表改 SQL 权威（内存只留活跃/排队任务，终态落 SQLite）+ 3 天 TTL 逐出；主会话删除级联删子会话；子会话总数超 300 惰性清理最不活跃主会话的子会话
- **修复后台任务并发控制失效**：原实现并发许可在 spawn 后立即释放（信号量形同虚设）——许可随任务闭包持有，任务结束释放

详见 [ADR-026](../architecture/decisions/026-background-tasks-unified-panel.md)。

## 0.2.6 修复（自演化综述可靠性 + 任务失败可见性 + 默认工作区常显）

- **修复自演化综述从未产出过任何产物的根因**（8/19、8/31 两次运行均无演化报告/规则/记忆落盘，任务以"综述输出解析失败：综述输出中未找到 JSON"告终）：综述智能体偶尔以散文回复（如 24 token 的"本周期无需变更"）或把 JSON 写入剪贴板工具而非回复文本，严格 JSON 解析即整体失败。三层修复：
  - `parse_llm_json` 增加平衡花括号提取回退——前言/后记/围栏夹杂的混排文本中正确提取首个 JSON 对象（字符串内花括号与转义引号正确跳过），旧契约"前言 + JSON 返回 None"正是本次失败形态之一
  - 解析失败降级为空计划（warn 日志 + 综述原文首行截断进摘要）：演化报告与水位线照常落盘、任务不再整体失败，失败可回溯、下周期可续跑（回归测试锁定）
  - 综述提示词明确禁止用工具输出结果，最终回复必须直接是 JSON
- **任务失败可见性**：调度器 `run_count` 此前无条件 +1 且失败信息只进日志——洞察页显示"自演化综述 1 次"实际可能是失败。`TaskStatus` 新增 `last_error`（持久化，跨重启可见），洞察页任务行显示失败图标 + 错误摘要（hover 看全文）；新增 `executing_task_id`（当前正在执行的任务），页面显示"执行中"徽标——消除"启动后看到摘要生成执行了 1 次而自演化综述没动"的错位感（实为串行执行中，综述尚未完成计数）
- **洞察页表头纠错**："Cron 表达式" → "执行间隔"（ADR-024 改间隔制后的遗留错误标签）
- **移除检索轨迹功能（ADR-025 / REJECTED #20）**：只覆盖上下文组装路径（同 query 的规则/记忆两条轨迹近乎相同，呈现为"重复记录"）、不覆盖 agent 主动的 `search_vfs` 路径、整个生命周期零有效使用。移除 `RetrievalTrace` 类型/构建器/表/`GET /retrieval/traces`/GUI 面板与侧边栏入口；保留 UsageStats（搜索热度/文档命中）与 tracing 检索日志
- **默认工作区始终显示**：会话列表默认分组（未绑定工作目录）此前在无会话时整体消失，作为工作区归属锚点应常显（空组可 hover「＋」直接新建会话）

详见 [ADR-025](../architecture/decisions/025-remove-retrieval-traces.md) 与 [REJECTED #20](../architecture/decisions/REJECTED.md)。

## 0.2.5 修复（todo 工具批量语义 + 完成条目保留展示）

- **批量操作**：todo 工具 create 接受 todos 数组一次写整份清单（不再逐条调用）；update 接受 updates 数组一次合并多条状态变更（开始/完成多条一次调用）；delete 接受 ids 数组；单条形态保留兼容。存储层 create_many/update_many/delete_many 单次落盘，update 原子（任一 id 不存在整体不上盘）
- **子待办**：parent_id 挂靠父待办（校验父存在且属于本会话），面板缩进展示——发现新工作/拆子任务随时追加
- **修复「todo 不存在或不属于当前会话」误报根因**：存储懒加载竞态——create/update/delete 不触发磁盘加载，新进程首个 create 会把既有条目全部覆盖；读盘移入写锁内双重检查（ensure_loaded）
- **修复后台终端命令对面板不可见**：execute_command(background) 启动的后台命令（cmd_*）只存在于 agent 内部注册表（task_status 可查），「后台任务」面板读的是委托任务注册表——实测"明明有后台实验在跑，面板却空空如也"。合并两套注册表为统一视图（BackgroundTask::from_command_task），面板显示终端/委托类型徽标，取消按钮对终端命令走进程树终止
- **修复整点定时任务触发的整进程闪退**（8/29 20:00、8/30 20:00、8/31 14:00 三次，WER BEX64 fail-fast）：根因链 = 摘要任务（cron 0 0 */6 * * *，UTC 语义 = 本地 8/14/20/2 点）发现待摘要条目 → 嵌入请求未携带 dimensions 参数（API 按模型默认 1024 维返回）→ 写入 LanceDB 时与配置 vector_dimension(3072) 的建表维度不一致 → arrow FixedSizeList 内部 panic → release panic=abort 整进程即刻终止且日志无任何输出。修复：嵌入路径按向量库建表维度显式请求 dimensions（index_entry 与查询路径，VectorStorage 新增 embedding_dim 声明）；upsert/查询前置维度校验，不匹配转为带行动指引的干净错误（回归测试锁定），杜绝此类 panic 再次发生
- **调度模型从 cron 改为间隔 + 补跑（ADR-024）**：个人 PC 服务不常驻，cron「某时刻执行」宕机即永不执行且 UTC 语义造成本地触发时刻漂移。改为间隔制——任务声明执行间隔，单一扫描循环每 60 秒顺序检查，「距上次执行 ≥ 间隔」立即执行；last_run 持久化（scheduler_state.json），宕机超期任务重启后自动补跑一次；全任务串行消除资源争抢；顺带修复边界双跑 bug（每任务在整点 ±1s 各执行一次）。用户自建定时任务同步迁移（旧 cron 自动换算），schedule_task 工具与「定时任务」创建表单改为间隔语义
- **完成条目保留展示**：面板不再过滤已完成条目——划线 + 对勾保留展示（可回看），无任何条目后面板才消失
- **面板宽度对齐输入框**（max-w-4xl 居中，与 composer 一致）

详见 [ADR-022 修订](../architecture/decisions/022-session-bound-task-ux.md)。

## 0.2.4 修复（流式静默中断，断流根因排查）

- **根因确认**：断流为 opencode 网关→GLM 上游连接中断（网关以非标准 finish_reason=network_error 结束流），DSH 同模型同供应商同样复现——非天演独有；但天演此前叠加了两个自身问题使断流"静默化"
- **客户端 120s 总超时（天演独有断流源）**：provider timeout 曾作为 reqwest 总请求时长限制，长思维链轮次超 120s 即被客户端掐断 → 改为读空闲超时（字节间静默才计时），活跃流总时长不受限
- **network_error 结尾块被静默丢弃**：非标准 finish_reason 反序列化失败即跳过 → 显式上抛并携带原始原因（与 DSH 报错可见性对齐）
- **提示只闪 3 秒**：中断标记补全，消息下方持久的红字"流式中断，已保留部分输出"提示
- **文件日志 0 字节**：文件层与 RUST_LOG 解耦，GUI 实例日志不再静默归零
- **输出上限链路确认**：设置页模型 output 限额（max_output_tokens）经 T6 动态预算正确生效（请求 max_tokens = min(配置值, 上下文预算)），前端 2048 参数不参与

## 0.2.3 修复（数据目录搬迁重做，ADR-023）

- **配置目录与数据目录分离**：配置文件固定为 `~/.tianyan/tianyan.toml`（不再多路径搜索，`TIANYAN_CONFIG` 环境变量仍可覆盖）；数据目录从配置 `storage.data_dir` 读取，两者彻底解耦
- **搬迁语义修正（真"搬"）**：搬迁忽略配置文件（历史安装中配置与数据同目录时，配置留在原地）；复制后逐文件校验（路径集合 + 字节大小）；**强制删除源数据条目**——被占用文件作为无害残留报告，绝不静默留下两份全量数据
- **顺序修正**：先更新配置再删除源（旧实现的"先删源再改配置"曾导致配置文件被自己搬走、更新失败、整体回滚——正是 0.2.2 中"搬迁后目录显示不变、两份数据并存"的根因）
- **失败可见**：搬迁后前端比对重启后配置的 data_dir 与所选新目录，回滚时明确报错而非误报成功

## 0.2.2 修复（0.2.0 验证反馈；0.2.1 因打包嵌入旧前端资源作废）

1. **数据目录搬迁交互**：数据目录改只读展示；「数据搬迁」按钮 → 对话框选择新目录（原生目录选择器/手动输入回退）；确认后系统校验目标目录为空（非空就地报错，不触发关停）再开始搬迁
2. **移除「计划」全局栏目**：todo/goal 是会话内推理辅助工具——数据按会话绑定，会话页输入框上方停靠展示活跃条目，完成即隐（对齐 DSH）；删除会话级联清理
3. **移除「任务」混合栏目**：内置任务并入「洞察」；定时任务独立一级栏目；后台任务移入会话页停靠条（运行中可取消，完成即隐，结果由主 agent 汇总进会话流）

详见 [ADR-022](../architecture/decisions/022-session-bound-task-ux.md)。

## 0.2 新增功能

### 会话待办/目标（todo/goal，会话绑定）
- **待办清单**：创建/完成/删除/优先级/关联目标，持久化 `todos.json`
- **目标**：长期目标，进度按关联待办完成比例自动计算（与 todolist 联动）
- **会话绑定 + 临时语义**（对齐 DSH）：todo/goal 是智能体的会话内推理辅助工具——数据按归属会话存储（`session_id`），会话页输入框上方停靠展示当前会话的活跃条目，全部完成后面板消失；**无全局「计划」栏目**；删除会话级联清理其 todo/goal

### 数据目录搬迁
- 设置页「数据存储」：数据目录**只读展示** + 「数据搬迁」按钮 → 对话框选择新目录（桌面端原生目录选择器 / 浏览器回退手动输入）→ 确认后系统校验目标目录为空（非空 400 就地报错，不触发关停）→ 自动完成（关 DB → 复制数据 → 更新配置 → 重启）
- 修复优雅关停循环引用（agent → 工具 → 调度器 → 统计 → SQLite），文件锁得以解除
- 失败自动回滚，数据不丢失

### Web 搜索配置 UI
- 设置页新增「Web 搜索」tab：后端选择（DuckDuckGo/Bing/SearXNG）+ SearXNG 端点 + 超时/缓存/抓取上限
- 配置向导新增「Web 搜索」步骤

### 待办/目标工具化
- 新增 `todo`/`goal` 动态工具：agent 可自主创建/更新/跟踪/修复待办与目标（会话绑定：创建归属当前会话，list/update/delete 只作用于本会话条目）

### 任务语义分层（0.2 修复后）
- **内置任务**（调度器 cron：摘要/GC 等）→ 在「洞察」栏目展示
- **定时任务**（到点调用智能体的周期 AI 工作）→ 独立一级栏目「定时任务」
- **后台任务**（会话发起的委托/终端任务）→ 会话绑定，在会话页停靠条展示（运行中可取消；完成即隐，结果由主 agent 汇总进会话流，ADR-013）

## 架构重构（0.2 之后，2026-08-30）

### 循环引用彻底修复（分层 + 解耦）
- 定时任务链：handler 经 `TaskResultSink` 接口回写（Weak）、`ScheduleTaskTool` 经 channel 解耦（不依赖 manager 类型）、manager 经 `TaskRegistrar` 接口注册——依赖单向向下，**不再依赖优雅关停特殊处理**（迁移验证旧目录完全清空）

### 分层重构（阶段 1-3）
- **基础类型层**：`roles`（角色纯类型）、`common`（含 RetrievalTrace）——config/agent/scheduler 共用，打破 `config↔agent`、`scheduler↔agent` 环
- **统一写入门面**（ADR-020）：`db::Database` 门面（单连接 + schema 集中）+ 业务域 Repository（stats/trace/execution/usage）；8 个组件不再各自持 SqliteDb
- **SQL 收敛**：Session/Stats/Trace/Execution/Usage 五域数据访问收口到 Repository/本模块（会话专属存储归 ADR-018）
- **db 纯底层**：SqliteDb 移入 db、SessionRepo 归位 session、RetrievalTrace 引用 common——**生产代码零模块环**（文件级 SCC = 0）
- **依赖清理**：cargo-machete 移除 7 个未使用依赖（core: anyhow/clap/diffy；server: tracing-subscriber；tauri: tower/tower-http/uuid）

## 0.1 主要功能（回顾）

### 智能体
- **对话与工具调用**：流式响应、思考过程、工具调用（文件/代码/搜索/网络/委托/定时任务）
- **编辑链路**：`apply_patch`（主力，unified diff + 上下文锚定，对齐 omo/Codex）+ `apply_edit`（内容匹配，极小改动）
- **子代理委托**：并行/嵌套委托（深度上限 3）、后台任务、完成通知
- **定时任务**：真实 cron 语义，可安排周期性工作

### 知识体系（VFS）
- **统一存储**：文档/记忆/规则/技能全部经 VFS（L0/L1/L2 双层摘要 + 向量检索）
- **自动注入**：soul/rules/memories 前缀按查询检索注入（会话首轮冻结，命中前缀缓存）
- **按需检索**：`search_vfs` 语义搜索整个 VFS（文档/记忆/规则/技能）
- **自进化**：技能/规则随使用自动学习（GEPA）

### 桌面应用
- **托盘常驻**：关窗最小化到托盘，后台任务继续运行；`Ctrl+Alt+T` 唤起
- **系统通知**：后台任务完成/审批挂起/主动提醒 → 桌面通知
- **剪贴板**：监听捕获 + agent 写出
- **自动更新**：启动检查 GitHub Releases，用户确认后安装

### 工作区
- 文件树/查看器/编辑、diff 面板、LSP 诊断、测试发现、构建验证

## 已知问题

见 [known-issues.md](known-issues.md)。重点：
- 模型生成截断工具调用（已护栏）
- apply_edit 精确匹配脆弱（引导用 apply_patch）
- 会话内规则/记忆不刷新（ADR-012 缓存优先，可用 search_vfs 检索）

## 安装与使用

### 桌面应用（Tauri）
1. 从 GitHub Releases 下载安装包（`tianyan_0.2.0_x64.msi`）
2. 首次运行 SmartScreen 警告时点"仍要运行"（自签名）
3. 启动后自动检查更新

### 本地服务（开发模式）
```powershell
cargo run -p tianyan-server   # 后端 http://127.0.0.1:3000
cd gui-vite && npm run dev    # 前端 http://localhost:5100
```

### 配置
- 配置文件：`tianyan.toml`（模型 Provider/Model、存储、安全、日志、Web 搜索）
- 数据目录：`~/.tianyan`（SQLite 会话 + VFS 内容 + 快照）

## 发布流程（维护者）

1. 更新版本号（workspace Cargo.toml / tauri.conf.json / gui-vite package.json）
2. 推送 `v0.2.0` 标签触发 GitHub Actions 发布流水线
3. 流水线产出 `.msi` + `.sig` + `latest.json` 上传到 GitHub Release
4. 用户启动应用时自动检查更新
