# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **独立 server 端口可配（`TIANYAN_PORT`）**：`server/src/main.rs` 由 `ServerConfig::default()` 改为 `ServerConfig::from_env()`——不设该变量时行为完全不变（默认 `127.0.0.1:3000`），设了则覆盖端口（非法值警告并回退默认）。动机：本地 3000 常被运行中的桌面应用占用，GUI e2e 需要一个不与之冲突的端口；顺带让独立 `tianyan-server` 的用户可换端口。解析抽为纯函数 `from_port_str` 并附判别力测试（合法 / 带空白 / 非法 / 空串 / 越界 / 缺失）

### Changed
- **GUI e2e 后端不再使用 3000**（`gui-vite/playwright.config.ts` 的 `E2E_PORT`，默认 3099，`TIANYAN_E2E_PORT` 可覆盖）：此前 e2e 与桌面应用抢同一端口，而后端条目 `reuseExistingServer: false` 会因端口冲突失败——等于「想跑 e2e 就得关掉自己的应用」（并掐断正在进行的会话）。vite 经 `TIANYAN_API_TARGET` 代理到 e2e 端口；`scripts/test.ps1` 的端口预检同源读取，避免两处默认值漂移
- **`ask_user` 参数收敛为单一形态（破坏性变更，对齐 DSH `ask_user_question`）**：`AskUserParams` 此前并行提供顶层 `question` + `options`（单问题快捷）与 `questions[]`（多问题分步）**两种写法**——同一意图两套 schema，且 `question` 是「必填却在 `questions` 形态下被忽略」的字段。**形态分叉是模型生成工具参数的主要抖动源**（同一份意图要在两套结构间选写法；实测该工具的参数生成失败全部集中于此——`options` 被退化成「字符串化的 JSON」）。现收敛为：**只有 `questions` 数组，单个问题 = 长度 1 的数组**。同步面：core schema（`AskUserParams`）/ 前端解析（`parseAskUserQuestions` 删除单问题回落分支）/ e2e 夹具（`mock-llm.mjs`）/ 测试夹具（`agent_ops_tests`、`loop_tests`、`agent_core` 测试辅助）；新增「旧扁平形态必被拒」判别力测试。`/chat/answer` 回答通道结构不变
- **`todo` 参数收敛为单一形态（破坏性变更）**：删除 update/delete 的「单条快捷形态」（顶层 `id`/`title`/`description`/`priority`/`parent_id`）——要改/删**单条也放进数组**（长度 1），与 `ask_user` 同型收敛；create 早已收敛（只接受 `todos`）。顺带修掉一处「实现的功能被 schema 藏起来」：`list` 实际支持 `status`/`goal_id` 过滤，但这两个字段的 schema 描述写的是「update 单条形态」→ 现**语义单一化**为 list 过滤专用并写进描述（此前无文档、无测试覆盖）。新增判别力测试：旧单条形态（`{"operation":"update","id":…}`）必被拒 + 数组写法（长度 1）可用 + list 过滤回归

### Fixed
- **三处 e2e 断言失效（既有漂移；CI 不跑 e2e，故长期未被察觉）**：① `chat.spec.ts` 把 `POST /chat/stream` mock 成 SSE body，而该端点自 ADR-028 已收敛为「开关」（返回 JSON）、前端用 `response.json()` 解析 → 助手回复断言**恒红**（改走历史消息路径；流式端到端归 `real-chat.spec.ts`）；② `real-workspace-session.spec.ts` 同样按 SSE 文本解析响应取 `session_id`（改为按 JSON 解析）；③ `session-page.spec.ts` 仍断言「暂无会话」空态，而该空态已随工作区分组引入被移除（改为断言「默认」分组可见 + 该文案计数为 0）。判别力：旧写法临时探针 1 failed；修复后完整套件 36 passed
- **vite dev 的 `/health` 代理此前从未生效**：该条目被误嵌在 `/api` 的配置对象**内部**，而 http-proxy 只识别 proxy 的**顶层**键——关于页版本展示 / 连接测试在 dev 模式下因此不达后端
- **文档漂移**：`/chat/stream` 仍被描述为「流式聊天 (SSE，含 chunk_type)」（`module-descriptions`、`system-architecture` 数据流图），与其「开关」语义矛盾；「独立 server 固定 3000 / 不支持改端口」的强断言同步为「默认 3000，`TIANYAN_PORT` 可覆盖」（AGENTS.md、README、troubleshooting、refactoring-practices、module-descriptions）

## [0.5.7] - 2026-09-20

### Fixed
- **嵌入服务非标准 usage 形状导致语义检索全线失败（用户实测 `search_vfs` 报错）**：DashScope `text-embedding-v4` 兼容端点只回 `usage.total_tokens`，而 async-openai 0.34 的 `EmbeddingUsage.prompt_tokens` 是**必填**字段（无 `#[serde(default)]`）→ 类型化反序列化在**库内部**失败（`missing field 'prompt_tokens'`；报错列号落在 JSON 解析终点，故「向量看着完全正常」的假象）→ 向量被整次丢弃。影响面：`search_vfs` 语义检索、知识库导入向量化、L0/L1 摘要向量、`DualLayerRetriever` 检索、记忆写入向量化。修法：embedding 绕开库类型化（手写 `POST /embeddings`；复用同一 http client / headers / `RetryPolicy` / `classify_http_status` 单点，**不新增请求封装层**）+ usage 宽容解析（缺 `prompt_tokens` 以 `total_tokens` 兜底）。附**成因锁定测试**（断言库类型化路径在该形状上必失败——上游修复后该测试转红，提示可回归类型化）
- **Markdown 列表渲染凭空多空行（用户报告：二级无序列表 / 有序列表）**：hast 会给含块级子元素的 `li` 在首尾插入 `\n` 文本节点（嵌套列表的「文本 → `<ul>`」之间、loose 列表的 `<p>` **前后**），而 U4 的 `[&_li]:whitespace-pre-wrap` 把这些 `\n` 渲染成**整行空行**——嵌套列表块高翻倍；loose 有序列表的前导 `\n` 更让 marker（`1.`/`2.`）与正文分成两行。修法：pre-wrap 只落在**不含块级子元素**的 li（`li:not(:has(p,ul,ol,pre,blockquote,table,hr))`）；含块级子元素时其段落软换行仍由 `[&_p]` 保证。浏览器实测：嵌套列表 248→121px、loose 有序列表 345→124px（每项 marker 行 47px→0）

### Added
- **provider wire 方言单点（ADR-038）**：各「OpenAI 兼容」实现的字段级差异收敛为**单一描述符** `ProviderDialect`（思考字段名 / 缓存命中字段 / 思考参数形态 / 嵌入 usage 形状），构造时 `resolve_dialect()` 解析一次并缓存、请求路径零判定；`DialectPreset` 预设表（`openai_compatible` / `deepseek` / `dashscope` / `ollama` / `custom`）——**新增服务商 = 加配置不改代码**（OCP 判别测试：假想 provider 零 Rust 改动接入）。粒度两级：provider preset + `ModelEntry.thinking_param` 模型级覆盖（Qwen 思考族差异是模型级，provider 级表达不了）。分层纪律：**静默失败项**（`thinking_field` / `thinking_param`）必须显式可配；**可见失败项**（`cache_field` / `embedding_usage`）声明首选 + 容错兜底（避免把「可见失败」改造成「配错就静默丢数据」）

