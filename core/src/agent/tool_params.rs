use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::executor::edit::ContentEdit;

/// 读取文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileParams {
    /// 文件路径。
    pub path: String,
    /// 起始行号（1 起始，默认 1）；大文件按需读取时用它定位目标范围。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 返回行数上限（默认 2000，单窗口钳制至 2000）；按需读取目标范围时收窄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 写入文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileParams {
    /// 文件路径。
    pub path: String,
    /// 文件内容。
    pub content: String,
}

/// 应用编辑参数（内容匹配编辑，1..=20 条）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApplyEditParams {
    /// 文件路径。
    pub path: String,
    /// 编辑列表（每条 old_string 须唯一，除非 replace_all）。
    pub edits: Vec<ContentEdit>,
}

/// 执行命令参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteCommandParams {
    /// 命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 后台运行（true 时立即返回 task_id/log_file，进程独立运行，不等待退出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// 后台就绪探测（仅 background=true 生效）：端口监听或日志关键词匹配后
    /// 自动通知主 agent「服务已就绪」；不探测则只有进程退出才通知。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<ReadyProbeParams>,
}

/// 后台命令就绪探测参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadyProbeParams {
    /// 就绪判定端口：TCP 连接成功即就绪（如 3000）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// 就绪判定日志关键词：日志出现该文本即就绪（如 "Listening"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// 首次探测等待（毫秒；指数退避起点；缺省 500）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay_ms: Option<u64>,
    /// 就绪探测总超时（毫秒；超时未就绪 → 通知主 agent 失败；缺省 300000）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// 角色匹配建议参数（ADR-016 P3 向量路由）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SuggestRoleParams {
    /// 任务描述（委托前调用：让系统按角色摘要语义匹配给出建议）。
    pub task: String,
    /// 返回前几名（默认 3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
}

/// 搜索代码参数。
///
/// 字段命名对齐 ripgrep 参数：`pattern` 为正式字段名（serde alias 兼容旧载荷的
/// `query`），`path` 兼容旧 `scope`；其余为可选的 ripgrep 高级参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    /// 搜索模式（正则；兼容旧字段名 `query`）。
    #[serde(alias = "query")]
    pub pattern: String,
    /// 搜索根目录（相对路径按会话工作目录解析；缺省为会话工作目录，
    /// 无会话时回退进程当前目录；兼容旧字段名 `scope`）。
    #[serde(default, alias = "scope", skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// glob 过滤器（如 `*.rs`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// 输出模式："files_with_matches"（默认）| "content" | "count"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    /// 文件类型过滤器（rg --type，如 "rust"）。
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    /// 忽略大小写（-i）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    /// 显示行号（-n，content 模式默认 true）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_number: Option<bool>,
    /// 上下文行数（-C）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<usize>,
    /// 前文行数（-B）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_context: Option<usize>,
    /// 后文行数（-A）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_context: Option<usize>,
    /// 结果条数上限（默认 200）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_limit: Option<usize>,
    /// 分页偏移（0 起始）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 多行匹配（-U）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiline: Option<bool>,
}

/// 调用技能参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CallSkillParams {
    /// 技能 ID。
    pub skill_id: String,
    /// 参数。
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
}

/// 运行测试参数。
///
/// `command` 为显式命令（向后兼容旧必填契约，缺省时按 `framework`/`suite`/`filter`
/// 由项目探测（[`crate::executor::project::probe_project`]）解析默认命令模板）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunTestsParams {
    /// 显式测试命令（向后兼容旧必填字段；缺省时由 framework 解析默认命令）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 工作目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 框架："auto"（默认，按项目探测）| "cargo" | "pytest" | "vitest"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// 测试过滤（cargo test <filter> / pytest -k <filter>）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// 测试套件（cargo -p <suite> / pytest <suite-file>）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
}

/// 验证构建参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerifyBuildParams {
    /// 构建命令。
    pub command: String,
    /// 工作目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 超时秒数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 追问选项（对齐 DSH ask_user_question：N 个选项 + 1 个自定义输入）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserOption {
    /// 选项标签（用户选择后作为回答值）。
    pub label: String,
    /// 选项说明（可选；展示用，不参与回答值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 单个追问问题（多问题分步：每个问题一个 tab，对齐 DSH questions 数组）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserQuestion {
    /// 问题内容。
    pub question: String,
    /// 候选选项（可选；提供时前端渲染为选项列表 + 自定义输入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskUserOption>>,
}

/// 追问用户参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AskUserParams {
    /// 问题内容（单问题快捷形式；`questions` 提供时忽略）。
    pub question: String,
    /// 候选选项（单问题快捷形式；`questions` 提供时忽略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskUserOption>>,
    /// 多问题列表（可选；提供时按每个问题一个 tab 分步追问，
    /// 并附带一个"补充信息" tab 供用户补充其他内容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<Vec<AskUserQuestion>>,
}

/// 子代理提交结果参数（委托循环显式完成信号）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubmitResultParams {
    /// 最终结果全文（Markdown 报告 / 结论）。
    pub result: String,
    /// 一句话摘要（可选；通知展示用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// 搜索知识库参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchVfsParams {
    /// 检索查询（自然语言）。
    pub query: String,
    /// 返回结果数量上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
}

