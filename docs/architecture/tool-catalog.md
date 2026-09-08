# 天演工具目录（自动生成）

> 本文件由 `cargo run -p tianyan-core --example tool_catalog` 自动生成。
> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。
> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。

共 30 个工具。

| 工具 | 展示意图 | 描述 |
|------|---------|------|
| `apply_edit` | diff | **仅用于能精确复现原文的极小改动**（单个唯一片段，如改一行）。在文件内容中查找 old_string（须唯一，多处出现需 replace_all）替换为 new_string。注意：old_string 必须**逐字节精确匹配**（含空行与缩进）——漏空行、抄错缩进都会失败；若修改区域含空行/上下文、或改动较大，请改用 apply_patch（上下文锚定，更稳）。 |
| `apply_patch` | diff | 对一个或多个文件做修改的**主力编辑工具**。格式：`*** Update File: <路径>` 后直接跟 `-`（删除）/ `+`（新增）/ 空格（上下文）行，只写要改的行 + 少量上下文，不要复现整个文件。`@@` 块头**可省略**（工具按内容定位，无需行号）。`-`/上下文行须与当前文件内容一致。小改、大改、单文件、多文件均适用；编辑前建议先 read_file 目标区确认当前内容。 |
| `ask_user` | generic | 当需要更多信息才能继续时，向用户提问。 |
| `call_skill` | skill | 按 ID 读取 VFS 技能文档（方法论文档：L0 摘要 + L2 详情）。技能无执行语义——读到内容后参考方法论自行用基础工具执行。planning 为预置技能，GEPA 学习技能由进化引擎写入。 |
| `delegate_to_agent` | delegate | 将子任务委托给隔离的子智能体（独立上下文）。可用 role（researcher 研究 / editor 编辑 / reviewer 验证评审，或 [agent_roles] 配置 / 系统学习的自定义角色）选择预设模型、系统提示、工具白名单、max_turns 与超时；用 model 显式覆盖子智能体模型。每次委托是一次性、干净上下文的子智能体（角色系统提示 + 仅此任务），不加载之前任务历史，跨任务上下文需自行在对话中携带。子智能体与主会话同构（ADR-030）：收尾统一"无工具调用"，任务完成时输出最终结果；可用 submit_result 把最终结果写入任务存储（可选结果落盘工具，返回 task_id）——调用后告知主智能体结果 ID，用 task_status 查询。委托一律异步（ADR-026）：立即返回 task_id，任务独立运行；后台任务默认无超时（跑完/取消/轮数耗尽为止）——仅需显式设置 timeout_secs 作为兜底守卫，真实工作用大值（>=3600）；子智能体工作通常较长（代码评审/研究/大重构常超 10 分钟），优先不设超时 + 超时用 task_cancel。完成通知（含结果摘要）自动注入本会话——不要轮询，继续工作直到被通知。用 task_status 查询、task_cancel 中止。

