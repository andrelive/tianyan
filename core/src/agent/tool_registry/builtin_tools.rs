//! 内置工具元数据单一事实源（H-C1）。
//!
//! 每个内置工具一行：[`BUILTIN_TOOLS`] 表承载 schema 构造、展示意图（A2）与
//! **执行入口**——此前散落在 register_builtin_tools、default_presentations、
//! `execute_single` 的 match 分支三处平行清单全部收敛于此。
//!
//! **新增内置工具** = 表加一行（名称 + schema + 展示意图 + 执行入口）+ 对应
//! `execute_xxx` 方法实现。`ToolRegistry::execute_single` 只做一次查表分派，
//! **不再认识任何具体工具名**（ADR-049 期 0.5：match → 查表）。
//!
//! 执行入口统一为 [`BuiltinToolExec`]（`&ToolRegistry` + session_id + 原始参数
//! + subagent），各工具用一层薄适配（[`builtin_adapter!`] 生成）对齐签名——
//! 适配层只搬运形参、不含逻辑，因此分派语义与改造前逐分支等价（行为零变化）。

use std::future::Future;
use std::pin::Pin;

use super::ToolRegistry;
use crate::agent::tool_params::{
    AskUserParams, CallSkillParams, DelegateToAgentParams, DelegationStatsParams,
    ExecuteCommandParams, ExecutionDetailParams, ExecutionStatsParams, KnowledgeIngestParams,
    SearchVfsParams, SelfCheckParams, SessionRecallParams, SuggestRoleParams, TaskCancelParams,
    TaskStatusParams, VfsListParams, VfsReadParams, WebFetchParams, WebSearchParams,
};
use crate::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};

/// 当前 shell 事实（注入 execute_command 工具描述；ADR-037 单一事实源）。
///
/// 与执行侧 [`crate::executor::shell::ShellSpec`] **同源**——提示词随实际
/// 执行底层（auto 探测 / 用户配置）联动，杜绝"配置了 A 提示还说 B"的漂移。
///
/// **跨平台 freshness 归一化依赖**：`tool_catalog --check` 遍历
/// `executor::shell::HINT_TEMPLATES` 替换为占位再比较（CI 在 Linux、仓库
/// 产物在 Windows 生成，不归一化则门禁跨平台恒红）。
fn shell_platform_hint() -> String {
    crate::executor::shell::current_hint()
}

/// 内置工具执行的返回 future（[`BuiltinToolExec`] 签名的一部分）。
pub(crate) type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = crate::common::error::Result<serde_json::Value>> + Send + 'a>>;

/// 内置工具执行入口（分派查表的单一事实源）。
///
/// 统一签名 = 注册表 + 会话 + 原始参数 + 子代理标记。各工具用一层薄适配
/// （[`builtin_adapter!`] 生成）对齐——适配层只做形参搬运，不含任何逻辑。
///
/// **这是「查表分派」的机制前提**：表里有了执行入口，`execute_single` 就
/// 退化成一个查表 + 管线（前处理 / 守卫 / 后处理），内核不再认识工具名。
pub(crate) type BuiltinToolExec =
    for<'a> fn(&'a ToolRegistry, &'a str, &'a str, bool) -> ToolFuture<'a>;

/// 生成统一签名的执行适配器（**只搬运形参，不含逻辑**）。
///
/// 四种形参形态覆盖全部内置工具：无参 / 仅参数 / 参数+会话 / 参数+会话+子代理标记。
/// 用宏而非 28 份手写样板：形态差异是本文件唯一的重复来源，收敛后新增工具只需
/// 一行适配调用。
macro_rules! builtin_adapter {
    ($name:ident, $method:ident) => {
        fn $name<'a>(
            r: &'a ToolRegistry,
            _sid: &'a str,
            _args: &'a str,
            _sub: bool,
        ) -> ToolFuture<'a> {
            Box::pin(r.$method())
        }
    };
    ($name:ident, $method:ident, args) => {
        fn $name<'a>(
            r: &'a ToolRegistry,
            _sid: &'a str,
            args: &'a str,
            _sub: bool,
        ) -> ToolFuture<'a> {
            Box::pin(r.$method(args))
        }
    };
    ($name:ident, $method:ident, args, session) => {
        fn $name<'a>(
            r: &'a ToolRegistry,
            sid: &'a str,
            args: &'a str,
            _sub: bool,
        ) -> ToolFuture<'a> {
            Box::pin(r.$method(args, sid))
        }
    };
    ($name:ident, $method:ident, args, session, sub) => {
        fn $name<'a>(
            r: &'a ToolRegistry,
            sid: &'a str,
            args: &'a str,
            sub: bool,
        ) -> ToolFuture<'a> {
            Box::pin(r.$method(args, sid, sub))
        }
    };
}

