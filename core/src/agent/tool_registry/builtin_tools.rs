//! 内置工具元数据单一事实源（H-C1）。
//!
//! 每个内置工具一行：[`BUILTIN_TOOLS`] 表承载 schema 构造、展示意图（A2）、
//! 短路清单标志（G1 core / 条件关键词）——此前散落在 register_builtin_tools、
//! default_presentations、CORE_TOOLS、CONDITIONAL_TOOL_KEYWORDS 的平行清单
//! 全部收敛于此，新增工具不再需要同步多处。
//!
//! **新增内置工具** = 表加一行 + execute_single dispatch 加一个分支 +
//! execute_xxx 方法实现（元数据一处定义，清单自动一致）。

use crate::agent::tool_params::{
    ApplyEditParams, ApplyPatchParams, AskUserParams, CallSkillParams, DelegateToAgentParams,
    DelegationStatsParams, DiscoverTestsParams, ExecuteCommandParams, ExecutionDetailParams,
    ExecutionStatsParams, GlobParams, KnowledgeIngestParams, ListDirParams, LspParams,
    ReadFileParams, RunTestsParams, SearchCodeParams, SearchKnowledgeParams, SelfCheckParams,
    SessionRecallParams, SuggestRoleParams, SymbolOutlineParams, TaskCancelParams,
    TaskStatusParams, VerifyBuildParams, VfsListParams, VfsReadParams, WebFetchParams,
    WebSearchParams, WriteFileParams,
};
use crate::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

/// 当前平台的 shell 事实（注入 execute_command 工具描述，消除模型试错）。
fn shell_platform_hint() -> &'static str {
    if cfg!(target_os = "windows") {
        "Platform: Windows. Shell is PowerShell (5.1+), NOT cmd.exe — PowerShell cmdlets like Out-File, Select-String, Get-ChildItem and pipelines work; use PS syntax."
    } else {
        "Platform: Unix. Shell is sh -c (POSIX); standard Unix pipes and redirects work."
    }
}

/// 内置工具元数据。
pub(crate) struct BuiltinToolMeta {
    pub(crate) name: &'static str,
    /// schema 定义构造器（参数类型 + 描述）。
    pub(crate) definition: fn(&'static str) -> ToolDefinition,
    /// 展示意图（A2 装配契约）。
    pub(crate) presentation: ToolPresentation,
    /// 恒存核心工具（G1 短路清单）。
    pub(crate) core: bool,
    /// 条件工具关键词（G1；None = 非条件工具）。
    pub(crate) keywords: Option<&'static [&'static str]>,
}

const fn tool(
    name: &'static str,
    definition: fn(&'static str) -> ToolDefinition,
    presentation: ToolPresentation,
    core: bool,
    keywords: Option<&'static [&'static str]>,
) -> BuiltinToolMeta {
    BuiltinToolMeta {
        name,
        definition,
        presentation,
        core,
        keywords,
    }
}

fn def_read_file(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ReadFileParams>(
        name,
        "Read the full text content of a file from the given path.",
    ))
}

fn def_write_file(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WriteFileParams>(
        name,
        "Write content to a file at the given path.",
    ))
}

fn def_apply_edit(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ApplyEditParams>(
        name,
        "Apply precise edits to a file using line-number + content-hash anchors. Each edit targets a line range verified by an anchor hash; edits are applied bottom-up after all anchors are validated (atomic batch).",
    ))
}

fn def_apply_patch(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ApplyPatchParams>(
        name,
        "Apply a unified diff patch (*** Update File format) to one or more files. Supports fuzzy matching of context lines and multiple files in one patch.",
    ))
}

