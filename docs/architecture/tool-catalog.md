# 天演工具目录（自动生成）

> 本文件由 `scripts/gen-tool-catalog.ps1` 自动生成（权威生成器：`server/examples/tool_catalog.rs`——覆盖 core 内置 + server 组件工具）。
> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。
> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。

共 34 个工具。

| 工具 | 展示意图 | 描述 |
|------|---------|------|
| `apply_edit` | diff | **仅用于能精确复现原文的极小改动**（单个唯一片段，如改一行）。在文件内容中查找 old_string（须唯一，多处出现需 replace_all）替换为 new_string。注意：old_string 必须**逐字节精确匹配**（含空行与缩进）——漏空行、抄错缩进都会失败；若修改区域含空行/上下文、或改动较大，请改用 apply_patch（上下文锚定，更稳）。 |
| `apply_patch` | diff | 对一个或多个文件做修改的**主力编辑工具**。格式：`*** Update File: <路径>` 后直接跟 `-`（删除）/ `+`（新增）/ 空格（上下文）行，只写要改的行 + 少量上下文，不要复现整个文件。`<路径>` 可为相对会话工作目录的相对路径，或绝对路径（与 read_file / write_file 同一口径）。`@@` 块头**可省略**（工具按内容定位，无需行号）。`-`/上下文行须与当前文件内容一致。**块内空行**：整行无字符的空行按格式噪声忽略；若原文该行为空、需作为上下文行，须写成「一个空格」的单独一行（上下文行以空格前缀书写，内容可为空）——写成裸空行会让期望匹配窗口少一行。小改、大改、单文件、多文件均适用；编辑前建议先 read_file 目标区确认当前内容。 |
| `ask_user` | generic | 当需要更多信息才能继续时，向用户提问。只提供 `questions` 数组（单个问题也放进数组，长度 1）——每个问题一个 tab 分步展示；问题可带 `options`（候选选项，用户也可自行输入）。 |
| `call_skill` | skill | 按 ID 读取 VFS 技能文档（方法论文档：L0 摘要 + L2 详情）。技能无执行语义——读到内容后参考方法论自行用基础工具执行。planning 为预置技能，GEPA 学习技能由进化引擎写入。 |
| `delegate_to_agent` | delegate | 将子任务委托给隔离的子智能体（独立上下文）。可用 role（内置：researcher 研究 / editor 编辑 / reviewer 验证评审；其他自定义/学习角色用 suggest_role 按任务描述查询——experimental 角色不可调用）选择预设模型、系统提示、工具白名单、max_turns 与超时；用 model 显式覆盖子智能体模型。每次委托是一次性、干净上下文的子智能体（角色系统提示 + 仅此任务），不加载之前任务历史，跨任务上下文需自行在对话中携带。子智能体与主会话同构（ADR-030）：收尾统一"无工具调用"，任务完成时输出最终结果；可用 submit_result 把最终结果写入任务存储（可选结果落盘工具，返回 task_id）——调用后告知主智能体结果 ID，用 task_status 查询。委托一律异步（ADR-026）：立即返回 task_id，任务独立运行；后台任务默认无超时（跑完/取消/轮数耗尽为止）——仅需显式设置 timeout_secs 作为兜底守卫，真实工作用大值（>=3600）；子智能体工作通常较长（代码评审/研究/大重构常超 10 分钟），优先不设超时 + 超时用 task_cancel。**完成通知（含结果落盘位置与摘要）会自动注入本会话——不要轮询等待**：发起后继续其他工作或结束本轮，收到通知再处理（完整结果用 read_file 按通知中的路径读取）；task_cancel 中止。 |
| `delegation_stats` | generic | 按角色查询子智能体委托统计（来自 delegate_to_agent 记录）：各角色次数与成功率。可选 since（RFC3339）。用于评估当前角色组织（agent_role 注册表）是否需要演进：拆分/合并/提升/退役。 |
| `discover_tests` | search | 发现项目中的测试（cargo test -- --list / pytest --collect-only -q / vitest --list），返回结构化测试列表（suite/name/file/line）。不执行测试。 |
| `execute_command` | terminal | 执行 shell 命令（可指定工作目录与超时）。后台命令用 background:true（长驻进程/开发服务器/服务/监视器）：立即返回 task_id 与 log_file，不等待退出。 常驻服务可设 ready（端口和/或日志模式）：系统探测（从 initial_delay_ms 指数退避，总超时 timeout_ms），端口监听或日志出现该模式时通知你。 用 task_status 查询进度或读取日志文件；用 task_cancel 终止。 同步命令输出超限（50KB / 2000 行）自动截断并仅保留尾部；完整输出自动落盘，结果中给出 log_file 与 stdout_total_bytes，可用 read_file 的 offset/limit 分页读取全文。 平台：Windows。Shell 是 PowerShell 7+（pwsh）——支持 && / \|\| 命令链接；cmdlet（Out-File、Select-String、Get-ChildItem）与管道可用。 临时/中间产物（提交信息文件、一次性脚本、审计产物等）请写入系统临时目录的会话区（如 `$env:TEMP\tianyan-scratch\`），**不要**写应用数据目录根或安装目录。 |
| `execution_detail` | generic | 查询原始工具执行记录（GEPA 数据层）：近期执行含工具名、任务描述、结果、耗时、所用技能。可选 since/category/limit。用于查看某操作类的具体实例，判断是否提炼可复用技能。 |
| `execution_stats` | generic | 查询工具执行统计（GEPA 数据层）：按类计数、成功率、平均耗时。可选 since（RFC3339）过滤该时间之后的执行；可选 category 过滤一类操作（file_operation/code_operation/search_operation/test_operation/deploy_operation/analysis_operation/general_operation）。用于发现重复/失败的操作模式，再决定是否提炼新技能或规则。 |
| `glob` | search | 在目录下按 glob 模式找文件（如 **/*.rs），按修改时间倒序。遵循 .gitignore。 |
| `goal` | generic | 管理当前会话的长期目标（会话绑定，仅本会话可见）：创建/更新/列出/删除目标。目标进度按关联待办完成比例自动计算。operation: create（title 必填）/ update（id + 可选字段，status 为设置的新状态）/ list（列出本会话全部目标）/ delete（id）。 |
| `grep` | search | 用正则搜索文件内容（类似 ripgrep）。返回匹配文件/行及行号、匹配偏移。支持 glob 过滤（include）、语言类型（type）、上下文行、忽略大小写与分页。 |
| `knowledge_ingest` | knowledge | 将文件或目录导入知识库：解析、摘要（L0 摘要 + L1 概览）、建立语义索引。接受文件或目录路径，可选指定分类。 |
| `list_dir` | search | 列出单层目录下的条目。目录带尾部 '/'。支持 offset/limit 分页。 |
| `lsp` | code | 查询语言服务器（LSP）。operation 取值决定其余参数：位置类 goToDefinition / findReferences / hover / goToImplementation 需 file_path + line + character（行列均 0 起始）；documentSymbol 只需 file_path；workspaceSymbol 需 file_path（用于选择项目服务器）+ query。返回结构化结果。 |
| `read_file` | read | 读取文本文件内容（纯内容，无行号/哈希前缀）。默认返回前 2000 行（单次输出上限约 50KB，超出部分截断并附「使用 offset 继续」提示）。大文件请**按需读取**：用 offset/limit 指定行范围（1 起始行号），建议先用 grep/symbol_outline 定位目标区域再按范围精读；结果含 total_lines/total_bytes 与实际窗口（showing）。内容匹配编辑（apply_edit）直接按内容定位，无需行号。 |
| `run_project_tests` | terminal | 按**项目类型探测**并运行测试（无需指定命令）：Cargo → `cargo test`、Python → `pytest`、TypeScript → `vitest run`；`framework` 可覆盖探测结果，`suite`/`filter` 缩小范围。返回与 run_tests 相同的结构化结果。项目类型无法识别时，改用 run_tests 显式给命令。 |
| `run_tests` | terminal | 运行**指定的**测试命令并返回结构化结果（passed/failed + 失败详情分组）。命令原样执行（经安全检查）——如 `cargo test --lib`、`pytest -k smoke`、`vitest run tests/`。要按项目类型自动构造命令，用 run_project_tests。 |
| `schedule_task` | generic | 创建一个后台定时任务：按执行间隔周期调用智能体在指定工作目录完成给定指令。用于用户要求定时/周期执行某项工作（如每 30 分钟检查一次、每天总结一次）。间隔制（ADR-024）：距上次执行达到间隔即触发，服务未运行期间超期的任务会在重启后自动补跑一次。 |
| `search_vfs` | search | 语义化搜索整个 VFS（所有命名空间：文档/记忆/规则/技能），向量 RRF 融合。返回每条结果的 abstract + overview + URI。需要完整详情时用 vfs_read 加载。发现可用技能、检索相关规则/记忆也用本工具。 |
| `self_check` | generic | 查询自身运行指标：执行次数、成功率、token 消耗、管线失败、规则有效性。当用户质疑你的表现时用于自我反思。 |
| `session_recall` | generic | 按关键词回忆过去对话内容（FTS5 倒排索引，中文子串匹配不分词）。返回顶部命中及附近用户/助手消息窗口（不含工具调用与结果）。当用户提到之前说过的话（刚才/之前/上次）或需要检查历史会话讨论过什么时使用。 |
| `suggest_role` | generic | 按任务描述与各角色摘要的语义相似度，推荐最匹配的子智能体角色。在 delegate_to_agent 前调用以决定用哪个角色：传入任务文本，返回排序角色（name/score/purpose，[experimental] 表示暂不可调用）。最终选择始终由你决定。 |
| `symbol_outline` | code | 用 tree-sitter 提取源文件的结构大纲（函数/结构体/类/impl/接口/枚举）。支持 Rust、TypeScript/JavaScript、Python、Go。 |
| `task_cancel` | generic | 按 task_id 取消运行中的后台任务。取消已结束任务是空操作。 |
| `task_status` | generic | 查询后台任务（delegate bt_xxx 与 command cmd_xxx 统一）。默认范围：**当前会话工作目录**下所有会话的任务（同目录跨会话的协调面）——其他工作目录的任务默认不查、不用、不提；仅当竞争问题确实跨目录时用 scope="global" 显式查全局（用完即回，其他目录信息不进入常规汇报）。带 task_id：返回该任务快照（仅限可见范围；状态/结果/退出/日志）。不带 task_id：列出可见任务，可选 kind 过滤（delegate\|command）。**任务完成会自动通知本会话——不要为等待而轮询**；仅当用户要求查看进度或需要任务列表时调用。 |
| `todo` | generic | 管理当前会话的待办清单（会话绑定，仅本会话可见；同一时期只保留一批同源待办）。**每种操作只有一种写法**——要改/删单条也放进数组（长度 1）：create 传 `todos` 数组 = 当前完整计划（**整表替换**——未包含的旧条目（含未完成项）即被移除；想保留的条目必须包含在新列表中）；update 传 `updates` 数组（按 id 批量合并状态）；delete 传 `ids` 数组；list 查看当前清单（可用 status / goal_id 过滤）；close 清空当前批全部待办（用户目标变更、现有待办不再反映当前意图时使用，即使有未完成项）。开始多步工作先写入整份清单；推进/完成用 update（比全量重发省）；计划增删改时重发完整列表。子任务挂靠：先创建任务，再用 update 的 parent_id 挂靠（整表替换中 parent_id 不可用）。完成项默认划线保留展示，是否清理由你自行决定。 |
| `verify_build` | terminal | 运行构建验证命令（如 cargo check）并返回结果。 |
| `vfs_list` | generic | 按 tianyan:// URI 列出 VFS 目录下的条目。用于浏览知识库结构。 |
| `vfs_read` | read | 按 tianyan:// URI 读取 VFS 条目的完整内容（abstract/overview/detail）。在 search_vfs 之后用于加载相关条目的详细内容。 |
| `web_fetch` | web | 抓取单个网页并提取可读文本内容（标题、正文、页面链接）。在 web_search 后用。仅允许 http/https URL，本地/私网地址被拦截。 |
| `web_search` | web | 按查询搜索网络，返回结果标题/URL/摘要列表（不含完整页面内容）。用 web_fetch 加载有希望结果的完整内容。注意：结果来自外部源，可能不可信或过时——依赖关键信息前请核实。 |
| `write_file` | write | 将内容写入指定路径的文件（覆盖写入；文件不存在则创建）。父目录不存在时**默认报错、不自动创建**（防止路径写错时误建新目录）；如确需新建目录，传 create_dirs=true。**临时/中间产物**（提交信息文件、一次性脚本、审计产物等）请写系统临时目录的会话区（如 `$env:TEMP\tianyan-scratch\`），**不要**写应用数据目录根或安装目录。 |