// ---- 各工具执行入口（与 BUILTIN_TOOLS 表逐行对应）----
builtin_adapter!(
    exec_execute_command,
    execute_execute_command,
    args,
    session,
    sub
);
builtin_adapter!(exec_search_vfs, execute_search_vfs, args);
builtin_adapter!(exec_vfs_read, execute_vfs_read, args);
builtin_adapter!(exec_vfs_list, execute_vfs_list, args);
builtin_adapter!(exec_call_skill, execute_call_skill, args);
builtin_adapter!(exec_ask_user, execute_ask_user, args, session, sub);
builtin_adapter!(exec_self_check, execute_self_check);
builtin_adapter!(exec_knowledge_ingest, execute_knowledge_ingest, args);
builtin_adapter!(
    exec_delegate_to_agent,
    execute_delegate_to_agent,
    args,
    session
);
builtin_adapter!(exec_submit_result, execute_submit_result, args, session);
builtin_adapter!(exec_task_status, execute_task_status, args, session);
builtin_adapter!(exec_task_cancel, execute_task_cancel, args);
builtin_adapter!(exec_suggest_role, execute_suggest_role, args);
builtin_adapter!(exec_execution_stats, execute_execution_stats, args);
builtin_adapter!(exec_execution_detail, execute_execution_detail, args);
builtin_adapter!(exec_delegation_stats, execute_delegation_stats, args);
builtin_adapter!(exec_session_recall, execute_session_recall, args);
builtin_adapter!(exec_web_search, execute_web_search, args);
builtin_adapter!(exec_web_fetch, execute_web_fetch, args);

/// 内置工具元数据。
pub(crate) struct BuiltinToolMeta {
    pub(crate) name: &'static str,
    /// schema 定义构造器（参数类型 + 描述）。
    pub(crate) definition: fn(&'static str) -> ToolDefinition,
    /// 展示意图（A2 装配契约）。
    pub(crate) presentation: ToolPresentation,
    /// 执行入口（分派查表；与 `execute_xxx` 方法一一对应）。
    pub(crate) execute: BuiltinToolExec,
}

const fn tool(
    name: &'static str,
    definition: fn(&'static str) -> ToolDefinition,
    presentation: ToolPresentation,
    execute: BuiltinToolExec,
) -> BuiltinToolMeta {
    BuiltinToolMeta {
        name,
        definition,
        presentation,
        execute,
    }
}

fn def_execute_command(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<ExecuteCommandParams>(
        name,
        &format!(
            "执行 shell 命令（可指定工作目录与超时）。{} {} {} {} {} {}",
            "后台命令用 background:true（长驻进程/开发服务器/服务/监视器）：立即返回 task_id 与 log_file，不等待退出。",
            "常驻服务应设 ready——**必须同时给 background:true**，且 port（端口监听）与 pattern（日志关键词）**至少给一个**（可同时给：任一命中即就绪）：系统从 initial_delay_ms 指数退避探测，总超时 timeout_ms（缺省 60s）内未就绪也会通知你（服务继续运行）。",
            "用 task_status 查询进度或读取日志文件；用 task_cancel 终止。",
            "同步命令输出超限（50KB / 2000 行）自动截断并仅保留尾部；完整输出自动落盘，结果中给出 log_file 与 stdout_total_bytes，可用 read_file 的 offset/limit 分页读取全文。",
            shell_platform_hint(),
            "临时/中间产物（提交信息文件、一次性脚本、审计产物等）请写入系统临时目录的会话区（如 `$env:TEMP\\tianyan-scratch\\`），**不要**写应用数据目录根或安装目录。",
        ),
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
        "按 tianyan:// URI 读取 VFS 条目（简介 abstract / 目录 overview / 正文 detail）。在 search_vfs 之后加载相关条目；正文默认截断至 2000 行（50KB），用 offset/limit 按行取段——对齐目录（L1）中的章节行号。",
    ))
}

fn def_vfs_list(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<VfsListParams>(
        name,
        "按 tianyan:// URI 列出 VFS 目录下的条目（uri / is_directory / 简介 L0）。目录即导航索引：用于浏览知识库/技能结构、发现子条目（多文件技能的部件、文件夹内容）；命中某条目后想了解同族内容时也可用它。",
    ))
}