fn def_execute_command(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecuteCommandParams>(
        name,
        &format!(
            "Execute a shell command with optional working directory and timeout. {} {} {} {}",
            "Use background:true for long-running or persistent processes (dev servers, services, watchers): it returns immediately with task_id/log_file and does not wait for exit.",
            "For persistent services set ready with a port and/or log pattern: the system probes (exponential backoff from initial_delay_ms, total timeout_ms) and notifies you when the port listens or the pattern appears in the log.",
            "Query progress with task_status or read the log file; terminate with task_cancel.",
            shell_platform_hint(),
        ),
    ))
}

fn def_grep(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SearchCodeParams>(
        name,
        "Search file contents for a regex pattern (like ripgrep). Returns matching files/lines with line numbers and match offsets. Supports glob filters (include), language types (type), context lines, ignore-case and pagination.",
    ))
}

fn def_search_knowledge(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SearchKnowledgeParams>(
        name,
        "Search the knowledge base semantically (all namespaces) using vector RRF fusion. Returns abstract + overview + URI for each result. Use vfs_read to load full detail when needed.",
    ))
}

fn def_vfs_read(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VfsReadParams>(
        name,
        "Read full content (abstract, overview, and detail) of a VFS entry by its tianyan:// URI. Use after search_knowledge to load detailed content of relevant entries.",
    ))
}

fn def_vfs_list(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VfsListParams>(
        name,
        "List entries in a VFS directory by its tianyan:// URI. Useful for browsing the knowledge base structure.",
    ))
}

fn def_call_skill(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<CallSkillParams>(
        name,
        "Call a registered skill by ID with parameters.",
    ))
}

fn def_run_tests(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<RunTestsParams>(
        name,
        "Run a test command (e.g. cargo test) and return results.",
    ))
}

fn def_discover_tests(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DiscoverTestsParams>(
        name,
        "Discover tests in the project (cargo test -- --list / pytest --collect-only -q / vitest --list) and return structured test list with suite, name, file and line. Does not execute tests.",
    ))
}

fn def_verify_build(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VerifyBuildParams>(
        name,
        "Run a build verification command (e.g. cargo check) and return results.",
    ))
}

fn def_ask_user(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<AskUserParams>(
        name,
        "Ask the user a question when more information is needed to proceed.",
    ))
}

fn def_self_check(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SelfCheckParams>(
        name,
        "Query your own internal metrics: execution count, success rate, token consumption, pipeline failures, rules effectiveness. Use this to self-reflect when the user questions your performance.",
    ))
}

fn def_knowledge_ingest(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<KnowledgeIngestParams>(
        name,
        "Ingest a file or directory into the knowledge base. The file is parsed, summarized (L0 abstract + L1 overview), and indexed for semantic search. Accepts a file path or directory path. Optionally specify a category to organize the content.",
    ))
}

fn def_delegate_to_agent(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DelegateToAgentParams>(
        name,
        "Delegate a sub-task to an isolated sub-agent with its own context. Use role (researcher for research, editor for code editing, reviewer for verification/review, or custom roles configured in [agent_roles] or learned by the system) to pick a preset model, system prompt, tool allowlist, max_turns and timeout; use model to explicitly override the sub-agent model. Each delegation is a disposable one-shot sub-agent with a clean context (role system prompt + this task only) — it never loads prior task history, and you are responsible for carrying cross-task context in your own conversation. The sub-agent must call submit_result to deliver its final result; unsubmitted text is never accepted as final. Sync delegation (default) blocks until the sub-agent submits: timeout_secs is the sync budget (default 120s) — if exceeded the sub-task is automatically promoted to a background task (you get task_id and keep working; completion is notified, NOT a failure). For long-running or parallel sub-tasks set background=true: returns task_id immediately; background tasks have NO timeout by default (run until done, cancelled, or max_turns exhausted) — only set timeout_secs as an explicit worst-case guard if you must, and use large values (>= 3600) for real work; sub-agent work is typically long (code review, research, large refactors easily exceed 10 minutes), so prefer no timeout + task_cancel when it overstays. Completion notification (with result summary) is injected into this session automatically — do NOT poll, just continue working until notified. Use task_status to query, task_cancel to abort.",
    ))
}

