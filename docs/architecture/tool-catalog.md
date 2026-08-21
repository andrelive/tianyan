# 天演工具目录（自动生成）

> 本文件由 `cargo run -p tianyan-core --example tool_catalog` 自动生成。
> 手工修改将被覆盖；freshness 由 `scripts/gen-tool-catalog.ps1 -Check` 门禁。
> 生成原则（DSH gen-tool-catalog 吸收）：运行时注册是 schema 唯一真相源。

共 25 个工具。

| 工具 | 展示意图 | 描述 |
|------|---------|------|
| `apply_edit` | diff | Apply precise edits to a file using line-number + content-hash anchors. Each edit targets a line range verified by an anchor hash; edits are applied bottom-up after all anchors are validated (atomic batch). |
| `apply_patch` | diff | Apply a unified diff patch (*** Update File format) to one or more files. Supports fuzzy matching of context lines and multiple files in one patch. |
| `ask_user` | generic | Ask the user a question when more information is needed to proceed. |
| `call_skill` | skill | Call a registered skill by ID with parameters. |
| `delegate_to_agent` | delegate | Delegate a sub-task to an isolated sub-agent with its own context. Use role (researcher for research, editor for code editing, reviewer for verification/review, or custom roles configured in [agent_roles]) to pick a preset model, system prompt, tool allowlist, max_turns and timeout; use model to explicitly override the sub-agent model. Set background=true to run it as a fire-and-forget background task: the tool returns a task_id immediately, and a completion notification (with the result summary) is injected into this session automatically — do NOT poll, just continue working until notified. Use task_status to query a task, task_cancel to abort it. |
| `discover_tests` | search | Discover tests in the project (cargo test -- --list / pytest --collect-only -q / vitest --list) and return structured test list with suite, name, file and line. Does not execute tests. |
| `execute_command` | terminal | Execute a shell command with optional working directory and timeout. |
| `glob` | search | Find files by glob pattern (e.g. **/*.rs) under a directory, sorted by modification time (newest first). Respects .gitignore. |
| `knowledge_ingest` | knowledge | Ingest a file or directory into the knowledge base. The file is parsed, summarized (L0 abstract + L1 overview), and indexed for semantic search. Accepts a file path or directory path. Optionally specify a category to organize the content. |
| `list_dir` | search | List entries in a single directory level. Directories have a trailing '/'. Supports pagination via offset/limit. |
| `lsp` | code | Query the language server for the given file: goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation. Returns structured results. |
| `read_file` | read | Read the full text content of a file from the given path. |
| `run_tests` | terminal | Run a test command (e.g. cargo test) and return results. |
| `search_code` | search | Search for code patterns using ripgrep. |
| `search_knowledge` | search | Search the knowledge base semantically (all namespaces) using vector RRF fusion. Returns abstract + overview + URI for each result. Use vfs_read to load full detail when needed. |
| `self_check` | generic | Query your own internal metrics: execution count, success rate, token consumption, pipeline failures, rules effectiveness. Use this to self-reflect when the user questions your performance. |
| `symbol_outline` | code | Extract a structural outline (functions, structs, classes, impls, interfaces, enums) of a source file using tree-sitter. Supports Rust, TypeScript/JavaScript, Python, Go. |
| `task_cancel` | generic | Cancel a running background task by its task_id. Cancelling an already finished task is a no-op. |
| `task_status` | generic | Query the status and result of a background task by its task_id (bt_xxx). Returns a non-blocking snapshot. Prefer waiting for the automatic completion notification over polling this tool repeatedly. |
| `verify_build` | terminal | Run a build verification command (e.g. cargo check) and return results. |
| `vfs_list` | generic | List entries in a VFS directory by its tianyan:// URI. Useful for browsing the knowledge base structure. |
| `vfs_read` | read | Read full content (abstract, overview, and detail) of a VFS entry by its tianyan:// URI. Use after search_knowledge to load detailed content of relevant entries. NOTE: for `tianyan://session/{id}` URIs, content is exported from the SQLite session store as JSONL (ADR-018 compatibility layer). |
| `web_fetch` | web | Fetch a single webpage and extract its readable text content (title, main text, and page links). Use after web_search to read promising pages. Only http/https URLs are allowed; local/private network addresses are blocked. |
| `web_search` | web | Search the web for the given query and return a list of result titles, URLs and snippets (no full page content). Use web_fetch to load the full content of promising results. NOTE: results come from external sources and may be untrusted or outdated — verify critical information before relying on it. |
| `write_file` | write | Write content to a file at the given path. |