### Changed
- **apply_patch 路径口径统一（ADR-009 补记）**：绝对路径按字面归一后使用（**不再拒绝**，与 `apply_edit` / `read_file` / `write_file` 一致）；安全检查与落盘**同源**——registry 与 executor 共用唯一定点 `patch_targets`（同一 `parse_patch` + 同一 `resolve_patch_path`），删除第二套路径提取 `collect_patch_paths`（其一致性此前只能靠人工维持，**那才是「拒绝绝对路径」想收窄却收窄不了的分歧面**）。`..` 仍在解析处直接拒绝（比白名单判定更早的深度防御，覆盖未经 `check_path` 的调用方）。顺带修正：LSP 诊断改传归一后的绝对路径（store 按绝对路径索引，此前传补丁原文的相对路径永远命中不到）

### Docs
- ADR-038 provider wire 方言（新建）+ ADR-009 补「路径口径统一」+ module-map / AGENTS.md 导航 / `config.example.toml` 方言与 `thinking_param` 示例
- **CI 产物同步**：`apply_patch` 工具描述补路径口径后重新生成 `docs/architecture/tool-catalog.md`——freshness 门禁在 tag 阶段抓到（本地手工跑 `cargo fmt` / `clippy` 不覆盖该检查）。已在 AGENTS.md「常见陷阱」标注该纪律，并把 `.\scripts\test.ps1 lint` 定为验收入口
- `scripts/test.ps1` 的 clippy 补 `--all-targets`（与 CI 同范围，消除「本地绿 / CI 红」——此前 `f67b642` 即该差异所致）

### 回归保护
- core **1374**（+27）· 前端 **424** · `cargo check --workspace --all-targets` / fmt / clippy（`--all-targets`）干净 · 判别力：方言基线等价（阶段 0 锁定后原样通过）· 嵌入成因锁定 · OCP 零代码接入 · 检查产物 == 写入位置

## [0.5.6] - 2026-09-20

### Fixed
- **任务取消「假成功」（用户实测 0.5.5 安装后）**：`task_cancel` 只路由到委托任务表，对**命令任务**（后台命令/长驻服务）静默 no-op 却返回成功——面板任务卡片卡在「运行中」不消失（取消了个寂寞）。修法：**双表路由**（先委托表，否则命令表并 kill 进程树；两表都无 → 明确 `not_found`，不再假成功）。新增 2 测试（命令任务真进 Cancelled / 未知 id 报未找到）
- **工具目录跨平台 freshness 假红（CI 必红项）**：`tool_catalog --check` 的平台归一化用**未转义**提示词模板替换，而产物表格把描述里的 `|` 转义为 `\|`（PS 系提示含 `&& / ||`）→ Linux（CI）比较时误报漂移（Windows 同平台两侧同文本而侥幸通过）。修法：归一化同时替换 `\|` 转义形态 + 判别力测试覆盖两种形态
- **Linux/CI 测试互斥（Quality 恒红根因）**：`kill_all_running_children` 按全局注册表杀全部子进程（生产语义正确），并行测试下误杀兄弟测试的命令 → Linux 上 2 条测试失败（Windows 侥幸绿）。修法：15 个「启动真实子进程/操作注册表」测试加互斥锁（串行，其余保持并行）→ Linux 复刻全绿

### Added
- **命令执行底层可配（ADR-037 `[executor].shell`）**：shell provider 单一事实源（`ShellSpec`：执行信息与提示词同源，杜绝「配置了 A 提示还说 B」的漂移）——`auto` 探测（Windows pwsh→powershell / Unix bash→sh，命中记绝对路径）+ 显式枚举（`pwsh`/`powershell`/`cmd`/`bash`/`sh`）+ `custom` 逃生舱（`shell_program`/`shell_args`/`shell_hint`）。找不到可执行**明确报错不静默回退**（静默回退会让用户以为在用 A 实际在用 B）；提示词随配置动态化（PS 5.1 明确标注不支持 `&&`/`||`）；**PS 系输出编码注入**（`[Console]::OutputEncoding=UTF8`——修 git/rg/cargo 中文输出乱码）；切换代价（一次前缀缓存 miss + 一次主动压缩）在配置注释中明示
- **`write_file` 目录语义**：新增 `create_dirs` 参数（默认 `false`）——父目录不存在时**默认报错**并回喂修法指引（"如确需新建目录，请重新调用并传 `create_dirs=true`"），**不自动创建**（防路径写错时静默「凭空多出一棵树」）；显式 `true` 时创建父目录后写入。`apply_edit`/`apply_patch` 不暴露该开关（语义上文件/工程结构已存在）

### Docs
- ADR-037 shell provider（决策固化）+ ADR-009 补「写入目录语义」+ AGENTS.md 导航更新
- `scripts/wsl-core-test.sh`：Linux 复刻脚本改**按模块分批**（本机 WSL 一次性跑全量会让实例崩溃 `Wsl/Service/E_UNEXPECTED`；分批稳定且覆盖全量 1347）

### 回归保护
- core **1347**（+3）· Linux 复刻分批 **1347** 全绿 · `tool_catalog --check` 两侧一致 · fmt / clippy 干净

## [0.5.5] - 2026-09-19

### Fixed
- **取消粒度（用户实测「停止按钮有时按了没反应」，ADR-036）**：取消置位链路此前已统一，但响应侧只有同步检查点（3 处），三处 await 阻塞盲区不可取消——A. LLM 请求在飞（等响应头；长思考模型 10–30s+）；B. 上下文组装/压缩（检索/压缩 LLM 调用无取消传导，15s 延迟实证）；C. 流式 recv 间隙（检查点在 `rx.recv()` **返回之后**——无新 chunk 时取消永不生效）。修法（统一语义「drop 即取消」）：新增 `agent::cancel` 等待原语（`wait_cancelled` 50ms 粒度 + `cancellable` 竞争，**预置位快速路径不启动新工作**），五处等待点统一套用（流式请求发送 / 流式 recv / 组装 / 压缩轮前+轮末 / 非流式请求）；model producer 补 `tx.closed()` select（接收端 drop → 立即退出并断连，旧实现挂到下一块数据或 read_idle）；**收尾语义零新增**（interrupted 前缀落库 / `Cancelled` / 跳过压缩，不回改已完成轮结果）。判别力实证：recv 等待间隙取消（旧实现挂死）· producer 断连（旧实现等 read_idle=30s）· 轮末取消跳过压缩（旧实现必红）——修复前全红、修复后全绿
- **长驻服务卡唤醒（用户报告「启动后端服务+两个测试，因服务存续导致永远不唤醒收尾」）**：就绪即完成（B 方案）——`CommandTask.counted` 新字段 + `release_wait_count`（就绪/探测超时释放待完成计数、幂等、触发唤醒）；终态递减只对 counted 任务。长驻服务不再算「待完成」，唤醒正常收尾
- **轮级失败持久横幅（LLM 请求失败不再「莫名中断」）**：error 事件置位会话级 `turnErrorBySession`（纯前端内存态），输入区上方持久横幅「上一轮中断：{原因}」+ 手动关闭 + 发送新消息自动清除