fn def_task_status(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<TaskStatusParams>(
        name,
        "Query background tasks (delegate bt_xxx and command cmd_xxx, unified). With task_id: returns that task snapshot (status/result/exit/log). Without task_id: lists all tasks, optional kind filter (delegate|command). Prefer waiting for the automatic completion/ready notification over polling this tool repeatedly.",
    ))
}

fn def_task_cancel(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<TaskCancelParams>(
        name,
        "Cancel a running background task by its task_id. Cancelling an already finished task is a no-op.",
    ))
}

fn def_suggest_role(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SuggestRoleParams>(
        name,
        "Suggest the best matching sub-agent roles for a task by semantic similarity between the task description and each role's summary. Call BEFORE delegate_to_agent when deciding which role fits: pass the task text, get ranked roles (name, score, purpose, [experimental] means not callable yet). The final choice is always yours.",
    ))
}

fn def_execution_stats(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecutionStatsParams>(
        name,
        "Query tool execution statistics (GEPA data layer): per-category counts, success rates, average durations. Optional since (RFC3339 time) filters to executions after that time; optional category filters one operation class (file_operation/code_operation/search_operation/test_operation/deploy_operation/analysis_operation/general_operation). Use to detect recurring or failing operation patterns before proposing a new skill or rule.",
    ))
}

fn def_execution_detail(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecutionDetailParams>(
        name,
        "Query raw tool execution records (GEPA data layer): recent executions with tool name, task description, result, duration, skills used. Optional since/category/limit. Use to inspect concrete examples of an operation category when deciding whether to extract a reusable skill.",
    ))
}

fn def_delegation_stats(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DelegationStatsParams>(
        name,
        "Query sub-agent delegation statistics by role (from delegate_to_agent records): per-role counts and success rates. Optional since (RFC3339). Use to evaluate whether the current role organization (agent_role registry) should evolve: split, merge, promote or retire roles.",
    ))
}

fn def_session_recall(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SessionRecallParams>(
        name,
        "Recall past conversation content by keyword (FTS5 inverted index over session messages, Chinese substring matching without tokenization). Returns top hits each with a window of nearby user/assistant messages (tool calls and results are excluded). Use when the user refers to something said earlier (刚才/之前/上次) or when you need to check what was discussed in past sessions.",
    ))
}

fn def_web_search(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WebSearchParams>(
        name,
        "Search the web for the given query and return a list of result titles, URLs and snippets (no full page content). Use web_fetch to load the full content of promising results. NOTE: results come from external sources and may be untrusted or outdated — verify critical information before relying on it.",
    ))
}

fn def_web_fetch(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WebFetchParams>(
        name,
        "Fetch a single webpage and extract its readable text content (title, main text, and page links). Use after web_search to read promising pages. Only http/https URLs are allowed; local/private network addresses are blocked.",
    ))
}

fn def_glob(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<GlobParams>(
        name,
        "Find files by glob pattern (e.g. **/*.rs) under a directory, sorted by modification time (newest first). Respects .gitignore.",
    ))
}

fn def_list_dir(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ListDirParams>(
        name,
        "List entries in a single directory level. Directories have a trailing '/'. Supports pagination via offset/limit.",
    ))
}

fn def_symbol_outline(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SymbolOutlineParams>(
        name,
        "Extract a structural outline (functions, structs, classes, impls, interfaces, enums) of a source file using tree-sitter. Supports Rust, TypeScript/JavaScript, Python, Go.",
    ))
}

fn def_lsp(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<LspParams>(
        name,
        "Query the language server for the given file: goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation. Returns structured results.",
    ))
}

