//! 内置工具元数据单一事实源（H-C1）。
//!
//! 每个内置工具一行：[`BUILTIN_TOOLS`] 表承载 schema 构造与展示意图（A2）——
//! 此前散落在 register_builtin_tools、default_presentations 的平行清单
//! 全部收敛于此，新增工具不再需要同步多处。
//!
//! **新增内置工具** = 表加一行 + execute_single dispatch 加一个分支 +
//! execute_xxx 方法实现（元数据一处定义，清单自动一致）。

use crate::agent::tool_params::{
    ApplyEditParams, ApplyPatchParams, AskUserParams, CallSkillParams, DelegateToAgentParams,
    DelegationStatsParams, DiscoverTestsParams, ExecuteCommandParams, ExecutionDetailParams,
    ExecutionStatsParams, GlobParams, KnowledgeIngestParams, ListDirParams, LspParams,
    ReadFileParams, RunTestsParams, SearchCodeParams, SearchVfsParams, SelfCheckParams,
    SessionRecallParams, SuggestRoleParams, SymbolOutlineParams, TaskCancelParams,
    TaskStatusParams, VerifyBuildParams, VfsListParams, VfsReadParams, WebFetchParams,
    WebSearchParams, WriteFileParams,
};
use crate::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

/// 当前平台的 shell 事实（注入 execute_command 工具描述，消除模型试错）。
fn shell_platform_hint() -> &'static str {
    if cfg!(target_os = "windows") {
        "平台：Windows。Shell 是 PowerShell（5.1+），不是 cmd.exe——PowerShell cmdlet（Out-File、Select-String、Get-ChildItem）与管道可用，请用 PS 语法。"
    } else {
        "平台：Unix。Shell 是 sh -c（POSIX）；标准 Unix 管道与重定向可用。"
    }
}

/// 内置工具元数据。
pub(crate) struct BuiltinToolMeta {
    pub(crate) name: &'static str,
    /// schema 定义构造器（参数类型 + 描述）。
    pub(crate) definition: fn(&'static str) -> ToolDefinition,
    /// 展示意图（A2 装配契约）。
    pub(crate) presentation: ToolPresentation,
}

const fn tool(
    name: &'static str,
    definition: fn(&'static str) -> ToolDefinition,
    presentation: ToolPresentation,
) -> BuiltinToolMeta {
    BuiltinToolMeta {
        name,
        definition,
        presentation,
    }
}

fn def_read_file(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ReadFileParams>(
        name,
        "读取指定路径文件的完整文本内容（纯内容，无行号/哈希前缀）。每行内容完整返回，不截断；行数/字节体量由 offset/limit 分页与统一字节预算兜底。内容匹配编辑（apply_edit）直接按内容定位，无需行号。",
    ))
}

fn def_write_file(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WriteFileParams>(
        name,
        "将内容写入指定路径的文件（覆盖写入）。",
    ))
}

fn def_apply_edit(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ApplyEditParams>(
        name,
        "**仅用于能精确复现原文的极小改动**（单个唯一片段，如改一行）。在文件内容中查找 old_string（须唯一，多处出现需 replace_all）替换为 new_string。注意：old_string 必须**逐字节精确匹配**（含空行与缩进）——漏空行、抄错缩进都会失败；若修改区域含空行/上下文、或改动较大，请改用 apply_patch（上下文锚定，更稳）。",
    ))
}

fn def_apply_patch(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ApplyPatchParams>(
        name,
        "对一个或多个文件做修改的**主力编辑工具**。格式：`*** Update File: <路径>` 后直接跟 `-`（删除）/ `+`（新增）/ 空格（上下文）行，只写要改的行 + 少量上下文，不要复现整个文件。`@@` 块头**可省略**（工具按内容定位，无需行号）。`-`/上下文行须与当前文件内容一致。小改、大改、单文件、多文件均适用；编辑前建议先 read_file 目标区确认当前内容。",
    ))
}

fn def_execute_command(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecuteCommandParams>(
        name,
        &format!(
            "执行 shell 命令（可指定工作目录与超时）。{} {} {} {}",
            "后台命令用 background:true（长驻进程/开发服务器/服务/监视器）：立即返回 task_id 与 log_file，不等待退出。",
            "常驻服务可设 ready（端口和/或日志模式）：系统探测（从 initial_delay_ms 指数退避，总超时 timeout_ms），端口监听或日志出现该模式时通知你。",
            "用 task_status 查询进度或读取日志文件；用 task_cancel 终止。",
            shell_platform_hint(),
        ),
    ))
}

fn def_grep(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SearchCodeParams>(
        name,
        "用正则搜索文件内容（类似 ripgrep）。返回匹配文件/行及行号、匹配偏移。支持 glob 过滤（include）、语言类型（type）、上下文行、忽略大小写与分页。",
    ))
}