### Added
- **前端「正在停止…」过渡反馈**：点击停止 → 后端收尾完成（`turn_state idle`）或端点返回 `no_active_stream` 之间，横幅 + 停止按钮禁用态（`stoppingBySession` 纯内存态 + 20s 兜底）

### Docs
- ADR-036 结构性取消（决策固化）+ REJECTED #21（否决「取消 = abort 整个轮 future」——丢失收尾语义）+ AGENTS.md 导航更新

### 回归保护
- core **1332**（+10：cancel 原语 7 + recv 等待取消 + producer 断连 + 轮末跳过压缩）· 前端 **423**（+2）· fmt / clippy / tsc / prettier 干净

## [0.5.4] - 2026-09-18

### Fixed
- **多会话流式串扰（用户实测）**：store 的 `setMessages` / `deleteMessagesFrom` 把"写到哪里"隐式绑定到 `currentSessionId`——切会话加载历史 / 回退操作的 `await` 期间跨越用户切换时**写入错误会话**（实测症状：切进当前会话时刷出另一会话的流内容；数据库无污染，纯前端 store 层）。修复（写入寻址显式化）：删除 `setMessages`（唯一写入路径 = `setSessionMessages(sessionId, …)`）；`deleteMessagesFrom` 加会话 id 参数；懒加载收口 `loadSessionHistory(sessionId)`（唯一入口：双重检查 + 流式中不覆盖，与快照同规则）；回退/重做在发起时捕获会话 id 写回。`updateLastMessage` 加 role 守卫（只写最后一条 assistant——注释与实现漂移纠正）。判别力实证：3 条红测试（慢加载切换污染 / 回退中途切换写错 / 末条 system 增量污染）修复前全红、修复后全绿

### Docs
- **任务持久化边界澄清（防漂移）**：命令类任务（`CommandManager`）**有意不落 SQL**（进程内保留、重启即清空，属预期——长会话任务全部落库会争夺注意力）；任务面板**有意仅显示当前会话**、不做跨会话聚合。落点：ADR-026 §4/§5「D 边界澄清」、ADR-013 交叉引用、module-map、三处代码注释（command.rs / coordinator.rs / AgentTasksPanel.tsx）

## [0.5.3] - 2026-09-18

### Fixed
- **跨会话压缩摘要串号（用户实测取证）**：会话 B 的压缩点摘要混入会话 A 的整段历史并落库，此后每轮被注入模型上下文。根因：`ContextCompressor.cached_summary` 是**进程级**可变状态、不按会话隔离——A 压缩后缓存其摘要，B 压缩把它当 `existing_summary` 增量合并。修法（数据源单一化）：删除 `cached_summary` / `cacheable_summary`（压缩器变为无会话状态），已有摘要改由调用方从**当前会话链最后一个 `compression_marker`** 提取（`ContextPipeline::extract_marker_summary`，剥展示包装）后显式传入；`compress()` 恒走全量摘要；空摘要语义保留（空结果不落库 → 天然不会成为「已有摘要」）。判别力实证：`test_cross_session_compress_does_not_reuse_previous_summary` 修复前红、修复后绿。同型「进程级共享可变状态」复核：`cached_soul` 全局单例（语义正确，无须按会话）、`injectable_context` / `injectable_snapshot` 已按会话（会话头持久化，ADR-012）、`pending_approval_fingerprints` 仅审批面板展示不进模型上下文。历史污染摘要按用户指示**保持原样**（未清理）
- **T1 审计批次（系统性审查 13 项）**：
  - **T1-6** `apply_patch` 模糊定位下静默删改 → 删除行内容校验（位置敏感守卫）
  - **T1-9** MCP 连接健康检查「在册≠可用」→ 真探活 + 死连接自动重连
  - **T1-10** `POST /chat/stream` 同会话并发未保护 → 重复发流返回 409（检查与插入同锁原子），不再覆盖取消槽（「停止」失效 + 两轮并发写同一会话）
  - **T1-12** 删会话残留快照目录 → 清理 + GC 清理空会话目录
  - **T1-13** `doc_load` 统计不落库 → 落库 + 刷盘扣减
  - **T1-14** LanceDB 查询默认 top-k=10 静默截断 → 显式 `limit`
  - **T1-15** 工具写入非原子（崩溃留半截文件）→ 临时文件 + rename 原子替换（`write_file` / `apply_edit` / `apply_patch` 三条路径）
  - **T1-16** 写类工具风险定级可被工具选择绕过 → 统一关键路径 High
  - **T1-17** 会话加载单行损坏废整个会话 → 逐行容错
  - **T1-18** 向量与内容可能失配 → 对账通道（缺索引重建 + 孤儿点清理），接入 GcTask
  - **T1-19** `trace_spans` 无界增长 → 按保留窗口清理
  - **T1-21** 端口变化后前端事件流不重连 → 重建 EventSource
  - **T1-22** web 响应 UTF-8 截断导致整个 fetch 失败 → 按字符边界容错
- **T2**：web 结果缓存加上限（256 条，淘汰最旧）· 单行超长输出的头/尾截断不再退化为零内容
- **CI（Linux / WSL 复刻门禁）**：修复 3 个只在 Linux 暴露的测试失败（`~` 展开落在测试黑名单、快照 hash 缓存撞同 mtime+size、`kill -9 -<pgid>` 返回成功但进程仍存活）→ `kill_process_tree` 三重保险（组 → 进程 → 子进程 pkill）+ 取消/超时 `child.wait()` 2s 兜底 + 等待改 `try_wait` 轮询；`hide_console_window` 的 `mut` 按平台裁剪（修 Linux clippy `unused_mut`）

### Changed
- **质量门禁改为 tag / PR 触发（用户要求「只有打 tag 才编译构建」）**：`quality.yml` 此前 `on.push.branches=[master]` 每推一次都跑 fmt/clippy/test/前端检查；现与 `release.yml` 一致改为 `push tags v*` + `pull_request`——**master 推送不再触发构建**
- 删除 `skills/learning` 死代码（~1100 行）——无生产消费方，且弃用分支会覆盖写销毁技能
- agent 契约漂移三处修正：`rollback_session` 文档分工 / 删死 API `has_pending_approval` / 删无效 `background` 参数
- 回归保护：core **1321** · server **166**（1 ignored）· mcp **18**；clippy 0（workspace 全目标）/ fmt 干净；判别力实证 T1-10 / T1-15 注入后转红（见各提交说明）

## [0.5.2] - 2026-09-17

