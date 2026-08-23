# 天演工具目录（自动生成）

> 本文件由 `cargo run -p tianyan-core --example tool_catalog` 自动生成。
> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。
> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。

共 30 个工具。

| 工具 | 展示意图 | 描述 |
|------|---------|------|
| `apply_edit` | diff | Apply precise edits to a file using line-number + content-hash anchors. Each edit targets a line range verified by an anchor hash; edits are applied bottom-up after all anchors are validated (atomic batch). |
| `apply_patch` | diff | Apply a unified diff patch (*** Update File format) to one or more files. Supports fuzzy matching of context lines and multiple files in one patch. |
| `ask_user` | generic | Ask the user a question when more information is needed to proceed. |
| `call_skill` | skill | Call a registered skill by ID with parameters. |
| `delegate_to_agent` | delegate | Delegate a sub-task to an isolated sub-agent with its own context. Use role (researcher for research, editor for code editing, reviewer for verification/review, or custom roles configured in [agent_roles] or learned by the system) to pick a preset model, system prompt, tool allowlist, max_turns and timeout; use model to explicitly override the sub-agent model. Each delegation is a disposable one-shot sub-agent with a clean context (role system prompt + this task only) — it never loads prior task history, and you are responsible for carrying cross-task context in your own conversation. The sub-agent must call submit_result to deliver its final result; unsubmitted text is never accepted as final. Sync delegation (default) blocks until the sub-agent submits: timeout_secs is the sync budget (default 120s) — if exceeded the sub-task is automatically promoted to a background task (you get task_id and keep working; completion is notified, NOT a failure). For long-running or parallel sub-tasks set background=true: returns task_id immediately; background tasks have NO timeout by default (run until done, cancelled, or max_turns exhausted) — only set timeout_secs as an explicit worst-case guard if you must, and use large values (>= 3600) for real work; sub-agent work is typically long (code review, research, large refactors easily exceed 10 minutes), so prefer no timeout + task_cancel when it overstays. Completion notification (with result summary) is injected into this session automatically — do NOT poll, just continue working until notified. Use task_status to query, task_cancel to abort.