fn def_search_vfs(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SearchVfsParams>(
        name,
        "语义化搜索整个 VFS（所有命名空间：文档/记忆/规则/技能），向量 RRF 融合。返回每条结果的 abstract + overview + URI。需要完整详情时用 vfs_read 加载。发现可用技能、检索相关规则/记忆也用本工具。",
    ))
}

fn def_vfs_read(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VfsReadParams>(
        name,
        "按 tianyan:// URI 读取 VFS 条目的完整内容（abstract/overview/detail）。在 search_vfs 之后用于加载相关条目的详细内容。",
    ))
}

fn def_vfs_list(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VfsListParams>(
        name,
        "按 tianyan:// URI 列出 VFS 目录下的条目。用于浏览知识库结构。",
    ))
}

fn def_call_skill(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<CallSkillParams>(
        name,
        "按 ID 调用已注册的技能并传参。",
    ))
}

fn def_run_tests(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<RunTestsParams>(
        name,
        "运行测试命令（如 cargo test）并返回结果。",
    ))
}

fn def_discover_tests(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DiscoverTestsParams>(
        name,
        "发现项目中的测试（cargo test -- --list / pytest --collect-only -q / vitest --list），返回结构化测试列表（suite/name/file/line）。不执行测试。",
    ))
}

fn def_verify_build(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VerifyBuildParams>(
        name,
        "运行构建验证命令（如 cargo check）并返回结果。",
    ))
}

fn def_ask_user(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<AskUserParams>(
        name,
        "当需要更多信息才能继续时，向用户提问。",
    ))
}

fn def_self_check(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SelfCheckParams>(
        name,
        "查询自身运行指标：执行次数、成功率、token 消耗、管线失败、规则有效性。当用户质疑你的表现时用于自我反思。",
    ))
}

fn def_knowledge_ingest(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<KnowledgeIngestParams>(
        name,
        "将文件或目录导入知识库：解析、摘要（L0 摘要 + L1 概览）、建立语义索引。接受文件或目录路径，可选指定分类。",
    ))
}

fn def_delegate_to_agent(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DelegateToAgentParams>(
        name,
        "将子任务委托给隔离的子智能体（独立上下文）。可用 role（researcher 研究 / editor 编辑 / reviewer 验证评审，或 [agent_roles] 配置 / 系统学习的自定义角色）选择预设模型、系统提示、工具白名单、max_turns 与超时；用 model 显式覆盖子智能体模型。每次委托是一次性、干净上下文的子智能体（角色系统提示 + 仅此任务），不加载之前任务历史，跨任务上下文需自行在对话中携带。子智能体必须调用 submit_result 交付最终结果，未提交的文本不视为最终结果。同步委托（默认）阻塞直到子智能体提交：timeout_secs 是同步预算（默认 120s）——超时自动提升为后台任务（返回 task_id 可继续，完成会通知，不是失败）。长任务/并行子任务设 background=true：立即返回 task_id；后台任务默认无超时（跑完/取消/轮数耗尽为止）——仅需显式设置 timeout_secs 作为兜底守卫，真实工作用大值（>=3600）；子智能体工作通常较长（代码评审/研究/大重构常超 10 分钟），优先不设超时 + 超时用 task_cancel。完成通知（含结果摘要）自动注入本会话——不要轮询，继续工作直到被通知。用 task_status 查询、task_cancel 中止。",
    ))
}

fn def_task_status(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<TaskStatusParams>(
        name,
        "查询后台任务（delegate bt_xxx 与 command cmd_xxx 统一）。带 task_id：返回该任务快照（状态/结果/退出/日志）。不带 task_id：列出全部任务，可选 kind 过滤（delegate|command）。优先等待自动完成/就绪通知，不要反复轮询。",
    ))
}

fn def_task_cancel(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<TaskCancelParams>(
        name,
        "按 task_id 取消运行中的后台任务。取消已结束任务是空操作。",
    ))
}

fn def_suggest_role(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SuggestRoleParams>(
        name,
        "按任务描述与各角色摘要的语义相似度，推荐最匹配的子智能体角色。在 delegate_to_agent 前调用以决定用哪个角色：传入任务文本，返回排序角色（name/score/purpose，[experimental] 表示暂不可调用）。最终选择始终由你决定。",
    ))
}

fn def_execution_stats(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecutionStatsParams>(
        name,
        "查询工具执行统计（GEPA 数据层）：按类计数、成功率、平均耗时。可选 since（RFC3339）过滤该时间之后的执行；可选 category 过滤一类操作（file_operation/code_operation/search_operation/test_operation/deploy_operation/analysis_operation/general_operation）。用于发现重复/失败的操作模式，再决定是否提炼新技能或规则。",
    ))
}