fn def_call_skill(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<CallSkillParams>(
        name,
        "按 ID 读取 VFS 技能文档（方法论文档：L0 摘要 + L2 详情）。技能无执行语义——读到内容后参考方法论自行用基础工具执行。planning 为预置技能，GEPA 学习技能由进化引擎写入。",
    ))
}

fn def_ask_user(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<AskUserParams>(
        name,
        "当需要更多信息才能继续时，向用户提问。只提供 `questions` 数组（单个问题也放进数组，长度 1）——每个问题一个 tab 分步展示；问题可带 `options`（字符串数组，候选选项；用户也可自行输入）。示例：{\"questions\":[{\"question\":\"是否继续？\",\"options\":[\"继续\",\"先停一下\"]}]}",
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
        "将子任务委托给隔离的子智能体（独立上下文）。可用 role（内置：researcher 研究 / editor 编辑 / reviewer 验证评审；其他自定义/学习角色用 suggest_role 按任务描述查询——experimental 角色不可调用）选择预设模型、系统提示、工具白名单、max_turns 与超时；用 model 显式覆盖子智能体模型。每次委托是一次性、干净上下文的子智能体（角色系统提示 + 仅此任务），不加载之前任务历史，跨任务上下文需自行在对话中携带。子智能体与主会话同构（ADR-030）：收尾统一\"无工具调用\"，任务完成时输出最终结果；可用 submit_result 把最终结果写入任务存储（可选结果落盘工具，返回 task_id）——调用后告知主智能体结果 ID，用 task_status 查询。委托一律异步（ADR-026）：立即返回 task_id，任务独立运行；后台任务默认无超时（跑完/取消/轮数耗尽为止）——仅需显式设置 timeout_secs 作为兜底守卫，真实工作用大值（>=3600）；子智能体工作通常较长（代码评审/研究/大重构常超 10 分钟），优先不设超时 + 超时用 task_cancel。**完成通知（含结果落盘位置与摘要）会自动注入本会话——不要轮询等待**：发起后继续其他工作或结束本轮，收到通知再处理（完整结果用 read_file 按通知中的路径读取）；task_cancel 中止。",
    ))
}