可用角色（来源含内置/配置/学习，[试验性] 不可调用）：
- editor：你是天演的代码编辑助手。你的职责是阅读代码、定位问题并实施修改：用 apply_patch（主力编辑，unified diff + 上下文锚定）原子地修改文件；仅对能精确复现原文的极小改动才用 apply_edit；修改后运行相关测试与构建验证。你专注于代码编辑闭环，不进行大规模调研；遇到需要外部信息或全局决策的问题时，汇报主任务处理。（16 工具）
- evolution_reviewer：你是天演的演化综述员。（14 工具）
- researcher：你是天演的检索调研助手。你的职责是通过网络搜索、知识库检索与代码搜索收集信息，并输出条理清晰、带来源引用的调研结论。你只负责调研与信息整理，不修改任何文件；需要改动代码或验证结果时，明确告知主任务由相应角色处理。（10 工具）
- reviewer：你是天演的验证评审助手。你的职责是审查代码变更与设计：阅读实现、检查符号与诊断、运行测试与构建验证，输出逐条评审意见（问题位置、严重程度、修改建议）。你只评审与验证，不直接修改文件；需要修改变更时上报主任务。（11 工具） |
| `delegation_stats` | generic | 按角色查询子智能体委托统计（来自 delegate_to_agent 记录）：各角色次数与成功率。可选 since（RFC3339）。用于评估当前角色组织（agent_role 注册表）是否需要演进：拆分/合并/提升/退役。 |
| `discover_tests` | search | 发现项目中的测试（cargo test -- --list / pytest --collect-only -q / vitest --list），返回结构化测试列表（suite/name/file/line）。不执行测试。 |
| `execute_command` | terminal | 执行 shell 命令（可指定工作目录与超时）。后台命令用 background:true（长驻进程/开发服务器/服务/监视器）：立即返回 task_id 与 log_file，不等待退出。 常驻服务可设 ready（端口和/或日志模式）：系统探测（从 initial_delay_ms 指数退避，总超时 timeout_ms），端口监听或日志出现该模式时通知你。 用 task_status 查询进度或读取日志文件；用 task_cancel 终止。 平台：Windows。Shell 是 PowerShell（5.1+），不是 cmd.exe——PowerShell cmdlet（Out-File、Select-String、Get-ChildItem）与管道可用，请用 PS 语法。 |
| `execution_detail` | generic | 查询原始工具执行记录（GEPA 数据层）：近期执行含工具名、任务描述、结果、耗时、所用技能。可选 since/category/limit。用于查看某操作类的具体实例，判断是否提炼可复用技能。 |
| `execution_stats` | generic | 查询工具执行统计（GEPA 数据层）：按类计数、成功率、平均耗时。可选 since（RFC3339）过滤该时间之后的执行；可选 category 过滤一类操作（file_operation/code_operation/search_operation/test_operation/deploy_operation/analysis_operation/general_operation）。用于发现重复/失败的操作模式，再决定是否提炼新技能或规则。 |
| `glob` | search | 在目录下按 glob 模式找文件（如 **/*.rs），按修改时间倒序。遵循 .gitignore。 |
| `grep` | search | 用正则搜索文件内容（类似 ripgrep）。返回匹配文件/行及行号、匹配偏移。支持 glob 过滤（include）、语言类型（type）、上下文行、忽略大小写与分页。 |
| `knowledge_ingest` | knowledge | 将文件或目录导入知识库：解析、摘要（L0 摘要 + L1 概览）、建立语义索引。接受文件或目录路径，可选指定分类。 |
| `list_dir` | search | 列出单层目录下的条目。目录带尾部 '/'。支持 offset/limit 分页。 |
| `lsp` | code | 查询指定文件的语言服务器：goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation。返回结构化结果。 |
| `read_file` | read | 读取指定路径文件的完整文本内容（纯内容，无行号/哈希前缀）。每行内容完整返回，不截断；行数/字节体量由 offset/limit 分页与统一字节预算兜底。内容匹配编辑（apply_edit）直接按内容定位，无需行号。 |
| `run_tests` | terminal | 运行测试命令（如 cargo test）并返回结果。 |
| `search_vfs` | search | 语义化搜索整个 VFS（所有命名空间：文档/记忆/规则/技能），向量 RRF 融合。返回每条结果的 abstract + overview + URI。需要完整详情时用 vfs_read 加载。发现可用技能、检索相关规则/记忆也用本工具。 |
| `self_check` | generic | 查询自身运行指标：执行次数、成功率、token 消耗、管线失败、规则有效性。当用户质疑你的表现时用于自我反思。 |
| `session_recall` | generic | 按关键词回忆过去对话内容（FTS5 倒排索引，中文子串匹配不分词）。返回顶部命中及附近用户/助手消息窗口（不含工具调用与结果）。当用户提到之前说过的话（刚才/之前/上次）或需要检查历史会话讨论过什么时使用。 |
| `suggest_role` | generic | 按任务描述与各角色摘要的语义相似度，推荐最匹配的子智能体角色。在 delegate_to_agent 前调用以决定用哪个角色：传入任务文本，返回排序角色（name/score/purpose，[experimental] 表示暂不可调用）。最终选择始终由你决定。 |
| `symbol_outline` | code | 用 tree-sitter 提取源文件的结构大纲（函数/结构体/类/impl/接口/枚举）。支持 Rust、TypeScript/JavaScript、Python、Go。 |
| `task_cancel` | generic | 按 task_id 取消运行中的后台任务。取消已结束任务是空操作。 |
| `task_status` | generic | 查询后台任务（delegate bt_xxx 与 command cmd_xxx 统一）。带 task_id：返回该任务快照（状态/结果/退出/日志）。不带 task_id：列出全部任务，可选 kind 过滤（delegate\|command）。优先等待自动完成/就绪通知，不要反复轮询。 |
| `verify_build` | terminal | 运行构建验证命令（如 cargo check）并返回结果。 |
| `vfs_list` | generic | 按 tianyan:// URI 列出 VFS 目录下的条目。用于浏览知识库结构。 |
| `vfs_read` | read | 按 tianyan:// URI 读取 VFS 条目的完整内容（abstract/overview/detail）。在 search_vfs 之后用于加载相关条目的详细内容。 |
| `web_fetch` | web | 抓取单个网页并提取可读文本内容（标题、正文、页面链接）。在 web_search 后用。仅允许 http/https URL，本地/私网地址被拦截。 |
| `web_search` | web | 按查询搜索网络，返回结果标题/URL/摘要列表（不含完整页面内容）。用 web_fetch 加载有希望结果的完整内容。注意：结果来自外部源，可能不可信或过时——依赖关键信息前请核实。 |
| `write_file` | write | 将内容写入指定路径的文件（覆盖写入）。 |