可用角色（来源含内置/配置/学习，[试验性] 不可调用）：
- editor：你是天演的代码编辑助手。你的职责是阅读代码、定位问题并实施修改：用语义化编辑（apply_edit / apply_patch）原子地修改文件，修改后运行相关测试与构建验证。你专注于代码编辑闭环，不进行大规模调研；遇到需要外部信息或全局决策的问题时，汇报主任务处理。（16 工具）
- evolution_reviewer：你是天演的演化综述员。（14 工具）
- researcher：你是天演的检索调研助手。你的职责是通过网络搜索、知识库检索与代码搜索收集信息，并输出条理清晰、带来源引用的调研结论。你只负责调研与信息整理，不修改任何文件；需要改动代码或验证结果时，明确告知主任务由相应角色处理。（10 工具）
- reviewer：你是天演的验证评审助手。你的职责是审查代码变更与设计：阅读实现、检查符号与诊断、运行测试与构建验证，输出逐条评审意见（问题位置、严重程度、修改建议）。你只评审与验证，不直接修改文件；需要修改变更时上报主任务。（11 工具） |
| `delegation_stats` | generic | Query sub-agent delegation statistics by role (from delegate_to_agent records): per-role counts and success rates. Optional since (RFC3339). Use to evaluate whether the current role organization (agent_role registry) should evolve: split, merge, promote or retire roles. |
| `discover_tests` | search | Discover tests in the project (cargo test -- --list / pytest --collect-only -q / vitest --list) and return structured test list with suite, name, file and line. Does not execute tests. |
| `execute_command` | terminal | Execute a shell command with optional working directory and timeout. Use background:true for long-running or persistent processes (dev servers, services, watchers): it returns immediately with task_id/log_file and does not wait for exit. For persistent services set ready with a port and/or log pattern: the system probes (exponential backoff from initial_delay_ms, total timeout_ms) and notifies you when the port listens or the pattern appears in the log. Query progress with task_status or read the log file; terminate with task_cancel. Platform: Windows. Shell is PowerShell (5.1+), NOT cmd.exe — PowerShell cmdlets like Out-File, Select-String, Get-ChildItem and pipelines work; use PS syntax. |
| `execution_detail` | generic | Query raw tool execution records (GEPA data layer): recent executions with tool name, task description, result, duration, skills used. Optional since/category/limit. Use to inspect concrete examples of an operation category when deciding whether to extract a reusable skill. |
| `execution_stats` | generic | Query tool execution statistics (GEPA data layer): per-category counts, success rates, average durations. Optional since (RFC3339 time) filters to executions after that time; optional category filters one operation class (file_operation/code_operation/search_operation/test_operation/deploy_operation/analysis_operation/general_operation). Use to detect recurring or failing operation patterns before proposing a new skill or rule. |
| `glob` | search | Find files by glob pattern (e.g. **/*.rs) under a directory, sorted by modification time (newest first). Respects .gitignore. |
| `grep` | search | Search file contents for a regex pattern (like ripgrep). Returns matching files/lines with line numbers and match offsets. Supports glob filters (include), language types (type), context lines, ignore-case and pagination. |
| `knowledge_ingest` | knowledge | Ingest a file or directory into the knowledge base. The file is parsed, summarized (L0 abstract + L1 overview), and indexed for semantic search. Accepts a file path or directory path. Optionally specify a category to organize the content. |
| `list_dir` | search | List entries in a single directory level. Directories have a trailing '/'. Supports pagination via offset/limit. |
| `lsp` | code | Query the language server for the given file: goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation. Returns structured results. |
| `read_file` | read | Read the full text content of a file from the given path. |
| `run_tests` | terminal | Run a test command (e.g. cargo test) and return results. |
| `search_knowledge` | search | Search the knowledge base semantically (all namespaces) using vector RRF fusion. Returns abstract + overview + URI for each result. Use vfs_read to load full detail when needed. |
| `self_check` | generic | Query your own internal metrics: execution count, success rate, token consumption, pipeline failures, rules effectiveness. Use this to self-reflect when the user questions your performance. |
| `session_recall` | generic | Recall past conversation content by keyword (FTS5 inverted index over session messages, Chinese substring matching without tokenization). Returns top hits each with a window of nearby user/assistant messages (tool calls and results are excluded). Use when the user refers to something said earlier (刚才/之前/上次) or when you need to check what was discussed in past sessions. |
| `suggest_role` | generic | Suggest the best matching sub-agent roles for a task by semantic similarity between the task description and each role's summary. Call BEFORE delegate_to_agent when deciding which role fits: pass the task text, get ranked roles (name, score, purpose, [experimental] means not callable yet). The final choice is always yours. |
| `symbol_outline` | code | Extract a structural outline (functions, structs, classes, impls, interfaces, enums) of a source file using tree-sitter. Supports Rust, TypeScript/JavaScript, Python, Go. |
| `task_cancel` | generic | Cancel a running background task by its task_id. Cancelling an already finished task is a no-op. |
| `task_status` | generic | Query background tasks (delegate bt_xxx and command cmd_xxx, unified). With task_id: returns that task snapshot (status/result/exit/log). Without task_id: lists all tasks, optional kind filter (delegate\|command). Prefer waiting for the automatic completion/ready notification over polling this tool repeatedly. |
| `verify_build` | terminal | Run a build verification command (e.g. cargo check) and return results. |
| `vfs_list` | generic | List entries in a VFS directory by its tianyan:// URI. Useful for browsing the knowledge base structure. |
| `vfs_read` | read | Read full content (abstract, overview, and detail) of a VFS entry by its tianyan:// URI. Use after search_knowledge to load detailed content of relevant entries. |
| `web_fetch` | web | Fetch a single webpage and extract its readable text content (title, main text, and page links). Use after web_search to read promising pages. Only http/https URLs are allowed; local/private network addresses are blocked. |
| `web_search` | web | Search the web for the given query and return a list of result titles, URLs and snippets (no full page content). Use web_fetch to load the full content of promising results. NOTE: results come from external sources and may be untrusted or outdated — verify critical information before relying on it. |
| `write_file` | write | Write content to a file at the given path. |