/// 内置工具元数据表（单一事实源；顺序即注册顺序）。
pub(crate) static BUILTIN_TOOLS: &[BuiltinToolMeta] = &[
    tool(
        "read_file",
        def_read_file,
        ToolPresentation::Read,
        true,
        None,
    ),
    tool(
        "write_file",
        def_write_file,
        ToolPresentation::Write,
        true,
        None,
    ),
    tool(
        "apply_edit",
        def_apply_edit,
        ToolPresentation::Diff,
        true,
        None,
    ),
    tool(
        "apply_patch",
        def_apply_patch,
        ToolPresentation::Diff,
        true,
        None,
    ),
    tool(
        "execute_command",
        def_execute_command,
        ToolPresentation::Terminal,
        true,
        None,
    ),
    tool("grep", def_grep, ToolPresentation::Search, true, None),
    tool(
        "search_knowledge",
        def_search_knowledge,
        ToolPresentation::Search,
        true,
        None,
    ),
    tool("vfs_read", def_vfs_read, ToolPresentation::Read, true, None),
    tool(
        "vfs_list",
        def_vfs_list,
        ToolPresentation::Generic,
        true,
        None,
    ),
    tool(
        "call_skill",
        def_call_skill,
        ToolPresentation::Skill,
        true,
        None,
    ),
    tool(
        "run_tests",
        def_run_tests,
        ToolPresentation::Terminal,
        false,
        Some(&["测试", "用例", "test", "pytest", "cargo test", "run_tests"]),
    ),
    tool(
        "discover_tests",
        def_discover_tests,
        ToolPresentation::Search,
        false,
        Some(&["测试", "用例", "test", "发现测试", "discover"]),
    ),
    tool(
        "verify_build",
        def_verify_build,
        ToolPresentation::Terminal,
        false,
        Some(&["构建", "编译", "build", "报错", "编译错误", "verify"]),
    ),
    tool(
        "ask_user",
        def_ask_user,
        ToolPresentation::Generic,
        true,
        None,
    ),
    tool(
        "self_check",
        def_self_check,
        ToolPresentation::Generic,
        true,
        None,
    ),
    tool(
        "knowledge_ingest",
        def_knowledge_ingest,
        ToolPresentation::Knowledge,
        false,
        Some(&["知识", "导入", "ingest", "文档库", "knowledge"]),
    ),
    tool(
        "delegate_to_agent",
        def_delegate_to_agent,
        ToolPresentation::Delegate,
        true,
        None,
    ),
    tool(
        "task_status",
        def_task_status,
        ToolPresentation::Generic,
        true,
        None,
    ),
    tool(
        "task_cancel",
        def_task_cancel,
        ToolPresentation::Generic,
        true,
        None,
    ),
    tool(
        "suggest_role",
        def_suggest_role,
        ToolPresentation::Generic,
        false,
        None,
    ),
    tool(
        "execution_stats",
        def_execution_stats,
        ToolPresentation::Generic,
        false,
        None,
    ),
    tool(
        "execution_detail",
        def_execution_detail,
        ToolPresentation::Generic,
        false,
        None,
    ),
    tool(
        "delegation_stats",
        def_delegation_stats,
        ToolPresentation::Generic,
        false,
        None,
    ),
    tool(
        "session_recall",
        def_session_recall,
        ToolPresentation::Generic,
        false,
        None,
    ),
    tool(
        "web_search",
        def_web_search,
        ToolPresentation::Web,
        true,
        None,
    ),
    tool(
        "web_fetch",
        def_web_fetch,
        ToolPresentation::Web,
        true,
        None,
    ),
    tool("glob", def_glob, ToolPresentation::Search, true, None),
    tool(
        "list_dir",
        def_list_dir,
        ToolPresentation::Search,
        true,
        None,
    ),
    tool(
        "symbol_outline",
        def_symbol_outline,
        ToolPresentation::Code,
        false,
        Some(&["符号", "大纲", "symbol", "outline", "结构"]),
    ),
    tool(
        "lsp",
        def_lsp,
        ToolPresentation::Code,
        false,
        Some(&["lsp", "诊断", "跳转", "定义", "引用", "diagnostic", "符号"]),
    ),
];