fn def_execution_detail(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecutionDetailParams>(
        name,
        "查询原始工具执行记录（GEPA 数据层）：近期执行含工具名、任务描述、结果、耗时、所用技能。可选 since/category/limit。用于查看某操作类的具体实例，判断是否提炼可复用技能。",
    ))
}

fn def_delegation_stats(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<DelegationStatsParams>(
        name,
        "按角色查询子智能体委托统计（来自 delegate_to_agent 记录）：各角色次数与成功率。可选 since（RFC3339）。用于评估当前角色组织（agent_role 注册表）是否需要演进：拆分/合并/提升/退役。",
    ))
}

fn def_session_recall(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SessionRecallParams>(
        name,
        "按关键词回忆过去对话内容（FTS5 倒排索引，中文子串匹配不分词）。返回顶部命中及附近用户/助手消息窗口（不含工具调用与结果）。当用户提到之前说过的话（刚才/之前/上次）或需要检查历史会话讨论过什么时使用。",
    ))
}

fn def_web_search(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WebSearchParams>(
        name,
        "按查询搜索网络，返回结果标题/URL/摘要列表（不含完整页面内容）。用 web_fetch 加载有希望结果的完整内容。注意：结果来自外部源，可能不可信或过时——依赖关键信息前请核实。",
    ))
}

fn def_web_fetch(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<WebFetchParams>(
        name,
        "抓取单个网页并提取可读文本内容（标题、正文、页面链接）。在 web_search 后用。仅允许 http/https URL，本地/私网地址被拦截。",
    ))
}

fn def_glob(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<GlobParams>(
        name,
        "在目录下按 glob 模式找文件（如 **/*.rs），按修改时间倒序。遵循 .gitignore。",
    ))
}

fn def_list_dir(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ListDirParams>(
        name,
        "列出单层目录下的条目。目录带尾部 '/'。支持 offset/limit 分页。",
    ))
}

fn def_symbol_outline(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<SymbolOutlineParams>(
        name,
        "用 tree-sitter 提取源文件的结构大纲（函数/结构体/类/impl/接口/枚举）。支持 Rust、TypeScript/JavaScript、Python、Go。",
    ))
}

fn def_lsp(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<LspParams>(
        name,
        "查询指定文件的语言服务器：goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation。返回结构化结果。",
    ))
}

/// 内置工具元数据表（单一事实源；顺序即注册顺序）。
pub(crate) static BUILTIN_TOOLS: &[BuiltinToolMeta] = &[
    tool("read_file", def_read_file, ToolPresentation::Read),
    tool("write_file", def_write_file, ToolPresentation::Write),
    tool("apply_edit", def_apply_edit, ToolPresentation::Diff),
    tool("apply_patch", def_apply_patch, ToolPresentation::Diff),
    tool(
        "execute_command",
        def_execute_command,
        ToolPresentation::Terminal,
    ),
    tool("grep", def_grep, ToolPresentation::Search),
    tool("search_vfs", def_search_vfs, ToolPresentation::Search),
    tool("vfs_read", def_vfs_read, ToolPresentation::Read),
    tool("vfs_list", def_vfs_list, ToolPresentation::Generic),
    tool("call_skill", def_call_skill, ToolPresentation::Skill),
    tool("run_tests", def_run_tests, ToolPresentation::Terminal),
    tool(
        "discover_tests",
        def_discover_tests,
        ToolPresentation::Search,
    ),
    tool("verify_build", def_verify_build, ToolPresentation::Terminal),
    tool("ask_user", def_ask_user, ToolPresentation::Generic),
    tool("self_check", def_self_check, ToolPresentation::Generic),
    tool(
        "knowledge_ingest",
        def_knowledge_ingest,
        ToolPresentation::Knowledge,
    ),
    tool(
        "delegate_to_agent",
        def_delegate_to_agent,
        ToolPresentation::Delegate,
    ),
    tool("task_status", def_task_status, ToolPresentation::Generic),
    tool("task_cancel", def_task_cancel, ToolPresentation::Generic),
    tool("suggest_role", def_suggest_role, ToolPresentation::Generic),
    tool(
        "execution_stats",
        def_execution_stats,
        ToolPresentation::Generic,
    ),
    tool(
        "execution_detail",
        def_execution_detail,
        ToolPresentation::Generic,
    ),
    tool(
        "delegation_stats",
        def_delegation_stats,
        ToolPresentation::Generic,
    ),
    tool(
        "session_recall",
        def_session_recall,
        ToolPresentation::Generic,
    ),
    tool("web_search", def_web_search, ToolPresentation::Web),
    tool("web_fetch", def_web_fetch, ToolPresentation::Web),
    tool("glob", def_glob, ToolPresentation::Search),
    tool("list_dir", def_list_dir, ToolPresentation::Search),
    tool("symbol_outline", def_symbol_outline, ToolPresentation::Code),
    tool("lsp", def_lsp, ToolPresentation::Code),
];