### Fixed
- **退出卡死（用户报告）**：点托盘退出后图标与进程残留（任务管理器剩 webview + cargo 子进程），只能强杀；此前"后台任务跑着时退出"同款。根因三条链：① 关停只置位 `shutdown_flag`，而它的唯一消费者是 `/tasks/stream`——正在跑 agent 轮的 `/chat/stream` 不读它 → axum 优雅关停死等活跃连接；② `server_handle.await` 与 tauri `rt.block_on(supervisor)` 均无上限 → 监督循环不结束 → `app_handle.exit(0)` 根本不执行；③ Windows 无 Job Object 绑定时父进程退出不带走子进程。修法：关停先 `cancel_all_active_turns`（遍历会话取消槽全部置位，跑到一半的命令被杀）→ `kill_all_running_children`（前后台子进程按进程树统一清理，全局注册表 + `ChildGuard` 自动注销）→ server 侧 5s / tauri 侧 10s **超时兜底**，保证一定能退
- **「停止」无法中断正在跑的命令（用户报告）**：取消检查点只在轮顶/chunk 循环，工具执行（`child.wait`）完全不读取消标志。修法：`execute_command_action_cancellable(..., cancel)` 三路竞争（正常退出 / 超时 / 取消），取消即杀进程树并返回「命令已取消」（与 timeout 分开）；工具层从会话取消槽取标志（`session_cancel_flag`）
- **刷盘先清零后落盘（T1-11）**：`usage_stats::flush` 先 `swap(0)` 清空计数器再落库，失败即丢整批统计；`trace::flush` 同形态。改为「读快照 → 落盘成功 → 才扣减（`fetch_sub`）」；trace 失败把 span 放回缓冲
- **CI（quality.yml）Linux 构建失败**：lance-encoding 的 build script 找不到 protoc（release.yml 有、quality.yml 漏配）→ 补 `arduino/setup-protoc@v3`

### Changed
- 回归保护：core **1308**（+5 回归）· server 164 · tauri 13 · mcp 16 · 前端 414；clippy 0（workspace 全目标）/ fmt 干净；判别力实证 4 处（取消注入 / 注册注入 / T1-11 两注入）

## [0.5.1] - 2026-09-17

### Fixed
- **分段加载跨页「调用|结果」分裂（用户实测发现）**：50 条硬切的分页边界约 **50%** 概率切在工具调用与其结果之间（全链 4633 条：92 个页界 46 命中）→ 含结果的页缺调用、含调用的页缺结果 → 结果渲染为孤立"工具结果"卡 + 调用卡 `result=null` 渲染"运行中"转圈（对已完成的历史工具）。现改为**页边界对齐用户消息**：50 条基准窗口 → 页从窗口内由远及近第一条 `user` 起；窗口内没有 → **一次反向查询**找最近一条更早的 `user`（不按批扩展、不设人为上限）；到链头整段兜底。全链模拟：跨页对 **46 → 0**、页首=user **56/56**、覆盖 **100%**
- **apply_patch 静默错位（用户报告 + 工具实现排查）**：块内含空上下文行时，解析层 `line.trim().is_empty()` 把「一个空格」的显式空上下文行也丢弃（与注释语义矛盾）→ 期望窗口少一行 → 精确匹配失败 → 模糊匹配命中"整体平移一行"窗口 → **静默改错位置**（新内容插到函数闭合 `}` 之前，语法破坏且无报错）。现：① 只忽略**完全空行**（`line.is_empty()`），空格前缀（含「一个空格」）保留为 `Context("")`；② 新增**位置敏感守卫** `positional_match_ratio`（逐位置非空行匹配率须达标）——拒"整体平移一行"、保留"个别行抄写误差"容错；守卫不通过则报"无法定位补丁块"（可见失败）
- **后台任务通知前端不实时显示（用户报告，0.5.0 写侧收口回归）**：ADR-028「落库即推送」挂在 `SessionManager` wrapper（只包装 `add_structured_message`），而 0.5.0 把所有写入改经 `ws.append`（工作集内直调 `SessionStore`）→ wrapper 不再是入口 → System 通知与压缩点落库后不广播（用户消息乐观渲染、assistant/工具结果流式推送，故只有通知暴露；0.3.12 的压缩点实时推送同源回归）。现把边界推送挂到 `ws.append`：`BoundaryPushSlot` 共享回调槽（支持后注入，对已加载与新建工作集均生效）+ 同一门控（`System || compression_marker`）；core 仍不感知通道（回调由 server 装配层注入，复用抽出的 `event_push::push_boundary_event`，两处共用避免映射漂移）
- 依赖清理：移除僵尸 `diffy` 声明（core 早已移除、workspace 残留 + core 错位注释）
- **后台命令并发上限失效（T1-5）**：`_permit` 是 `spawn_background` 局部变量，函数返回即 drop（提前释放）——命令仍在运行但许可已归还，ADR-026「16 并发」形同虚设；现 `OwnedSemaphorePermit` 移交 watcher 持有，随命令真正结束（进程退出 → 收输出泵 → 落终态 → 通知）释放
- **LSP 池中死客户端永不复用（T1-7）**：服务器崩溃后读循环置 `dead`，但客户端仍留池中 → 该项目根所有后续查询永久失败（「连接已关闭」）且永不自愈；现复用前检查存活（`LspClient::is_dead` + `LspManager::take_live_server`），死客户端就地逐出并重建
- **apply_patch 工具描述补「块内空行」约定**：完全空行按格式噪声忽略；原文空行作上下文须写成「一个空格」的单独一行（补齐输入侧约定，工具目录同步重生成）
- **日志轮转接线**：`[logging].max_file_size` / `max_files` 此前为死配置（日志文件无限累积）；新增 `RotatingWriter`（按大小轮转、单条日志不跨文件）+ `prune_family`（按 mtime 保留最新 N 个含当前文件）；`core::init_logging` 接线 `[logging].file`；tauri 侧接入
- 回归保护：core **1303**（+6 回归）· server **164** · tauri 13 · mcp 16 · 前端 **414**；判别力实证（apply_patch 两处注入 → 恰好 2 测试红；页边界注入 → 2 测试红；边界推送注入 → core+server 各 1 红；并发上限注入 → 1 红；LSP 注入 → 1 红；日志轮转注入 → 3 红）；clippy 0（workspace） / fmt 干净

## [0.5.0] - 2026-09-17

### Added
- **会话工作集（ADR-035，架构级）**：新增 `core/src/agent/working_set.rs`——`SessionWorkingSet`（会话的**物化上下文缓存**，键 = 会话 id）+ `WorkingSetRegistry`（`ensure` / `get` / `remove` / `lock` / `try_lock` / `sweep_idle`）。定位：**非第二权威**（权威仍是 SQLite，ADR-018/027 不变）、**非新存储**、不引入第二把锁；每轮不再全量重建（连续轮 / 唤醒轮复用同一份段）
- **段（segment）**：工作集只物化「最近压缩点 → 现在」（`start_seq` = 段起点，不变式：段内第 i 条 `seq == start_seq + i`）；段外（压缩点之前）经库按需读；`append` 的 seq 改为**库尾推进**，`delta_since` 按段坐标切片。新增 `reshape_after_compression`（压缩后段起点前移 + 消费水位同一临界区推进）；重建点 = 最近压缩点
- **续跑判定取代通知投递水位**：`ctx.last_seq`（本轮组装所见库尾）vs `ws.last_seq`（工作集所见库尾）——有新消息则**增量注入 + `continue` 自消化**（不入队、不唤醒）。`mark_consumed`（单调 CAS）/ `has_unconsumed` / `delta_since` 为唯一读进度来源，**水位三件套全删**（`notice_delivered` / `NOTICE_WATERMARK_INIT` / `has_pending_notices` / `store_last_seq` / `mark_notices_delivered`）
- **唤醒幂等 `ensure_loop_running`**：同一会话 N 条并发通知**只产生 1 轮唤醒**（`try_lock` + `has_unconsumed`，与 loop 退出判定**同一临界区**，消除"收尾判定无新消息→退出"与"新通知到达"之间的丢通知窗口）；已有 loop 在跑时，通知落库即被该轮消化
- **分段加载（展示层直查库，与工作集解耦）**：`ChatMessage.seq` + `SessionManager::load_before` + `GET /api/v1/sessions/{id}/messages?before_seq=&limit=`（默认 50 / 上限 200；无参保持全量）；前端上滚 `loadOlder`（顶到链首短路，依赖浏览器原生滚动锚定保持视口）
- **`turn_state` 轮状态事件**：用户轮 / 唤醒轮开始-结束**配对**推送 + `auto` 标志（唤醒轮），SSE 透传；前端 `turnState` store 据此联动输入区
- **唤醒轮可停止**：唤醒轮注册会话取消槽（此前 `cancel: None`——"停止"对唤醒轮完全无效）+ `/chat/streams/{id}/cancel` 无用户流时经 `Agent::cancel_active_turn` 置位