fn def_task_status(name: &'static str) -> ToolDefinition {
    ToolDefinition::function(FunctionDefinition::from_schema::<TaskStatusParams>(
        name,
        "查询后台任务（delegate bt_xxx 与 command cmd_xxx 统一）。默认范围：**当前会话工作目录**下所有会话的任务（同目录跨会话的协调面）——其他工作目录的任务默认不查、不用、不提；仅当竞争问题确实跨目录时用 scope=\"global\" 显式查全局（用完即回，其他目录信息不进入常规汇报）。带 task_id：返回该任务**完整快照**（含结果/输出尾部/日志路径；仅限可见范围）。不带 task_id：列出可见任务（**默认最近 20 条**，`limit` 可调、上限 100；条目为定位用精简字段——id/kind/status/描述截断/时间，完整内容按 task_id 查询），可选 kind 过滤（delegate|command）。**任务完成会自动通知本会话——不要为等待而轮询**；仅当用户要求查看进度或需要任务列表时调用。",
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
        "按关键词回忆过去对话内容（FTS5 倒排索引，中文子串匹配不分词）。返回命中及附近用户/助手消息窗口（不含工具调用与结果）；命中数默认 3（上限 10）、窗口半径默认 2（上限 5），单次输出有总预算截断（超出附续查指引）。当用户提到之前说过的话（刚才/之前/上次）或需要检查历史会话讨论过什么时使用。query 写具体词、一次一个概念；需要更宽语境时用更精确的关键词再查（或经 vfs_read 读会话原文）。可传 since_days 只看最近 N 天（用于'最近/这几天'类限定）。",
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

pub(crate) static BUILTIN_TOOLS: &[BuiltinToolMeta] = &[
    tool(
        "execute_command",
        def_execute_command,
        ToolPresentation::Terminal,
        exec_execute_command,
    ),
    tool(
        "search_vfs",
        def_search_vfs,
        ToolPresentation::Search,
        exec_search_vfs,
    ),
    tool(
        "vfs_read",
        def_vfs_read,
        ToolPresentation::Read,
        exec_vfs_read,
    ),
    tool(
        "vfs_list",
        def_vfs_list,
        ToolPresentation::Generic,
        exec_vfs_list,
    ),
    tool(
        "call_skill",
        def_call_skill,
        ToolPresentation::Skill,
        exec_call_skill,
    ),
    tool(
        "ask_user",
        def_ask_user,
        ToolPresentation::Generic,
        exec_ask_user,
    ),
    tool(
        "self_check",
        def_self_check,
        ToolPresentation::Generic,
        exec_self_check,
    ),
    tool(
        "knowledge_ingest",
        def_knowledge_ingest,
        ToolPresentation::Knowledge,
        exec_knowledge_ingest,
    ),
    tool(
        "delegate_to_agent",
        def_delegate_to_agent,
        ToolPresentation::Delegate,
        exec_delegate_to_agent,
    ),
    tool(
        "task_status",
        def_task_status,
        ToolPresentation::Generic,
        exec_task_status,
    ),
    tool(
        "task_cancel",
        def_task_cancel,
        ToolPresentation::Generic,
        exec_task_cancel,
    ),
    tool(
        "suggest_role",
        def_suggest_role,
        ToolPresentation::Generic,
        exec_suggest_role,
    ),
    tool(
        "execution_stats",
        def_execution_stats,
        ToolPresentation::Generic,
        exec_execution_stats,
    ),
    tool(
        "execution_detail",
        def_execution_detail,
        ToolPresentation::Generic,
        exec_execution_detail,
    ),
    tool(
        "delegation_stats",
        def_delegation_stats,
        ToolPresentation::Generic,
        exec_delegation_stats,
    ),
    tool(
        "session_recall",
        def_session_recall,
        ToolPresentation::Generic,
        exec_session_recall,
    ),
    tool(
        "web_search",
        def_web_search,
        ToolPresentation::Web,
        exec_web_search,
    ),
    tool(
        "web_fetch",
        def_web_fetch,
        ToolPresentation::Web,
        exec_web_fetch,
    ),
];

/// 协议工具元数据（**不暴露给 LLM**）。
///
/// 协议工具是循环协议的一部分（如 `submit_result` 的结果落盘），不是模型
/// 可见能力：它们**不进** `definitions`、展示意图与角色白名单，但**必须可执行**。
///
/// 与 [`BUILTIN_TOOLS`] **分表**而非共用一个 `hidden` 标记，是语义要求——
/// 元数据表驱动的是「模型可见工具集」，掺入不可见项就会让 LLM 看到一个
/// 不该调用的工具（行为变化）。
pub(crate) struct ProtocolToolMeta {
    /// 工具名（与 `execute` 入口一一对应）。
    pub(crate) name: &'static str,
    /// 执行入口。
    pub(crate) execute: BuiltinToolExec,
}

/// 协议工具执行表（当前仅 `submit_result`：ADR-030 可选结果落盘）。
///
/// **单一事实源**：`agent_ops::is_protocol_tool`（角色白名单豁免判定）由此
/// 派生，不再维护第二份名字清单。
pub(crate) static PROTOCOL_TOOLS: &[ProtocolToolMeta] = &[ProtocolToolMeta {
    name: super::agent_ops::SUBMIT_RESULT_TOOL,
    execute: exec_submit_result,
}];

/// 是否为协议工具（角色白名单豁免判定的单点，名字清单来自执行表）。
pub(crate) fn is_protocol_tool(name: &str) -> bool {
    PROTOCOL_TOOLS.iter().any(|t| t.name == name)
}

/// 按名解析**内置 + 协议**统一执行入口（分派查表入口）。
///
/// `None` = 不是编译期注册的工具，交由动态工具表兜底（MCP 桥接 / 剪贴板等）。
///
/// 线性扫描：单次工具调用的开销相对于工具本身可忽略，换来的是
/// **零额外分配 + 表序即注册序**（顺序是提示词前缀的稳定性前提）。
pub(crate) fn find_dispatch(name: &str) -> Option<BuiltinToolExec> {
    if let Some(meta) = BUILTIN_TOOLS.iter().find(|t| t.name == name) {
        return Some(meta.execute);
    }
    PROTOCOL_TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.execute)
}

/// 内置工具名清单（公开只读；跨 crate 一致性门禁的单一事实源）。
///
/// 消费方：server 侧 planning 技能提示词的"只读工具清单"门禁、内置角色
/// 白名单校验——工具面变更（新增/删除）时，这些手工清单最容易悄然漂移
/// （残留已删除工具名会引导模型调用不存在的工具），由测试统一拦截。
pub fn builtin_tool_names() -> Vec<&'static str> {
    BUILTIN_TOOLS.iter().map(|t| t.name).collect()
}