/// 读取 VFS 条目参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VfsReadParams {
    /// VFS URI（如 tianyan://knowledge/doc.md）。
    pub uri: String,
}

/// 列出 VFS 目录参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VfsListParams {
    /// VFS URI（如 tianyan://knowledge/），默认列出根目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// 委托子 Agent 参数。
///
/// 委托采用单轮调用：LLM 返回工具调用请求后，由外层 AgentLoop 自主决策。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateToAgentParams {
    /// 子任务描述。
    pub task: String,
    /// 系统提示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 子 Agent 最大循环轮数（默认 200，范围 1-500）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// 委托整体超时（秒）。超时返回错误，子 Agent 循环被中断。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 后台执行（默认 false）：true 时立即返回任务 ID，任务独立运行，
    /// 完成时自动向父会话注入通知（含结果摘要与剩余任务计数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// 子 Agent 角色名（内置 researcher / editor / reviewer，或 tianyan.toml
    /// `[agent_roles]` 节自定义角色）。角色提供模型、系统提示、工具白名单、
    /// max_turns 与 timeout_secs；同名参数显式指定时优先于角色值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 子 Agent 模型名称（与主 Agent 模型同空间；优先级：
    /// 显式 model > role.model > 主 Agent 模型）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// 后台任务状态查询参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskStatusParams {
    /// 后台任务 ID（delegate_to_agent(background) 返回的 task_id）；缺省列出全部任务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// 列表过滤（delegate | command；缺省全部）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// 后台任务取消参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskCancelParams {
    /// 后台任务 ID。
    pub task_id: String,
}

/// 自我检查参数（无参数 — Agent 自省查询内部指标）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SelfCheckParams {}

/// 知识导入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeIngestParams {
    /// 要导入的文件或目录路径。
    pub path: String,
    /// 可选的分类（如 technical, business, references, screenshots, photos, diagrams, code, data, projects, external）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// Web 搜索参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchParams {
    /// 搜索查询词。
    pub query: String,
    /// 返回结果上限（默认 8，最大 20）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
}

/// Web 抓取参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchParams {
    /// 目标 URL（仅 http/https；本地/内网地址被拒绝）。
    pub url: String,
    /// 返回内容字符上限（默认 50000，范围 500-200000）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_file_params_schema() {
        let def = crate::model::FunctionDefinition::from_schema::<ReadFileParams>(
            "read_file",
            "Read a file",
        );
        assert_eq!(def.name, "read_file");
        assert!(def.parameters.get("properties").is_some());
    }

    #[test]
    fn test_ask_user_params_serialization() {
        let params = AskUserParams {
            question: "What is your name?".to_string(),
            options: None,
            questions: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("What is your name?"));
    }
}

/// glob 查找文件参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GlobParams {
    /// glob 模式（如 `**/*.rs`、`*.toml`）。
    pub pattern: String,
    /// 搜索根目录（默认当前工作目录）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 列出目录条目参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDirParams {
    /// 目录路径。
    pub path: String,
    /// 分页偏移（0 起始，排序后应用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// 分页条数上限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 应用统一 diff 补丁参数（codex 风格 `*** Update File` 信封格式，可含多文件）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApplyPatchParams {
    /// 补丁文本（可能包含多个文件的补丁块）。
    pub patch: String,
}

/// 符号大纲提取参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SymbolOutlineParams {
    /// 源码文件路径。
    pub path: String,
}

/// 发现测试参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscoverTestsParams {
    /// 项目根目录（或项目内任意目录，向上探测）。
    pub path: String,
}

/// LSP 查询参数（lsp 工具：goToDefinition / findReferences / hover /
/// documentSymbol / workspaceSymbol / goToImplementation）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LspParams {
    /// 操作类型。
    pub operation: String,
    /// 目标文件路径（workspaceSymbol 也用它选择项目服务器；全操作必需）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// 目标行（0 起始；workspaceSymbol 可省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// 目标列（0 起始；workspaceSymbol 可省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<usize>,
    /// workspaceSymbol 查询关键字。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}
/// 执行统计查询参数（execution_stats；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionStatsParams {
    /// 起始时间（RFC3339，如 2026-08-18T00:00:00Z；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// 类别过滤（file_operation / code_operation / search_operation /
    /// test_operation / deploy_operation / analysis_operation / general_operation）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// 执行明细查询参数（execution_detail；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionDetailParams {
    /// 起始时间（RFC3339；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// 类别过滤。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 最大返回条数（默认 20，上限 50）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// 角色委托统计查询参数（delegation_stats；GEPA 数据层，ADR-017）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegationStatsParams {
    /// 起始时间（RFC3339；缺省 = 全部历史）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

/// 会话回忆查询参数（session_recall；FTS5 倒排索引，ADR-017 决策 6）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionRecallParams {
    /// 回忆关键词（中文子串匹配，无需分词；至少 3 个字符）。
    pub query: String,
    /// 最大命中数（默认 5，上限 20）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// 每个命中附近的窗口半径（前后各 N 条消息，默认 5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<i64>,
}