### Changed
- **写侧收口（单一写入口）**：四类写入（追加 / 整表重写 / 头部更新 / 删除）全部经工作集（内部先落库 → 再更缓存 → 再派发）；`SessionsService` 五个活跃写路径 + `agent` 层落库（`Agent::persist_structured` / `AgentLoop::persist_turn_message` / 通知器）全部收口；`AppState.working_sets` 与 `Agent` 共享同一 `Arc`（热重载不分裂）
- **快照键统一到消息 ID（弃数字位置）**：`capture` / `restore` / `diff` / `load_tree` 的 `index: usize` → `key: &str`；缓存指针改 `latest.cache.json`；捕获时机挪到**用户消息落库后**；`diff` API 增 `message_id`（`index` 保留为位置映射兼容入口）。段化因此不再依赖"内存条数 == 位置"隐式等式
- **消息按 seq 落位**：前端 `applyServerMessage` 改 `insertBySeq`（修复订阅快照 + 实时事件乱序到达导致的**顺序倒置**）；订阅快照裁剪为最近 100 条 + `has_more` + 游标
- **输入区联动（U10-B 方案）**：唤醒轮（`auto`）运行中**输入禁用 + 发送按钮转停止**；用户轮现状保持不变

### Fixed
- **U10 唤醒轮三症状（用户报告）**：① 唤醒轮停不掉（取消槽缺失）；② 唤醒轮"发了没响应"（无轮状态事件，前端不知道在跑）；③ 消息顺序与库不一致（按到达顺序 append）。三条现由 `turn_state` + `insertBySeq` + 取消槽根治
- **T1-24 重启重放历史通知（机制性消失）**：旧方案靠"投递水位"（水位是内存态，重启归零 → 历史通知全当未投递重投）；工作集落地后**无投递动作即无重放路径**——历史通知只在段里出现一次，无需水位
- **会话 header 被旧值整体覆盖（竞态）**：旧 `update_session` 用 server 侧旧 header 覆盖 → 丢刚固化的 `injectable_snapshot`；现经工作集 `update_header`（仅快照实际变化才同步内存前缀）
- **旁路写补漏**：演化会话清理、定时任务会话重建、`Agent::update_session_header` 三处绕过工作集的直写已收口；`ensure_fresh`（`MAX(seq)` 比较）降级为**架构违规探测器**（warn + 重建兜底，注释显式记录三条检测不到的情形：`rewrite` 等长 / 写操作相互抵消 / 仅 header 变化）
- 回归保护：core **1293** 全绿（0.4.7 基线 1275 → +18）· server **160** · tauri 13 · mcp 16 · 前端 **409**；一致性回归清单 10 条逐条落测 + 段化专项 4 条；**判别力实证 6 处**（注入旧行为 → 必红后还原：续跑判定缺失 / 重建水位归零=T1-24 形态 / 唤醒改排队=9 轮形态 / 快照按位置命名 / `insertBySeq` 改回尾部 append / 段化保留全链）；clippy 0 / fmt 干净

## [0.4.7] - 2026-09-16

### Fixed
- **后台任务唤醒风暴（T1-23，用户报告）**：主轮正常收尾后 3.5 分钟内连跑 **9 轮唤醒**、每轮重复汇报同一批失败（数据实证；模型自述"已在前两轮逐条汇报"）。根因：① 上下文只在轮启动时组装一次，turn 间**不重读历史** → 活动轮期间落库的通知只能等下一个 loop 才被读到（"右侧完成了、主会话没反应"）；② 事件各自 wake 被 turn_guard 排队，主轮结束后逐个重读同一批历史通知。现：**轮边界增量投递**（每 turn 前将新通知尾部追加进上下文，前缀不变）+ **投递水位**（恰好一次，投递与水位推进同一临界区）+ **条件唤醒**（轮收尾仅有未投递通知时补一轮；唤醒入口同判定 → 已消化的排队唤醒直接跳过）。逐条/就绪/通知带指令语义不变
- **run_tests/verify_build 缺省 cwd（T1-1）**：缺省落进程目录（桌面=安装目录）→ 现缺省即会话工作目录，沙箱/审批/执行同一口径
- **唤醒轮失败判定（T1-2）**：跨会话 + 吃 3 天历史终态 → 现限定本会话 + 1 小时时间窗
- **task_cancel 不终止执行（T1-3）**：旧只改面板状态（继续烧 token/占许可）→ 现协作标志 + `AbortHandle` 兜底
- **显式 namespace 检索被意图推断短路（T1-4）**：rules/memories 静默为空 → 现 namespace 直接下推 VFS 搜索
- 回归保护：core **1275** 全绿（0.4.6 基线 1271 → +4）· server 157 · tauri 13 · 前端 406；每条均判别力实证；clippy 0 / fmt 干净

## [0.4.6] - 2026-09-15

### Fixed
- **ask_user 追问跨会话串台（U7，用户报告）**：`pendingClarification` 是 store 单一全局字段（无会话维度）——A 会话追问在 B 会话也弹；在 B 回答会提交到 B，A 的 `ask_user` 永久挂起。现追问绑定来源会话（气泡/提交/滚动跟随均按会话判定）
- **图片裂图（U8，用户报告）**：tauri CSP `default-src 'self'; ...` 无 `img-src` → `data:` URL 被 `default-src` 拦截（粘贴与消息展示同时“裂“）；现 CSP 增 `img-src 'self' data: blob:`
- **新建工作目录分组闪断（U9，用户报告）**：ChatPanel 创建成功后立即清目录 + SessionList 立即撤占位 + `reload` 不返回 Promise（清理与刷新间空窗）；现清理改为数据就绪驱动
- **配置保存段保全（T0-6）**：`save_to_file` 全量序列化覆盖清掉未建模键（自定义段/未来版本字段）；现保存前读取现有文件、未建模键递归合并
- **事件订阅快照补工具结果（T0-7）**：快照（打开/重连的唯一历史路径）走轻量转换（`tool_calls: None`）→ 历史工具卡片永久“运行中”；现抽出完整转换（跨消息合并工具结果）供历史与快照同源使用
- **委托深度守卫改按注册表层级（T0-8）**：旧为“在途计数”——并发兄弟委托被当嵌套层数，**第 4 个并列委托被误拒**；现子代理派生下一层注册表（父 + 1），并发互不累加
- **工具执行历史不再内存累积（T0-9）**：无消费者内存 Vec 线性泄漏（含工具完整输出）且与 `ExecutionLog` 重复存储；现删缓冲与死 API，记录唯一副本走数据层
- **子代理超时不再污染会话级取消标志（T0-10）**：超时置位的是父轮取消标志 → 父轮/后续委托被误取消；现子代理本地标志 + 镜像传播
- **定时任务失败不再记为 success（T0-11）**：失败分支不可达（调度器统计永远成功）；现 `Result` 语义 + `TaskResult::failed`
- **配置热重载重建快照管理器（T0-12）**：热重载沿用启动期实例 → 改工作目录后快照/回退仍在旧目录（或永久禁用）；现按新配置重建替换
- **子会话级联递归 + 上限清理事务（T0-13）**：级联只删一层 → 嵌套委托孙会话成永久孤儿；候选把中间节点当主会话；非事务；现 `WITH RECURSIVE` + 根会话候选 + 单事务
- **run_tests/verify_build 缺省 cwd 归属会话工作目录（T1-1）**：缺省落进程目录（桌面=安装目录）；现会话工作目录，沙箱/审批/执行同一口径
- **唤醒轮失败判定限定本会话 + 时间窗（T1-2）**：跨会话 + 吃 3 天历史终态 → 指令与事实不符；现本会话 + 1 小时窗
- **task_cancel 真正终止执行（T1-3）**：旧只改面板状态（继续烧 token/占许可）；现协作标志 + `AbortHandle` 兜底
- **显式 namespace 检索绕开意图推断（T1-4）**：意图把范围限到别 namespace → rules/memories 静默为空；现 namespace 直接下推 VFS 搜索
- 回归保护：core **1271** 全绿（0.4.5 基线 1256 → +15）· 前端 406 · tauri 13 · server 157 + 集成 21；**每条修复均判别力实证**（注入旧行为必红）；clippy 0 / fmt 干净

## [0.4.5] - 2026-09-15

### Fixed
- **无语言围栏代码块换行渲染（U6，用户报告）**：`pre` 组件透明化（避开与高亮器双容器嵌套）导致**无语言标记**的 ``` 围栏块只剩 inline `<code>`（无 `white-space: pre`），多行内容折叠成一行（目录树 ├── 各行挤在一起；数据层正常，纯渲染）；现 `code` 组件按 ReactMarkdown 约定（块级 children 以 \n 结尾）区分块级/内联，块级渲染自带 pre 语义容器
- **SSRF 重定向校验（T0-3，安全）**：web_fetch 只校验初始 URL，HTTP 客户端默认静默跟随重定向——公网 URL 302 到私网即绕过防护（判别力实证：旧代码成功抓回 127.0.0.1 内网内容）；现每一跳发出前 `validate_public_url`，拒绝则不发出请求
- **摘要刷新检测（T0-4）**：① `processed` 缓存短路在时间戳检测之前——条目一旦处理过，内容更新后摘要/向量永不刷新（直到 FIFO 淘汰/重启）；② `process_uri` 先写摘要后更新向量——向量失败时摘要已“完成”，向量永久缺失（检索漏条目）。现时间戳检测优先 + 反序（先向量、后摘要作提交点，失败下轮自愈）
- **Trace 落盘事务 RAII（T0-5）**：手写 `BEGIN`/`COMMIT` 且 `let _ =` 吞错——COMMIT 失败静默丢数据、INSERT 失败无 ROLLBACK（事务悬挂，`is_autocommit()` 实证）；现用 rusqlite `Transaction`（Drop 自动回滚 + 开启/提交错误上抛）
- **命令分段引号感知（T0-2 补充）**：分段判定不感知引号——命令中字符串字面量里的 `|`（如 `'lint|format|build'`）被切段后段首词恰为黑名单词 → 误拦合法命令（实测天演自身命令被拒）；现引号内不分段（未闭合引号保守退化，不漏检）
- 回归保护：core **1256** 全绿 + 前端 **403** 全绿；主回归先红后绿（判别力已证）；clippy 0 / fmt 干净 / eslint + tsc + build 全绿

## [0.4.4] - 2026-09-15

### Added
- 唤醒轮轮末压缩检查（C1）：`process_wake` 为独立实现（不走 `run_agent_turn`），此前无轮末压缩检查——高负载区间若主要由唤醒轮推进（等子代理报告 / 后台通知轮），上下文持续增长而压缩被无限推迟（实测 60.8%→81.7% 区间零压缩）；现唤醒轮末与用户轮共用同一压缩判定链

### Fixed
- **压缩空摘要静默失败治理（C2）**：生产现象“上下文 82% 未触发压缩”的根因不是未触发——压缩检查已通过、摘要请求已发出，但模型偶发返回空 content（思考模型“想完没说话”）：空摘要被静默丢弃（不落库压缩点、与成功路径共用“上下文压缩完成” info 日志——运维不可见）、空串污染增量摘要缓存、压缩被推迟到下一轮。现空摘要**重试一次**（对齐 AgentLoop 空响应重试，两笔真实消耗合并入账）、空摘要不进入缓存、丢弃时 **warn 告警**且不改写对话
- **路径沙箱 `..` 词法归一化（T0-1）**：`canonicalize` 失败（目标不存在，如写新文件）时回退不折叠 `..`——组件级前缀匹配被 `sub/../..` 欺骗：**白名单逃逸**（旧代码实测把 `allowed/ghost/../../escape.txt` 判为 Allowed）、**黑名单漏报**（`src/../.git/config` 漏过 `.git` 禁令）。现新增 `lexical_normalize`（has_root 语义不越根/盘符）+ `resolve_canonical`（词法折叠 × 最近已存在祖先物理解析，符号链接亦无法欺骗）+ `resolve_tool_path` 判定/执行统一口径
- **命令分段判定（T0-2）**：命令统一经 shell 执行，但安全判定只覆盖“整条首词 + 整条前缀”——**relaxed（默认）模式**下 `git status && rm -rf /` 第二段完全不做黑名单/白名单/解释器检查；**strict 模式**漏检换行分段（`echo hi\nrm -rf /`）；**审批层**危险命令被低估为 Medium（确认门被绕过）。现 `split_command_segments` 切段（`&&`/`||`/`;`/`|`/换行）+ `check_command` 逐段判定 + 换行入元字符检查 + `assess_risk` 逐段取最严
- task_status 工作目录视野（U5）：默认按本会话工作目录过滤（同目录跨会话可见 = 同目录竞争协调面），`scope="global"` 显式查全局（用完即回），跨目录信息不进入汇报；归属未知（旧数据）仍可见
- 回归保护：core **1248 全绿**（0.4.3 基线 1234 → +14 条）；C2/T0-1/T0-2 主回归**先红后绿**（判别力已证）；clippy 0；fmt 干净

## [0.4.3] - 2026-09-15

### Added
- **panic 隔离防线（release 构建行为变更）**：release `panic = "abort"` 改为 **`unwind`**——此前任何单点 panic 都会升级为**全进程无痕退出**（0.4.2 闪退事故的放大器；`catch_unwind` / `JoinError::is_panic` 类防线在 abort 下全部静默失效），且无现场可查；新增 panic hook（`common::panic_hook`）现场落盘 `%APPDATA%/com.tianyan.app/logs/panic.log`（消息 / 位置 / 回溯，防递归设计），tauri / server 两入口安装
- 异步任务纪律（U1）：soul「异步任务纪律」节 + `task_status` 工具描述强化 + 非终态查询返回「无需轮询」提醒（委派后频繁轮询的行为治理）
- 委托结果通知轻量化（U2）：子代理完整结果落盘 `{data_dir}/task_results/{id}.md`，任务通知只携带「结果路径 + ≤300 字摘要」（超长截断 / 界面污染根除）

### Fixed
- **外部链接守卫（U3）**：点击消息中的 http 链接此前由应用 webview 自身导航（无法回退、只能重启）；现插件级 `on_navigation` 守卫——外部 URL 一律取消导航并转交默认浏览器（放行 `tauri.localhost` / dev localhost）；无用途 shell 插件清理 → 官方 `tauri-plugin-opener`
- **消息正文软换行保留（U4）**：文本结构化内容（画线框图 ASCII art）与用户输入多行文本的换行 / 连续空格被折叠（Markdown 软换行 + 浏览器默认 `white-space` 折叠）；现正文渲染对段落 / 列表项应用 `whitespace-pre-wrap`——保留换行与空格对齐，且仍允许长行自动换行
- 顺带修复两处「按 unwind 语义编写、被 abort 编译静默失效」的既有防御（委托 watcher panic 转 fail、LSP 回调隔离）
- 回归保护：U1 task_status 3/3 + core 1229 全绿；U2 判别力已证（注入旧行为必红）；U3 tauri 12/12；U4 前端 3 条先红后绿 + 全量 400 全绿 + 构建产物 CSS 规则确认；panic hook 单测 3/3 + release 探针（JOIN_ERROR_IS_PANIC / HOOK_REPORT_OK / SURVIVED）

## [0.4.2] - 2026-09-14

### Fixed
- **应用闪退根因修复（后台任务/命令输出链 UTF-8 边界安全）**：后台任务输出缓冲的 32KB 尾部截断以**字节**定位丢弃起点（`String::drain(..excess)`），起点落在中文等多字节字符中间时必 panic；release 构建 `panic = "abort"` 使该 panic 升级为**全进程无痕退出**（无日志、GUI 直接消失——「闪退」根因；实测崩点 33270 = 32768 + 502，两次复现一致，纯 ASCII 不触发）。修复 = 新增 `common::truncate::truncate_keep_tail_bytes`（UTF-8 边界安全，对齐既有截断单点），两处截断点替换；顺带修复跨 8KB 读边界中文被 lossy 为 `�`（carry 拼接）
- **VFS sqlite 后端 NULL 层读取**：`read_content` 对 NULL 列直接取值报 `Invalid column type Null`——L0/L1 缺失为合法状态（`ContextEntry` 三层均 `Option<String>`，local 后端语义即 `not_found`），读取报错根因。修复为 NULL → `not_found`（后端语义契约对齐）
- 回归保护：截断边界新增 7 条测试 + 2 条反向验证（注入旧截断必红）；VFS NULL 新增回归测试 + 反向验证（注入旧行为必红，报错文本与用户检索所见一致）；core 1223 全绿，fmt / clippy 0

### Added
- 技能导入工具（`core/examples/skill_import.rs`）：外部标准化方法论（`abstract.md` + `content.md` 目录式）→ VFS 技能库导入器（`--dry-run` / `--verify` / `--data-dir` 三模式；幂等，重跑即更新；显式 `update_summary_vectors`）；随工具附 11 篇内置方法论源（`core/examples/import-skills/`：调试/并行 agent/评审/TDD/研究/交接等）

## [0.4.1] - 2026-09-14

### Fixed
- grep 工具搜索根工作区归属：相对 `path` 与缺省 `path` 此前以进程 cwd（桌面端为 exe 安装目录）为搜索基准，遍历不到目标时**静默返回 0 条假阴性**（脚手架排查被误导为「文档断链」的根因）；现与 glob/read_file 同一套规则解析到**会话工作目录**（无会话时保持旧回退语义），搜索输出同时回显解析后的搜索根
- **用户取消保留已收输出落库（对齐 DSH「中断先于分发」）**：点击停止时，流式中已收到的正文/思考（用户已看到的内容）整体丢弃、不落库——界面已渲染而库里没有（刷新即失、「已保留部分输出」提示名不副实）。现取消时：非空白的正文/推理**落库**（finish `interrupted`），**未分发的工具调用一律丢弃**（含未闭合参数的流式半截调用——保留就须捏造结果），下一次请求自动包含用户已看到的内容；完全无内容时不落库（与旧行为同）
- **关于页版本号动态读取**：此前硬编码「版本 0.1.0」（与发布版本长期漂移），现读取服务 `/health` 真实版本（后端不可达时显示占位符）
- **聊天自动追踪底部**：工具调用/流式内容增高时视图不跟底——滚动观察目标由固定高度的滚动容器改为**内容列**（`data-chat-flow`），保留容器观察
- 回归保护：grep 新增两条回归测试（相对路径 / 缺省 path 命中会话工作目录）+ 取消落库 4 条单元测试与端到端断言（含「无 tool_calls 落库」）+ 前端回归测试（关于页 / 滚动跟随）

## [0.4.0] - 2026-09-13

### Added
- 记忆巩固通道（ADR-034）：演化综述 `merge` 动作（目标原地重写 + `merge_from` 归档旧条）；`auto_consolidation` 配置兑现（prompt 巩固职责 + apply 层开关）
- 运维子域单一判定 `memory_paths::is_operational_path`（记忆面板 / 摘要生成 / 检索统一过滤）；GC TTL 白名单分层（cases/clipboard 90 天、evolution_reports 30 天、语义记忆与状态类豁免）
- 概览压缩契约：`generate_overview` 短内容直用（≤2000 token）+ 长度守卫（超原文回退）+ prompt 忠实原则
- 存量清洗工具 `core/examples/memory_cleanup.rs`（幂等 + dry-run）
- 真实后端集成测试（SqliteBackend + 内存 DB）：删除/合并链形态锁定

### Fixed
- `find_entry` 仅匹配目录条目 → 演化删除通道自上线从未生效（三个归档目录恒为空）→ 修复为匹配任意条目（文件/目录）
- GC `scan_memory` 仅扫一层 → 递归全树（90 天 TTL 从未生效）
- 综述清单：memory 递归条目明细（此前仅目录名）；skill root 修正（此前指向空目录）
- 记忆面板 / 摘要扫描含运维数据（状态/日志/归档）→ 消费面分离

### Changed
- 写入收口：过程记录（版本发布 / 功能完成 / 里程碑）不写记忆；过时、被证伪、重复条目主动列入删除
- 概览生成：短内容不再调用 LLM（直用原文）——消除扩写/脑补

## [0.3.18] - 2026-09-13

### Added
- 命令输出截断落盘：`execute_command` 截断前完整输出写入 `{data_dir}/command_logs/exec-{uuid8}.log`（header + stdout + `[stderr]` 分节，best effort）；返回新增 `stdout_total_bytes` / `stderr_total_bytes` / `log_file`（配合 `read_file` offset/limit 分页回读）——消除“截断即丢失”

### Changed
- 待办 `create` 语义改为整表替换（对齐 DSH last-write-wins）：无条件替换整批（`TodoStore::replace_many` 原子写、失败不触碰旧批）——模型重规划后废弃项随替换自然移除；删除单条快捷形态；响应新增 `replaced`；父子挂靠改经 `update`
- 截断实现对称化：新增 `truncate_tail_noted`（对称既有 `truncate_head_noted`），截断标记携带完整输出路径与回读指引

## [0.3.17] - 2026-09-13

### Added
- 嵌入调用用量入账：`EmbeddingUsageSink` 契约（model 层定义 + 装配层注入）+ `UsageLogEmbeddingSink`（异步落库）——嵌入 token 此前完全不写 `usage_logs`（不入账 = 统计/账单盲区）；VFS 实例与 Agent 实例（构造 + 热重载）均注入
- 查询嵌入缓存（键 `model|dimensions|text`，容量 256，FIFO）+ 空文本/空查询短路——同一 query 一次检索被嵌 2 次、`text_len=0` 空调用均被消除

### Changed
- 动态工具注册表 `HashMap` → 注册序 `Vec`（追加末尾、按名去重）；`definitions()` 声明稳定性契约（连续取定义逐字节一致）——消除工具定义顺序漂移对前缀缓存命中的干扰
- 角色清单与工具描述解耦：`delegate_to_agent` 描述移除运行时角色 L0 摘要注入（保留内置角色名 + 指向 `suggest_role`）；删除 `RoleRegistry::delegate_role_segment()`
- 会话级工具表变化检测：真用户轮在请求前比对 `toolset_fingerprint`（有序名 + 描述 + params schema），与基线不同则强制压缩一次（首轮只记基线；唤醒轮不检查）

### Fixed
- 终止/取消轮把**上一轮** assistant 结论当作本轮边界事件下发（前端合并进本轮占位气泡）→ 轮前记水位 `prev_assistant_id`，只下发本轮新产生消息（`select_turn_assistant` 水位纯函数）
- 子代理 `submit_result` 被执行侧角色白名单误拒 → 协议工具豁免单点（`PROTOCOL_TOOLS` / `is_protocol_tool`，请求注入与执行豁免同源；白名单外普通工具仍被拒）
- 停止按钮竞态：新会话首个请求未返回（会话 id 未就绪）时点停止 → 取消请求发不出；新增 `cancelWhenSessionIdReady`（轮询等待 id 就绪后补发）

## [0.3.16] - 2026-09-12

### Added
- 机制文档 4 份：`docs/architecture/context-pipeline.md`（组装+压缩+量化+故障模式）、`event-protocol.md`（事件协议+快照恢复+可靠性分层）、`model-provider-notes.md`（reasoning 契约+实测）、`task-runtime.md`（任务运行时模型）
- 运维手册 3 份：`docs/operations/{troubleshooting,data-health-check,release-msi}.md`（巡检 SQL 全部经 EXPLAIN 实测）
- 回归测试 5 个（VFS 前缀匹配 ×2、usage 统计口径 ×1、事件总线背压 ×1、命令输出节流 ×1），全部经反向验证（注入旧缺陷必红）

### Changed
- 结构重构（行为零变化）：`CommandManager::spawn_background` 308 → 65 行；`AgentLoop::run_stream` 226 → 46；`run_turns` 222 → 105；子代理委托入口 235 → 95
- 事件总线由无界 → **有界**（`EVENT_BUS_CAPACITY = 4096` + `try_send` 丢弃计数）；`task_event_tx` 的 `Lagged` 由静默 `continue` 改为显式告警
- 命令输出事件按 **100ms 时间窗合并**（对齐 ADR-028 声明）+ 流结束 flush 残留增量
- `db::SqliteDb::open` / `open_in_memory` / `init_all_schemas` 返回 `TianyanError`（rusqlite 类型不再外泄）；`Database` 门面去掉重复错误前缀
- 统计落盘 `flush_counts` 语句 prepare 一次复用（逐行插入仍是语义要求）

### Fixed
- VFS `list_directory` 的 `LIKE` 前缀匹配会把 URI 里的 `_` 当通配符（可能列出兄弟目录条目）→ 改 `substr(uri, 1, length(?1)) = ?1`；`delete_entry` 长度改由 SQL 侧 `length()` 计算（消除 Rust 字节长度与 SQL 字符长度混用）
- `db::UsageRepo::total_stat` 丢弃传入 SQL、按 `conds.len()` 硬编码重建 WHERE（过滤组合一改即静默失配）→ WHERE 子句单点构造
- 截断实现重复：`tool_registry::truncate_trace_params` 删除，统一走 `common::truncate::truncate_utf8_boundary`
- `scripts/build.ps1` 产物路径错误（查 `tauri/target`，实际在 workspace `<root>/target`）；补 `protoc` 依赖检查
- `docs/operations/data-health-check.md` 首版 `DELETE ... AS b` 语法（SQLite 不支持 DELETE 表别名）已修正

### Removed
- `core/src/agent/role_store.rs` 兼容 re-export（统一 `crate::role_store`）

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

## [0.3.15] - 2026-09-12

### Changed
- **安全策略收敛（ADR-033）**：审批行为收敛为 `ApprovalMode`（`autonomous` 默认 / `confirm` / `interactive`），命令检查收敛为 `SafetyMode` 四态（`relaxed` 默认：跳过元字符/解释器检查、黑名单仍强制）；`allow_all_operations`/`wait_for_approval`/`confirm_commands`/`unattended_mode` 四个旧开关下线（读取兼容 + 启动自动归一）
- 默认 soul 增加「思考语言」约束（内部思考必须中文；同义内容省 ~13% token）

### Fixed
- 文件日志层恒用配置级别（`file_filter` 接线：此前未挂载，`RUST_LOG` 会连带过滤文件层）
- 后台任务惰性加载失败回滚标志（锁不可用曾导致永不重试 → 历史任务不加载、中断任务不标 Failed、唤醒不触发）
- 写/读路径 `try_lock` → `lock().await`（7 处静默丢弃 → 任务状态/用量/GEPA 数据丢失）
- 模型错误语义分类单点（KIND 前缀）+ 配置面板连接测试改语义谓词（ADR-014）
- 流式 usage 解析去除生产路径唯一 `unwrap`；续读偏移按实际返回行数（字节截断场景曾跳行）

### Added
- CI 质量工作流（`.github/workflows/quality.yml`）：fmt / Rust 单测 / tool-catalog freshness / 前端 lint+typecheck+vitest

### Docs
- ADR-033；三处回归测试缺口补齐（f2/f1/e + 压缩点自身）；前端 prettier/eslint 清零；AGENTS/module-map/known-issues 口径同步

## [0.3.10] ~ [0.3.14] - 2026-09-11 ~ 2026-09-12

### Changed / Fixed / Added
- 详见 [`docs/release/RELEASE_NOTES.md`](docs/release/RELEASE_NOTES.md)：记忆面板分类树、记忆浏览修复、唤醒轮实时性 + ollama 思考字段回传、压缩点体验与 user 锚定、预算误报修复 + 压缩阈值 60% + 读通道治理 + 思考块收起

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
